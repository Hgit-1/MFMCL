use super::*;
use serde_json::{Value, json};
use std::io::Write;
use zip::{ZipWriter, write::SimpleFileOptions};

struct Fixture {
    root: PathBuf,
    instance: InstanceRecord,
}

impl Fixture {
    fn new() -> Self {
        let root =
            std::env::temp_dir().join(format!("mfmcl-mod-inspection-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(root.join("mods")).unwrap();
        let instance = InstanceRecord {
            id: "inspection".into(),
            name: "inspection".into(),
            game_version: "1.21.4".into(),
            loader: "fabric@0.16.10".into(),
            instance_dir: root.clone(),
            game_dir: root.clone(),
            isolated: true,
            state: crate::InstanceState::Ready,
            created_at: String::new(),
            last_launched_at: None,
        };
        Self { root, instance }
    }

    fn jar(&self, file: &str, entries: &[(&str, &[u8])]) {
        let mut writer =
            ZipWriter::new(fs::File::create(self.root.join("mods").join(file)).unwrap());
        for (name, bytes) in entries {
            writer
                .start_file(
                    *name,
                    SimpleFileOptions::default()
                        .compression_method(zip::CompressionMethod::Deflated),
                )
                .unwrap();
            writer.write_all(bytes).unwrap();
        }
        writer.finish().unwrap();
    }

    fn fabric(&self, file: &str, metadata: Value) {
        self.jar(
            file,
            &[("fabric.mod.json", &serde_json::to_vec(&metadata).unwrap())],
        );
    }

    fn quilt(&self, file: &str, metadata: Value) {
        self.jar(
            file,
            &[("quilt.mod.json", &serde_json::to_vec(&metadata).unwrap())],
        );
    }

    fn inspect(&self) -> Vec<ModIssue> {
        inspect_mods(&self.instance)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn fabric(id: &str) -> Value {
    json!({"schemaVersion": 1, "id": id, "version": "1.2.3"})
}
fn quilt(id: &str) -> Value {
    json!({"schema_version": 1, "quilt_loader": {"id": id, "version": "1.2.3"}})
}
fn has(issues: &[ModIssue], code: &str) -> bool {
    issues.iter().any(|issue| issue.code == code)
}

#[test]
fn forge_and_neoforge_descriptors_check_required_dependencies_and_duplicate_ids() {
    for label in ["forge", "neoforge"] {
        let mut f = Fixture::new();
        f.instance.loader = format!("{label}@1.0");
        let descriptor = if label == "forge" {
            "META-INF/mods.toml"
        } else {
            "META-INF/neoforge.mods.toml"
        };
        let text = r#"
modLoader = "javafml"
loaderVersion = "[1,)"
[[mods]]
modId = "main_mod"
version = "1.0"
[[dependencies.main_mod]]
modId = "needed"
mandatory = true
side = "CLIENT"
[[dependencies.main_mod]]
modId = "optional_mod"
mandatory = false
type = "optional"
[[dependencies.main_mod]]
modId = "server_only"
mandatory = true
side = "SERVER"
"#;
        f.jar("main.jar", &[(descriptor, text.as_bytes())]);
        let issues = f.inspect();
        assert!(issues.iter().any(|issue| issue.code == "missing_dependency" && issue.message.contains("needed")));
        assert!(
            !issues
                .iter()
                .any(|issue| issue.message.contains("optional_mod")
                    || issue.message.contains("server_only"))
        );
        let dependency = b"[[mods]]\nmodId = \"needed\"\nversion = \"1.0\"";
        f.jar("dependency.jar", &[(descriptor, dependency)]);
        assert!(!has(&f.inspect(), "missing_dependency"));
        f.jar("duplicate.jar", &[(descriptor, dependency)]);
        assert!(has(&f.inspect(), "duplicate_mod_id"));
    }
}

#[test]
fn empty_and_missing_mod_directories_have_no_findings() {
    let f = Fixture::new();
    assert!(f.inspect().is_empty());
    fs::remove_dir(f.root.join("mods")).unwrap();
    assert!(f.inspect().is_empty());
}

#[test]
fn duplicate_ids_are_metadata_based_disabled_jars_are_ignored_and_results_are_stable() {
    let f = Fixture::new();
    f.fabric("z-unrelated-name.jar", fabric("same_id"));
    f.fabric("a-unrelated-name.jar", fabric("same_id"));
    fs::write(f.root.join("mods/broken.jar.disabled"), b"not a zip").unwrap();
    let issues = f.inspect();
    assert_eq!(issues.len(), 1);
    assert_eq!(issues[0].code, "duplicate_mod_id");
    assert_eq!(issues[0].severity, ModIssueSeverity::Error);
    assert_eq!(
        issues[0].files,
        ["a-unrelated-name.jar", "z-unrelated-name.jar"]
    );
    assert_eq!(issues, f.inspect());
    let encoded = serde_json::to_value(&issues[0]).unwrap();
    assert_eq!(encoded["severity"], "error");
    assert_eq!(encoded.as_object().unwrap().len(), 4);
    assert_eq!(
        serde_json::from_value::<ModIssue>(encoded).unwrap(),
        issues[0]
    );
}

#[test]
fn mandatory_dependencies_use_ids_and_aliases_not_filenames_and_ignore_builtins() {
    let f = Fixture::new();
    let mut main = fabric("main_mod");
    main["depends"] = json!({"absent_mod": ">=99 <100", "alias_mod": "*", "minecraft": "*", "java": "*", "fabricloader": "*", "quilt_loader": "*", "fabric-loader": "*", "quilt-loader": "*", "forge": "*", "neoforge": "*"});
    main["recommends"] = json!({"optional_recommendation": "*"});
    main["suggests"] = json!({"optional_suggestion": "*"});
    f.fabric("main.jar", main);
    let mut provider = fabric("provider_mod");
    provider["provides"] = json!(["alias_mod"]);
    f.fabric("absent_mod.jar", provider);
    let issues = f.inspect();
    assert_eq!(issues.len(), 1, "{issues:?}");
    assert_eq!(issues[0].code, "missing_dependency");
    assert!(issues[0].message.contains("absent_mod"));
    assert_eq!(issues[0].files, ["main.jar"]);
}

#[test]
fn quilt_optional_conditional_and_alternative_dependencies_are_evaluated() {
    let mut f = Fixture::new();
    f.instance.loader = "quilt@0.27.0".into();
    let mut main = quilt("main_mod");
    main["quilt_loader"]["depends"] = json!([
        {"id": "required_mod", "versions": ">=2"},
        {"id": "optional_mod", "optional": true},
        {"id": "conditional_mod", "unless": "another_mod"},
        ["alternative_a", "alternative_b"],
        "provided_mod", "minecraft", "quilt_loader"
    ]);
    f.quilt("main.jar", main);
    let mut provider = quilt("provider_mod");
    provider["quilt_loader"]["provides"] = json!([{"id": "provided_mod", "version": "9.0.0"}]);
    f.quilt("provider.jar", provider);
    let issues = f.inspect();
    assert_eq!(issues.len(), 3, "{issues:?}");
    assert!(
        issues
            .iter()
            .all(|issue| issue.code == "missing_dependency")
    );
    assert!(
        issues
            .iter()
            .any(|issue| issue.message.contains("required_mod"))
    );
    assert!(
        issues
            .iter()
            .any(|issue| issue.message.contains("conditional_mod"))
    );
    assert!(
        issues
            .iter()
            .any(|issue| issue.message.contains("alternative_a")
                && issue.message.contains("alternative_b"))
    );
}

#[test]
fn loader_checks_respect_fabric_on_quilt_and_dual_descriptors() {
    let mut f = Fixture::new();
    f.fabric("fabric.jar", fabric("fabric_mod"));
    for loader in ["fabric@1", "quilt@1", "unknown@1"] {
        f.instance.loader = loader.into();
        assert!(f.inspect().is_empty(), "{loader}");
    }
    for loader in ["vanilla", "forge@1", "neoforge@1"] {
        f.instance.loader = loader.into();
        assert!(has(&f.inspect(), "incompatible_loader"), "{loader}");
    }
    fs::remove_file(f.root.join("mods/fabric.jar")).unwrap();
    f.quilt("quilt.jar", quilt("quilt_mod"));
    f.instance.loader = "fabric@1".into();
    assert!(has(&f.inspect(), "incompatible_loader"));
    f.instance.loader = "quilt@1".into();
    assert!(f.inspect().is_empty());
    fs::remove_file(f.root.join("mods/quilt.jar")).unwrap();
    f.jar(
        "dual.jar",
        &[
            (
                "fabric.mod.json",
                &serde_json::to_vec(&fabric("dual_mod")).unwrap(),
            ),
            (
                "quilt.mod.json",
                &serde_json::to_vec(&quilt("dual_mod")).unwrap(),
            ),
        ],
    );
    for loader in ["fabric@1", "quilt@1"] {
        f.instance.loader = loader.into();
        assert!(f.inspect().is_empty(), "{loader}");
    }
}

#[test]
fn incompatible_quilt_mod_does_not_satisfy_fabric_dependencies() {
    let f = Fixture::new();
    let mut main = fabric("main_mod");
    main["depends"] = json!({"quilt_mod": "*"});
    f.fabric("main.jar", main);
    f.quilt("quilt.jar", quilt("quilt_mod"));
    let issues = f.inspect();
    assert!(has(&issues, "incompatible_loader"));
    assert!(has(&issues, "missing_dependency"));
}

#[test]
fn conflicts_evaluate_ranges_and_do_not_treat_partial_versions_as_wildcards() {
    let f = Fixture::new();
    let mut main = fabric("main_mod");
    main["breaks"] = json!({"wildcard_mod": "*", "exact_mod": ["0.0.1", "=1.2.3"], "range_mod": ">=1 <2", "partial_mod": "1.2", "absent_mod": "*", "minecraft": "*", "different_mod": "9.9.9"});
    main["conflicts"] = json!({"advisory_mod": "1.2.3"});
    f.fabric("main.jar", main);
    for id in [
        "wildcard_mod",
        "exact_mod",
        "range_mod",
        "partial_mod",
        "different_mod",
        "advisory_mod",
    ] {
        f.fabric(&format!("{id}.jar"), fabric(id));
    }
    let issues = f.inspect();
    assert_eq!(issues.len(), 4, "{issues:?}");
    assert!(issues.iter().all(|issue| issue.code == "declared_conflict"
        && issue.severity == ModIssueSeverity::Warning
        && issue.files.len() == 2));
    for issue in &issues {
        assert!(!issue.message.contains("partial_mod"));
    }
}

#[test]
fn quilt_breaks_conditions_alternatives_and_all_version_ranges_are_evaluated() {
    let mut f = Fixture::new();
    f.instance.loader = "quilt@1".into();
    let mut main = quilt("main_mod");
    main["quilt_loader"]["breaks"] = json!([
        {"id": "other_mod", "versions": "*"},
        {"id": "range_mod", "versions": {"all": [">1", "<2"]}},
        {"id": "conditional_mod", "unless": "missing_condition"},
        ["alternative_mod", "other_mod"]
    ]);
    f.quilt("main.jar", main);
    for id in [
        "other_mod",
        "range_mod",
        "conditional_mod",
        "alternative_mod",
    ] {
        f.quilt(&format!("{id}.jar"), quilt(id));
    }
    let issues = f.inspect();
    assert_eq!(issues.len(), 5, "{issues:?}");
    assert!(issues.iter().all(|issue| issue.code == "declared_conflict"));
}

#[test]
fn corrupt_unknown_or_nested_metadata_suppresses_false_absence_findings() {
    for case in [
        "bad_zip",
        "bad_json",
        "bad_schema",
        "bad_field",
        "unknown",
        "nested",
    ] {
        let f = Fixture::new();
        let mut main = fabric("main_mod");
        main["depends"] = json!({"possibly_nested_or_unreadable": "*"});
        f.fabric("main.jar", main);
        match case {
            "bad_zip" => fs::write(f.root.join("mods/other.jar"), b"broken").unwrap(),
            "bad_json" => f.jar("other.jar", &[("fabric.mod.json", b"{")]),
            "bad_schema" => {
                let mut metadata = fabric("other_mod");
                metadata["schemaVersion"] = json!(2);
                f.fabric("other.jar", metadata);
            }
            "bad_field" => {
                let mut metadata = fabric("other_mod");
                metadata["depends"] = json!([]);
                f.fabric("other.jar", metadata);
            }
            "unknown" => f.jar(
                "possibly_nested_or_unreadable.jar",
                &[("META-INF/mods.toml", b"not parsed")],
            ),
            "nested" => {
                let mut metadata = fabric("other_mod");
                metadata["jars"] = json!([{"file": "META-INF/jars/dependency.jar"}]);
                f.fabric("other.jar", metadata);
            }
            _ => unreachable!(),
        }
        let issues = f.inspect();
        assert!(has(&issues, "inspection_incomplete"), "{case}: {issues:?}");
        assert!(!has(&issues, "missing_dependency"), "{case}: {issues:?}");
    }
}

#[test]
fn compressed_metadata_size_is_bounded() {
    let f = Fixture::new();
    let mut metadata = fabric("large_mod");
    metadata["description"] = json!("x".repeat(MAX_METADATA_BYTES as usize));
    f.fabric("large.jar", metadata);
    assert!(has(&f.inspect(), "metadata_limit"));
}

#[test]
fn jar_count_and_size_are_bounded_before_parsing() {
    let f = Fixture::new();
    for index in 0..=MAX_MOD_JARS {
        fs::write(f.root.join(format!("mods/{index:04}.jar")), []).unwrap();
    }
    let issues = f.inspect();
    assert_eq!(issues.len(), 1);
    assert_eq!(issues[0].code, "jar_limit");
    for index in 0..=MAX_MOD_JARS {
        fs::remove_file(f.root.join(format!("mods/{index:04}.jar"))).unwrap();
    }
    fs::File::create(f.root.join("mods/large.jar"))
        .unwrap()
        .set_len(MAX_JAR_BYTES + 1)
        .unwrap();
    assert!(has(&f.inspect(), "archive_limit"));
}

#[test]
fn directory_entries_are_bounded_even_for_non_mod_files() {
    let f = Fixture::new();
    for index in 0..=MAX_DIRECTORY_ENTRIES {
        fs::write(f.root.join(format!("mods/{index}.txt")), []).unwrap();
    }
    let issues = f.inspect();
    assert_eq!(issues.len(), 1);
    assert_eq!(issues[0].code, "directory_limit");
}

#[test]
fn zip_entry_count_and_central_directory_are_bounded_before_archive_construction() {
    let f = Fixture::new();
    let mut writer = ZipWriter::new(fs::File::create(f.root.join("mods/many.jar")).unwrap());
    for index in 0..=MAX_ZIP_ENTRIES {
        writer
            .start_file(format!("{index}.txt"), SimpleFileOptions::default())
            .unwrap();
    }
    writer.finish().unwrap();
    assert!(has(&f.inspect(), "archive_limit"));
    fs::remove_file(f.root.join("mods/many.jar")).unwrap();
    let mut eocd = vec![0; 22];
    eocd[..4].copy_from_slice(b"PK\x05\x06");
    eocd[12..16].copy_from_slice(&((MAX_CENTRAL_BYTES + 1) as u32).to_le_bytes());
    fs::write(f.root.join("mods/forged.jar"), eocd).unwrap();
    assert!(has(&f.inspect(), "archive_limit"));
}

#[test]
fn case_ambiguous_filenames_and_metadata_are_rejected() {
    let f = Fixture::new();
    f.fabric("a.jar", fabric("one_mod"));
    let alias = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(f.root.join("mods/A.jar"));
    if let Ok(file) = alias {
        drop(file);
        f.fabric("A.jar", fabric("two_mod"));
        assert!(has(&f.inspect(), "ambiguous_file_name"));
        fs::remove_file(f.root.join("mods/A.jar")).unwrap();
    }
    fs::remove_file(f.root.join("mods/a.jar")).unwrap();
    f.jar(
        "case.jar",
        &[(
            "FABRIC.MOD.JSON",
            &serde_json::to_vec(&fabric("one_mod")).unwrap(),
        )],
    );
    assert!(has(&f.inspect(), "ambiguous_metadata"));
    fs::remove_file(f.root.join("mods/case.jar")).unwrap();
    f.fabric("UPPER.JAR", fabric("one_mod"));
    assert!(has(&f.inspect(), "ambiguous_file_name"));
}

#[test]
fn exact_duplicate_zip_metadata_names_are_detected_before_zip_deduplication() {
    let f = Fixture::new();
    let metadata = serde_json::to_vec(&fabric("one_mod")).unwrap();
    f.jar(
        "duplicate.jar",
        &[
            ("fabric.mod.json", &metadata),
            ("xabric.mod.json", &metadata),
        ],
    );
    let path = f.root.join("mods/duplicate.jar");
    let mut bytes = fs::read(&path).unwrap();
    for index in 0..bytes.len().saturating_sub(14) {
        if bytes[index..].starts_with(b"xabric.mod.json") {
            bytes[index] = b'f';
        }
    }
    fs::write(path, bytes).unwrap();
    assert!(has(&f.inspect(), "ambiguous_metadata"));
}

#[test]
fn zip64_and_trailing_data_are_rejected() {
    let f = Fixture::new();
    f.fabric("bad.jar", fabric("some_mod"));
    let path = f.root.join("mods/bad.jar");
    let mut bytes = fs::read(&path).unwrap();
    let end = bytes.len() - 22;
    let mut locator = vec![0; 20];
    locator[..4].copy_from_slice(b"PK\x06\x07");
    bytes.splice(end..end, locator);
    fs::write(&path, bytes).unwrap();
    assert!(has(&f.inspect(), "archive_limit"));
    f.fabric("bad.jar", fabric("some_mod"));
    fs::OpenOptions::new()
        .append(true)
        .open(path)
        .unwrap()
        .write_all(b"extra")
        .unwrap();
    assert!(has(&f.inspect(), "invalid_metadata"));
}

#[cfg(unix)]
#[test]
fn file_directory_and_metadata_symlinks_are_rejected_and_disabled_links_ignored() {
    use std::os::unix::fs::symlink;
    let f = Fixture::new();
    symlink(
        f.root.join("missing"),
        f.root.join("mods/disabled.jar.disabled"),
    )
    .unwrap();
    assert!(f.inspect().is_empty());
    symlink(f.root.join("missing"), f.root.join("mods/active.jar")).unwrap();
    assert!(has(&f.inspect(), "unsafe_mod_file"));
    fs::remove_file(f.root.join("mods/active.jar")).unwrap();
    let mut writer = ZipWriter::new(fs::File::create(f.root.join("mods/link.jar")).unwrap());
    writer
        .add_symlink(
            "fabric.mod.json",
            "outside.json",
            SimpleFileOptions::default(),
        )
        .unwrap();
    writer.finish().unwrap();
    assert!(has(&f.inspect(), "unsafe_metadata"));
    fs::remove_dir_all(f.root.join("mods")).unwrap();
    fs::create_dir(f.root.join("real_mods")).unwrap();
    symlink(f.root.join("real_mods"), f.root.join("mods")).unwrap();
    assert!(has(&f.inspect(), "unsafe_mods_directory"));
}

fn archive_bytes(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let mut writer = ZipWriter::new(std::io::Cursor::new(Vec::new()));
    for (name, bytes) in entries {
        writer
            .start_file(
                *name,
                SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated),
            )
            .unwrap();
        writer.write_all(bytes).unwrap();
    }
    writer.finish().unwrap().into_inner()
}

#[test]
fn dependency_mismatch_has_actual_files_and_unknown_syntax_is_not_compatibility() {
    let f = Fixture::new();
    let mut main = fabric("main");
    main["depends"] = json!({"dependency": ">=2 <3", "unknown": "(>=1)"});
    f.fabric("main.jar", main);
    f.fabric("dep.jar", fabric("dependency"));
    f.fabric("unknown.jar", fabric("unknown"));
    let issues = f.inspect();
    let issue = issues
        .iter()
        .find(|i| i.code == "dependency_version_mismatch")
        .unwrap();
    assert!(issue.message.contains("1.2.3") && issue.message.contains(">=2 <3"));
    assert_eq!(issue.files, ["dep.jar", "main.jar"]);
    assert!(has(&issues, "unknown_version_constraint"));
    assert!(!has(&issues, "missing_dependency"));
    assert_eq!(issues, f.inspect());
}

#[test]
fn quilt_alternative_versions_and_unless_use_real_providers() {
    let mut f = Fixture::new();
    f.instance.loader = "quilt@1".into();
    let mut main = quilt("main");
    main["quilt_loader"]["depends"] = json!([
        [{"id":"absent", "versions":"*"}, {"id":"api", "versions": {"all":[">=2", "<3"]}}],
        {"id":"conditional", "unless":{"id":"api", "versions":">=2"}}
    ]);
    f.quilt("main.jar", main);
    let mut provider = quilt("actual");
    provider["quilt_loader"]["provides"] = json!([{"id":"api", "version":"2.5"}]);
    f.quilt("provider.jar", provider);
    assert!(f.inspect().is_empty(), "{:?}", f.inspect());
}

#[test]
fn nested_fabric_candidates_are_recursive_and_backtrack_without_duplicate_ids() {
    let f = Fixture::new();
    let mut main = fabric("main");
    main["depends"] = json!({"dependency":">=1"});
    main["jars"] = json!([{"file":"jars/new.jar"}, {"file":"jars/old.jar"}]);
    let mut newer = fabric("dependency");
    newer["version"] = json!("2.0");
    newer["depends"] = json!({"base":">=2"});
    let newer = serde_json::to_vec(&newer).unwrap();
    let newer = archive_bytes(&[("fabric.mod.json", &newer)]);
    let mut older = fabric("dependency");
    older["version"] = json!("1.0");
    older["depends"] = json!({"leaf":"~1.2"});
    older["jars"] = json!([{"file":"deep/leaf.jar"}]);
    let leaf = serde_json::to_vec(&fabric("leaf")).unwrap();
    let leaf = archive_bytes(&[("fabric.mod.json", &leaf)]);
    let older = serde_json::to_vec(&older).unwrap();
    let older = archive_bytes(&[("fabric.mod.json", &older), ("deep/leaf.jar", &leaf)]);
    let main = serde_json::to_vec(&main).unwrap();
    f.jar(
        "main.jar",
        &[
            ("fabric.mod.json", &main),
            ("jars/new.jar", &newer),
            ("jars/old.jar", &older),
        ],
    );
    f.fabric("base.jar", fabric("base"));
    assert!(f.inspect().is_empty(), "{:?}", f.inspect());
    let mut root_override = fabric("dependency");
    root_override["version"] = json!("3.0");
    f.fabric("root.jar", root_override);
    assert!(!has(&f.inspect(), "duplicate_mod_id"));
}

#[test]
fn unreadable_unsafe_and_overbudget_nested_jars_never_become_providers() {
    for nested_path in [
        "../evil.jar",
        "a/../evil.jar",
        "a//evil.jar",
        "/evil.jar",
        "C:evil.jar",
        "a\\evil.jar",
    ] {
        let f = Fixture::new();
        let mut main = fabric("main");
        main["jars"] = json!([{"file":nested_path}]);
        f.fabric("main.jar", main);
        assert!(has(&f.inspect(), "unsafe_nested_path"), "{nested_path}");
        assert!(has(&f.inspect(), "inspection_incomplete"));
    }
    let f = Fixture::new();
    let mut main = fabric("main");
    main["jars"] = json!([{"file":"nested.jar"}]);
    main["depends"] = json!({"nested":"*"});
    f.fabric("main.jar", main);
    assert!(has(&f.inspect(), "missing_nested_jar"));
    assert!(!has(&f.inspect(), "missing_dependency"));
    let metadata = serde_json::to_vec(&fabric("nested")).unwrap();
    let bytes = archive_bytes(&[("fabric.mod.json", &metadata)]);
    for mut budget in [
        InspectionBudget {
            expanded: MAX_EXPANDED_BYTES - 1,
            ..Default::default()
        },
        InspectionBudget {
            entries: MAX_TOTAL_ENTRIES,
            ..Default::default()
        },
        InspectionBudget {
            jars: MAX_TOTAL_JARS,
            ..Default::default()
        },
    ] {
        assert!(
            inspect_archive(
                std::io::Cursor::new(bytes.clone()),
                "test.jar",
                "fabric",
                0,
                &mut budget
            )
            .is_err()
        );
    }
    let mut inner = bytes;
    for _ in 0..=MAX_NESTED_DEPTH {
        let mut metadata = fabric("container");
        metadata["jars"] = json!([{"file":"inner.jar"}]);
        let metadata = serde_json::to_vec(&metadata).unwrap();
        inner = archive_bytes(&[("fabric.mod.json", &metadata), ("inner.jar", &inner)]);
    }
    let (_, issues, complete) = inspect_archive(
        std::io::Cursor::new(inner),
        "test.jar",
        "fabric",
        0,
        &mut InspectionBudget::default(),
    )
    .unwrap();
    assert!(!complete && has(&issues, "archive_limit"));
}

#[test]
fn quilt_nested_paths_use_quilt_schema_and_forge_does_not_load_fabric_nests() {
    let mut f = Fixture::new();
    f.instance.loader = "quilt@1".into();
    let mut main = quilt("main");
    main["quilt_loader"]["jars"] = json!(["nested.jar"]);
    main["quilt_loader"]["depends"] = json!([{"id":"dependency", "versions":">=1"}]);
    let child = serde_json::to_vec(&fabric("dependency")).unwrap();
    let child = archive_bytes(&[("fabric.mod.json", &child)]);
    let main = serde_json::to_vec(&main).unwrap();
    f.jar(
        "main.jar",
        &[("quilt.mod.json", &main), ("nested.jar", &child)],
    );
    assert!(f.inspect().is_empty(), "{:?}", f.inspect());
    f.instance.loader = "forge@1".into();
    assert!(has(&f.inspect(), "incompatible_loader"));
    assert!(!has(&f.inspect(), "missing_nested_jar"));
}

#[test]
fn forge_manifest_versions_and_maven_unions_distinguish_mismatch_from_unknown() {
    let mut f = Fixture::new();
    f.instance.loader = "forge@1".into();
    let main = b"[[mods]]\nmodId='main'\nversion='1'\n[[dependencies.main]]\nmodId='api'\nmandatory=true\nversionRange='(,1.0],[2,)'";
    let api = b"[[mods]]\nmodId='api'\nversion='${file.jarVersion}'";
    f.jar("main.jar", &[("META-INF/mods.toml", main)]);
    f.jar(
        "api.jar",
        &[
            ("META-INF/mods.toml", api),
            (
                "META-INF/MANIFEST.MF",
                b"Manifest-Version: 1.0\r\nImplementation-Version: 1.\r\n 6\r\n\r\n",
            ),
        ],
    );
    let issues = f.inspect();
    assert!(has(&issues, "dependency_version_mismatch"));
    assert!(!has(&issues, "missing_dependency"));
    f.jar("api.jar", &[("META-INF/mods.toml", api)]);
    assert!(has(&f.inspect(), "unknown_version_constraint"));
    assert!(!has(&f.inspect(), "dependency_version_mismatch"));
}

#[test]
fn forge_jarjar_picks_one_coordinate_version_across_containers() {
    let mut f = Fixture::new();
    f.instance.loader = "forge@1".into();
    for (file, id, version) in [("a.jar", "main_a", "1.2"), ("b.jar", "main_b", "1.5")] {
        let main = format!(
            "[[mods]]\nmodId='{id}'\nversion='1'\n[[dependencies.{id}]]\nmodId='api'\nmandatory=true\nversionRange='[1.4,)'"
        );
        let api = format!("[[mods]]\nmodId='api'\nversion='{version}'");
        let child = archive_bytes(&[("META-INF/mods.toml", api.as_bytes())]);
        let metadata = serde_json::to_vec(&json!({"jars":[{"identifier":{"group":"test","artifact":"api"},"version":{"range":"[1,2)","artifactVersion":version},"path":"lib/api.jar"}]})).unwrap();
        f.jar(
            file,
            &[
                ("META-INF/mods.toml", main.as_bytes()),
                ("META-INF/jarjar/metadata.json", &metadata),
                ("lib/api.jar", &child),
            ],
        );
    }
    assert!(f.inspect().is_empty(), "{:?}", f.inspect());
}

#[test]
fn forge_jarjar_selects_coordinates_not_mod_ids_or_mod_versions() {
    let mut f = Fixture::new();
    f.instance.loader = "forge@1".into();
    let main = b"[[mods]]\nmodId='main'\nversion='1'\n[[dependencies.main]]\nmodId='api'\nmandatory=true\nversionRange='[1.5,2)'";
    let child = archive_bytes(&[(
        "META-INF/mods.toml",
        b"[[mods]]\nmodId='api'\nversion='1.6'",
    )]);
    let descriptor = json!({"jars":[{"identifier":{"group":"example.group","artifact":"library"},"version":{"range":"[9,)","artifactVersion":"9.1"},"path":"META-INF/jarjar/library.jar"}]});
    let descriptor = serde_json::to_vec(&descriptor).unwrap();
    f.jar(
        "main.jar",
        &[
            ("META-INF/mods.toml", main),
            ("META-INF/jarjar/metadata.json", &descriptor),
            ("META-INF/jarjar/library.jar", &child),
        ],
    );
    assert!(f.inspect().is_empty(), "{:?}", f.inspect());
    let mut version = fabric("unrelated");
    version["version"] = json!("1");
    f.fabric("wrong-loader.jar", version);
    assert!(has(&f.inspect(), "incompatible_loader"));
}
