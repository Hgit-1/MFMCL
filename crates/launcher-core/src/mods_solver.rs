use super::*;

pub(super) fn provider_map(
    indices: impl IntoIterator<Item = usize>,
    mods: &[InspectedMod],
) -> BTreeMap<String, Vec<(usize, String)>> {
    let mut result = BTreeMap::<String, Vec<(usize, String)>>::new();
    for index in indices {
        let item = &mods[index];
        result
            .entry(item.id.clone())
            .or_default()
            .push((index, item.version.clone()));
        for (id, version) in &item.provides {
            if id != &item.id {
                result
                    .entry(id.clone())
                    .or_default()
                    .push((index, version.clone()));
            }
        }
    }
    result
}

/// Nested Fabric/Quilt JARs are candidates, not mandatory root mods. Root IDs
/// win; only candidates needed by mandatory dependencies are activated. Search
/// is bounded and never executes loader code. JarJar is resolved by coordinates,
/// not by a mod's unrelated version or ID.
pub(super) fn select(
    mut mods: Vec<InspectedMod>,
    loader: &str,
    budget: &InspectionBudget,
    issues: &mut Vec<ModIssue>,
) -> (Vec<InspectedMod>, bool) {
    let mut complete = true;
    let mut artifacts = budget.jarjar_candidates.clone();
    let mut chosen = BTreeMap::new();
    for (key, candidates) in &mut artifacts {
        candidates.sort_by(|a, b| versions::preference(&b.0, &a.0, true).then(a.1.cmp(&b.1)));
        candidates.dedup();
        let ranges = &budget.jarjar_ranges[key];
        let candidate = candidates.iter().find(|(version, _)| {
            ranges
                .iter()
                .all(|range| versions::evaluate(range, version, true) == Match::Yes)
        });
        if let Some((_, file)) = candidate {
            chosen.insert(key.clone(), file.clone());
        } else {
            complete = false;
            let unknown = candidates.iter().any(|(version, _)| {
                ranges
                    .iter()
                    .any(|range| versions::evaluate(range, version, true) == Match::Unknown)
            });
            issues.push(finding(
                if unknown {
                    ModIssueSeverity::Warning
                } else {
                    ModIssueSeverity::Error
                },
                if unknown {
                    "unknown_version_constraint"
                } else {
                    "jarjar_resolution_conflict"
                },
                format!(
                    "JarJar 坐标 {key} 无法满足所有已读取的 Maven 范围，或包含不支持的版本语法。"
                ),
                candidates.iter().map(|(_, file)| file.clone()).collect(),
            ));
        }
    }
    mods.retain(|item| {
        item.containers
            .iter()
            .all(|(key, file)| chosen.get(key) == Some(file))
            && item
                .artifact
                .as_ref()
                .is_none_or(|(key, _)| chosen.get(key) == Some(&item.file))
    });
    let roots: Vec<_> = mods
        .iter()
        .enumerate()
        .filter(|(_, m)| !m.embedded || (m.forge.is_some() && m.artifact.is_some()))
        .map(|(i, _)| i)
        .collect();
    let mut candidates: Vec<_> = mods
        .iter()
        .enumerate()
        .filter(|(_, m)| m.embedded && m.forge.is_none() && !incompatible_loader(m, loader))
        .map(|(i, _)| i)
        .collect();
    candidates.sort_by(|a, b| {
        versions::preference(&mods[*b].version, &mods[*a].version, false)
            .then(mods[*a].file.cmp(&mods[*b].file))
    });
    fn search(
        selected: Vec<usize>,
        candidates: &[usize],
        mods: &[InspectedMod],
        loader: &str,
        steps: &mut usize,
        work: &mut usize,
    ) -> Option<Vec<usize>> {
        *steps += 1;
        if *steps > 4096 || *work > 32_768 || selected.len() > MAX_TOTAL_JARS {
            return None;
        }
        let environment = provider_map(
            selected
                .iter()
                .copied()
                .filter(|i| !incompatible_loader(&mods[*i], loader)),
            mods,
        );
        let mut unmet = None;
        for index in &selected {
            let item = &mods[*index];
            if incompatible_loader(item, loader) {
                continue;
            }
            for dependency in &item.depends {
                *work += 1;
                if *work > 32_768 {
                    return None;
                }
                if dependency_match(dependency, &environment, item.forge.is_some(), false)
                    == Match::No
                {
                    let can_supply = candidates.iter().any(|candidate| {
                        *work += 1;
                        if *work > 32_768 {
                            return false;
                        }
                        if selected.contains(candidate)
                            || selected.iter().any(|i| mods[*i].id == mods[*candidate].id)
                        {
                            return false;
                        }
                        let providers = provider_map(
                            selected.iter().copied().chain(std::iter::once(*candidate)),
                            mods,
                        );
                        dependency_match(dependency, &providers, item.forge.is_some(), false)
                            == Match::Yes
                    });
                    if *work > 32_768 {
                        return None;
                    }
                    if !can_supply {
                        if (item.embedded && item.forge.is_none())
                            || dependency_targets(dependency, &environment)
                                .iter()
                                .any(|i| mods[*i].embedded && mods[*i].forge.is_none())
                        {
                            return None;
                        }
                        continue;
                    }
                    unmet = Some((dependency, item.forge.is_some()));
                    break;
                }
            }
            if unmet.is_some() {
                break;
            }
        }
        let Some((dependency, forge)) = unmet else {
            return Some(selected);
        };
        for candidate in candidates {
            *work += 1;
            if *work > 32_768 {
                return None;
            }
            if selected.contains(candidate)
                || selected.iter().any(|i| mods[*i].id == mods[*candidate].id)
            {
                continue;
            }
            let mut next = selected.clone();
            next.push(*candidate);
            let providers = provider_map(next.iter().copied(), mods);
            if dependency_match(dependency, &providers, forge, false) != Match::Yes {
                continue;
            }
            let conflict = next.iter().any(|i| {
                let item = &mods[*i];
                let others = provider_map(
                    next.iter()
                        .copied()
                        .filter(|j| *j != *i && (*i == *candidate || *j == *candidate)),
                    mods,
                );
                item.conflicts.iter().any(|d| {
                    conflict_match(d, &providers, &others, item.forge.is_some()) == Match::Yes
                })
            });
            if conflict {
                continue;
            }
            if let Some(result) = search(next, candidates, mods, loader, steps, work) {
                return Some(result);
            }
        }
        None
    }
    let mut steps = 0;
    let mut work = 0;
    let selected = search(
        roots.clone(),
        &candidates,
        &mods,
        loader,
        &mut steps,
        &mut work,
    );
    let selected = if let Some(selected) = selected {
        selected
    } else {
        // Keep root findings actionable. A failed candidate search is reported
        // separately; unused nested candidates never become certain providers.
        if !candidates.is_empty() || steps > 4096 || work > 32_768 {
            issues.push(finding(ModIssueSeverity::Warning, if steps > 4096 || work > 32_768 { "nested_solver_limit" } else { "nested_resolution_conflict" }, "嵌套候选不能满足所有根模组依赖，或搜索达到 4096 状态 / 32768 候选检查限额；未把候选当成已加载模组。", vec![]));
            if steps > 4096 || work > 32_768 {
                complete = false;
            }
        }
        roots
    };
    let selected: BTreeSet<_> = selected.into_iter().collect();
    (
        mods.into_iter()
            .enumerate()
            .filter(|(i, _)| selected.contains(i))
            .map(|(_, m)| m)
            .collect(),
        complete,
    )
}
