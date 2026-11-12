use super::*;

pub(super) fn read<R: Read + Seek>(
    archive: &mut zip::ZipArchive<R>,
    name: &str,
    loader: &str,
    budget: &mut InspectionBudget,
) -> Result<Option<Vec<InspectedMod>>, ModIssue> {
    let order = if loader == "forge" {
        [
            ("META-INF/mods.toml", "forge"),
            ("META-INF/neoforge.mods.toml", "neoforge"),
        ]
    } else {
        [
            ("META-INF/neoforge.mods.toml", "neoforge"),
            ("META-INF/mods.toml", "forge"),
        ]
    };
    for (path, descriptor_kind) in order {
        let Some(bytes) = read_entry(archive, path, name, MAX_METADATA_BYTES, budget)? else {
            continue;
        };
        let text = std::str::from_utf8(&bytes)
            .map_err(|_| file_error("invalid_metadata", "模组 TOML 元数据不是 UTF-8。", name))?;
        let root: toml::Value = toml::from_str(text)
            .map_err(|_| file_error("invalid_metadata", "模组 TOML 元数据损坏", name))?;
        let invalid = || file_error("invalid_metadata", "模组 TOML 字段无效", name);
        let kind = if descriptor_kind == "forge"
            && root
                .get("dependencies")
                .and_then(toml::Value::as_table)
                .is_some_and(|all| {
                    all.values().any(|items| {
                        items.as_array().is_some_and(|items| {
                            items.iter().any(|dependency| {
                                dependency.get("modId").and_then(toml::Value::as_str)
                                    == Some("neoforge")
                            })
                        })
                    })
                }) {
            "neoforge"
        } else {
            descriptor_kind
        };
        let definitions = root
            .get("mods")
            .and_then(toml::Value::as_array)
            .ok_or_else(invalid)?;
        if definitions.is_empty() || definitions.len() > 256 {
            return Err(invalid());
        }
        let mut manifest_version = String::new();
        if definitions.iter().any(|definition| {
            definition.get("version").and_then(toml::Value::as_str) == Some("${file.jarVersion}")
        }) {
            if let Some(bytes) = read_entry(
                archive,
                "META-INF/MANIFEST.MF",
                name,
                MAX_METADATA_BYTES,
                budget,
            )? {
                if let Ok(text) = std::str::from_utf8(&bytes) {
                    let mut attributes: Vec<String> = Vec::new();
                    for line in text.lines() {
                        if line.is_empty() {
                            break;
                        }
                        if let Some(continuation) = line.strip_prefix(' ') {
                            if let Some(last) = attributes.last_mut() {
                                last.push_str(continuation);
                            }
                        } else {
                            attributes.push(line.into());
                        }
                    }
                    let mut versions = attributes
                        .iter()
                        .filter_map(|line| line.split_once(':'))
                        .filter(|(key, _)| key.eq_ignore_ascii_case("Implementation-Version"));
                    if let Some((_, version)) = versions.next() {
                        if versions.next().is_none() {
                            manifest_version = version.trim().into();
                        }
                    }
                }
            }
        }
        let mut result = Vec::new();
        for definition in definitions {
            let id = definition
                .get("modId")
                .and_then(toml::Value::as_str)
                .filter(|id| valid_id(id))
                .ok_or_else(invalid)?;
            let version = definition
                .get("version")
                .and_then(toml::Value::as_str)
                .map(|value| {
                    if value == "${file.jarVersion}" {
                        manifest_version.as_str()
                    } else {
                        value
                    }
                })
                .filter(|value| !value.contains("${"))
                .unwrap_or("");
            let mut item = InspectedMod {
                file: name.into(),
                id: id.into(),
                version: version.into(),
                provides: vec![],
                depends: vec![],
                conflicts: vec![],
                nested: vec![],
                embedded: false,
                artifact: None,
                containers: vec![],
                quilt: false,
                forge: Some(kind),
            };
            if let Some(dependencies) = root.get("dependencies").and_then(|all| all.get(id)) {
                let dependencies = dependencies.as_array().ok_or_else(invalid)?;
                if dependencies.len() > 1024 {
                    return Err(invalid());
                }
                for declaration in dependencies {
                    let dependency_id = declaration
                        .get("modId")
                        .and_then(toml::Value::as_str)
                        .filter(|id| valid_id(id))
                        .ok_or_else(invalid)?;
                    let side = declaration
                        .get("side")
                        .and_then(toml::Value::as_str)
                        .unwrap_or("BOTH");
                    if side.eq_ignore_ascii_case("SERVER") {
                        continue;
                    }
                    if !side.eq_ignore_ascii_case("BOTH") && !side.eq_ignore_ascii_case("CLIENT") {
                        return Err(invalid());
                    }
                    let dependency_type = declaration.get("type").and_then(toml::Value::as_str);
                    let mandatory = declaration
                        .get("mandatory")
                        .map(|v| v.as_bool().ok_or_else(invalid))
                        .transpose()?;
                    let required = dependency_type
                        .map(|value| value == "required")
                        .unwrap_or_else(|| mandatory.unwrap_or(kind == "neoforge"));
                    let incompatible = dependency_type == Some("incompatible");
                    let version_range = declaration
                        .get("versionRange")
                        .and_then(toml::Value::as_str)
                        .unwrap_or("");
                    let versions = serde_json::Value::String(
                        if version_range.is_empty() {
                            "*"
                        } else {
                            version_range
                        }
                        .into(),
                    );
                    if required {
                        item.depends.push(Dependency::new(dependency_id, versions));
                    } else if incompatible {
                        item.conflicts
                            .push(Dependency::new(dependency_id, versions));
                    }
                }
            }
            result.push(item);
        }
        return Ok(Some(result));
    }
    Ok(None)
}
