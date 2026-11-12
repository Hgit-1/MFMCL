use super::*;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};

fn runtime(major: u32) -> JavaRuntimeInfo {
    JavaRuntimeInfo {
        path: format!("fixture/java-{major}"),
        major_version: major,
        version_text: format!("openjdk version \"{major}.0.2\""),
    }
}
fn requirement(min: u32, max: Option<u32>) -> JavaRequirement {
    JavaRequirement {
        min_major: min,
        max_major: max,
        component: "java-runtime-delta".into(),
    }
}
fn instance(version: &str, loader: &str) -> InstanceRecord {
    serde_json::from_value(serde_json::json!({
        "id":"fixture", "name":"fixture", "gameVersion":version, "loader":loader,
        "instanceDir":"fixture/instance", "gameDir":"fixture/game", "isolated":true,
        "state":"ready", "createdAt":""
    }))
    .unwrap()
}
fn metadata(major: u32) -> GameVersionMetadata {
    serde_json::from_value(serde_json::json!({"id":"snapshot", "javaVersion":{"majorVersion":major,"component":"java-runtime-delta"}})).unwrap()
}
fn executable() -> &'static str {
    if cfg!(windows) {
        "bin/java.exe"
    } else {
        "bin/java"
    }
}
fn manifest(url: &str, content: &[u8]) -> RuntimeManifest {
    serde_json::from_value(serde_json::json!({"files": {
        executable(): {"type":"file", "executable":true, "downloads":{"raw":{
            "url":url, "size":content.len(), "sha1":hex::encode(Sha1::digest(content))
        }}}
    }}))
    .unwrap()
}
fn temp() -> PathBuf {
    std::env::temp_dir().join(format!("mfmcl-java-test-{}", Uuid::new_v4()))
}

async fn server(
    body: Vec<u8>,
    fail_first: bool,
) -> (String, Arc<AtomicUsize>, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let count = calls.clone();
    let handle = tokio::spawn(async move {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                break;
            };
            let mut request = [0; 2048];
            let _ = stream.read(&mut request).await;
            let call = count.fetch_add(1, Ordering::SeqCst);
            let (status, bytes) = if fail_first && call == 0 {
                ("503 Service Unavailable", &b"retry"[..])
            } else {
                ("200 OK", body.as_slice())
            };
            let header = format!(
                "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                bytes.len()
            );
            let _ = stream.write_all(header.as_bytes()).await;
            let _ = stream.write_all(bytes).await;
        }
    });
    (format!("http://{address}/raw"), calls, handle)
}

#[test]
fn java_rules_and_priority_survive_reopen_and_instance_deletion_cascades() {
    let root = temp();
    let database = root.join("launcher.sqlite");
    let store = crate::LauncherStore::open(&database).unwrap();
    let paths = vec![
        root.join("jdk21/bin/java").display().to_string(),
        root.join("jdk17/bin/java").display().to_string(),
    ];
    let mut ids = Vec::new();
    for (index, rule) in [
        JavaRule::default(),
        JavaRule {
            mode: JavaSelectionMode::Range,
            range: "17-21".into(),
            ..Default::default()
        },
        JavaRule {
            mode: JavaSelectionMode::Folder,
            folder: root.join("bundled").display().to_string(),
            ..Default::default()
        },
        JavaRule {
            mode: JavaSelectionMode::Forced,
            path: paths[0].clone(),
            ..Default::default()
        },
    ]
    .into_iter()
    .enumerate()
    {
        let instance = store
            .create_instance(
                &crate::CreateInstanceInput {
                    name: format!("Java {index}"),
                    game_version: "1.21.4".into(),
                },
                &root,
            )
            .unwrap();
        store
            .save_java_preferences(
                &JavaPreferencesUpdate {
                    ordered_paths: Some(paths.clone()),
                    instance_id: Some(instance.id.clone()),
                    rule: Some(rule.clone()),
                },
                None,
            )
            .unwrap();
        assert_eq!(
            store.load_java_preferences().unwrap().instance_rules[&instance.id],
            rule
        );
        ids.push(instance.id);
    }
    drop(store);
    let reopened = crate::LauncherStore::open(&database).unwrap();
    let preferences = reopened.load_java_preferences().unwrap();
    assert_eq!(preferences.ordered_paths, paths);
    assert_eq!(preferences.instance_rules.len(), 4);
    let request = reopened
        .resolve_java_selection(&ids[1], None, None, None)
        .unwrap();
    assert_eq!(request.mode, JavaSelectionMode::Range);
    assert_eq!(request.range, "17-21");
    assert_eq!(request.ordered_paths, Some(paths));
    reopened.delete_instance(&ids[1]).unwrap();
    assert!(
        !reopened
            .load_java_preferences()
            .unwrap()
            .instance_rules
            .contains_key(&ids[1])
    );
    assert!(
        reopened
            .save_java_preferences(
                &JavaPreferencesUpdate {
                    instance_id: Some(ids[1].clone()),
                    rule: Some(JavaRule::default()),
                    ..Default::default()
                },
                None
            )
            .is_err()
    );
    drop(reopened);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn java_preference_validation_rejects_invalid_payloads_without_changing_confirmed_data() {
    let root = temp();
    let store = crate::LauncherStore::open(":memory:").unwrap();
    let instance = store
        .create_instance(
            &crate::CreateInstanceInput {
                name: "Validation".into(),
                game_version: "1.21.4".into(),
            },
            &root,
        )
        .unwrap();
    let path = root.join("java").display().to_string();
    let confirmed = JavaRule {
        mode: JavaSelectionMode::Range,
        range: "21".into(),
        ..Default::default()
    };
    store
        .save_java_preferences(
            &JavaPreferencesUpdate {
                instance_id: Some(instance.id.clone()),
                rule: Some(confirmed.clone()),
                ordered_paths: Some(vec![path.clone()]),
            },
            None,
        )
        .unwrap();
    let settings = crate::LauncherSettings {
        memory_mb: 4096,
        ..Default::default()
    };
    for rule in [
        JavaRule {
            mode: JavaSelectionMode::Range,
            range: "[21,17]".into(),
            ..Default::default()
        },
        JavaRule {
            mode: JavaSelectionMode::Range,
            range: "x".repeat(257),
            ..Default::default()
        },
        JavaRule {
            mode: JavaSelectionMode::Folder,
            folder: "relative/path".into(),
            ..Default::default()
        },
        JavaRule {
            mode: JavaSelectionMode::Forced,
            path: "".into(),
            ..Default::default()
        },
        JavaRule {
            mode: JavaSelectionMode::Forced,
            path: format!("{path}\0"),
            ..Default::default()
        },
    ] {
        assert!(
            store
                .save_java_preferences(
                    &JavaPreferencesUpdate {
                        instance_id: Some(instance.id.clone()),
                        rule: Some(rule),
                        ordered_paths: Some(vec![]),
                    },
                    Some(&settings)
                )
                .is_err()
        );
    }
    for paths in [
        vec![path.clone(), path.clone()],
        vec![String::new()],
        vec!["java".into()],
        vec![path.clone(); 257],
    ] {
        assert!(
            store
                .save_java_preferences(
                    &JavaPreferencesUpdate {
                        ordered_paths: Some(paths),
                        ..Default::default()
                    },
                    None
                )
                .is_err()
        );
    }
    assert!(
        store
            .save_java_preferences(
                &JavaPreferencesUpdate {
                    instance_id: Some("unknown".into()),
                    rule: Some(JavaRule::default()),
                    ordered_paths: Some(vec![]),
                },
                Some(&settings)
            )
            .is_err()
    );
    assert!(
        store
            .save_java_preferences(
                &JavaPreferencesUpdate {
                    rule: Some(JavaRule::default()),
                    ..Default::default()
                },
                None
            )
            .is_err()
    );
    let loaded = store.load_java_preferences().unwrap();
    assert_eq!(loaded.ordered_paths, vec![path]);
    assert_eq!(loaded.instance_rules[&instance.id], confirmed);
    assert_eq!(
        store.load_settings().unwrap().memory_mb,
        crate::LauncherSettings::default().memory_mb
    );
    assert!(serde_json::from_str::<JavaRule>(r#"{"mode":"bogus"}"#).is_err());
    assert!(serde_json::from_str::<JavaRule>(r#"{"mode":"auto","unexpected":true}"#).is_err());
}

#[test]
fn legacy_java_migration_is_atomic_one_time_and_never_overwrites_backend_choices() {
    let root = temp();
    let store = crate::LauncherStore::open(":memory:").unwrap();
    let make = |name: &str| {
        store
            .create_instance(
                &crate::CreateInstanceInput {
                    name: name.into(),
                    game_version: "1.21.4".into(),
                },
                &root,
            )
            .unwrap()
    };
    let chosen = make("Chosen");
    let legacy_instance = make("Legacy");
    store
        .save_java_preferences(
            &JavaPreferencesUpdate {
                instance_id: Some(chosen.id.clone()),
                rule: Some(JavaRule::default()),
                ordered_paths: Some(vec![]),
            },
            None,
        )
        .unwrap();
    let rule = JavaRule {
        mode: JavaSelectionMode::Range,
        range: "17+".into(),
        ..Default::default()
    };
    let legacy = JavaPreferences {
        ordered_paths: vec![root.join("java").display().to_string()],
        instance_rules: BTreeMap::from([
            (chosen.id.clone(), rule.clone()),
            (legacy_instance.id.clone(), rule.clone()),
            (
                "no-such-instance".into(),
                JavaRule {
                    mode: JavaSelectionMode::Forced,
                    ..Default::default()
                },
            ),
        ]),
        ..Default::default()
    };
    let migrated = store.migrate_legacy_java_preferences(&legacy).unwrap();
    assert!(migrated.legacy_migrated);
    assert!(migrated.ordered_paths.is_empty());
    assert_eq!(migrated.instance_rules[&chosen.id], JavaRule::default());
    assert_eq!(migrated.instance_rules[&legacy_instance.id], rule);
    assert!(!migrated.instance_rules.contains_key("no-such-instance"));
    let repeated = store
        .migrate_legacy_java_preferences(&JavaPreferences {
            ordered_paths: vec!["invalid".into()],
            ..Default::default()
        })
        .unwrap();
    assert!(repeated.ordered_paths.is_empty());
    assert_eq!(repeated.instance_rules, migrated.instance_rules);

    let fresh = crate::LauncherStore::open(":memory:").unwrap();
    let instance = fresh
        .create_instance(
            &crate::CreateInstanceInput {
                name: "Invalid legacy".into(),
                game_version: "1.21.4".into(),
            },
            &root,
        )
        .unwrap();
    assert!(
        fresh
            .migrate_legacy_java_preferences(&JavaPreferences {
                ordered_paths: legacy.ordered_paths.clone(),
                instance_rules: BTreeMap::from([(
                    instance.id,
                    JavaRule {
                        mode: JavaSelectionMode::Range,
                        range: "invalid".into(),
                        ..Default::default()
                    }
                )]),
                ..Default::default()
            })
            .is_err()
    );
    let untouched = fresh.load_java_preferences().unwrap();
    assert!(!untouched.legacy_migrated);
    assert!(untouched.ordered_paths.is_empty());
    assert!(untouched.instance_rules.is_empty());
    assert!(
        fresh
            .migrate_legacy_java_preferences(&JavaPreferences::default())
            .unwrap()
            .legacy_migrated
    );
}

#[test]
fn selector_precedence_distinguishes_inherited_rule_explicit_auto_and_empty_overrides() {
    let root = temp();
    let preferred = root.join("global/java");
    let priorities = vec!["candidate-21".into(), "candidate-17".into()];
    for rule in [
        JavaRule {
            mode: JavaSelectionMode::Range,
            range: "17-21".into(),
            ..Default::default()
        },
        JavaRule {
            mode: JavaSelectionMode::Folder,
            folder: root.display().to_string(),
            ..Default::default()
        },
        JavaRule {
            mode: JavaSelectionMode::Forced,
            path: root.join("forced/java").display().to_string(),
            ..Default::default()
        },
    ] {
        let preferences = JavaPreferences {
            ordered_paths: priorities.clone(),
            instance_rules: BTreeMap::from([("fixture".into(), rule.clone())]),
            ..Default::default()
        };
        let inherited =
            resolve_java_selection(&preferences, "fixture", None, None, Some(&preferred));
        assert_eq!(inherited.mode, rule.mode);
        assert_eq!(inherited.range, rule.range);
        assert_eq!(inherited.folder, rule.folder);
        assert_eq!(inherited.path, rule.path);
        assert_eq!(inherited.ordered_paths, Some(priorities.clone()));
        assert_eq!(
            preferred_java_paths(&inherited),
            vec![preferred.to_str().unwrap(), "candidate-21", "candidate-17"]
        );

        let explicit_auto: JavaSelectionRequest =
            serde_json::from_str(r#"{"mode":"auto"}"#).unwrap();
        let automatic = resolve_java_selection(
            &preferences,
            "fixture",
            Some(explicit_auto),
            Some("ignored-legacy".into()),
            Some(&preferred),
        );
        assert_eq!(automatic.mode, JavaSelectionMode::Auto);
        assert!(
            automatic.path.is_empty() && automatic.range.is_empty() && automatic.folder.is_empty()
        );
        assert_eq!(automatic.ordered_paths, Some(priorities.clone()));

        let cleared = resolve_java_selection(
            &preferences,
            "fixture",
            Some(JavaSelectionRequest {
                ordered_paths: Some(vec![]),
                global_path: Some(String::new()),
                ..Default::default()
            }),
            None,
            Some(&preferred),
        );
        assert!(preferred_java_paths(&cleared).is_empty());
        let forced = resolve_java_selection(
            &preferences,
            "fixture",
            None,
            Some(String::new()),
            Some(&preferred),
        );
        assert_eq!(forced.mode, JavaSelectionMode::Forced);
        assert!(
            select_java(
                &root,
                &instance("1.21.4", "vanilla"),
                &metadata(21),
                &forced
            )
            .is_err()
        );
    }
}

#[test]
fn official_install_future_can_run_on_a_multithreaded_runtime() {
    fn assert_send<T: Send>(_: T) {}
    let root = temp();
    let requirement = requirement(21, None);
    let control = DownloadControl::new();
    assert_send(install_official_java(&root, &requirement, &control));
    assert_send(ensure_java(
        &root,
        &instance("1.21.4", "vanilla"),
        &metadata(21),
        &JavaSelectionRequest::default(),
        &control,
    ));
}

#[test]
fn auto_walks_past_valid_but_incompatible_candidates_and_keeps_priority() {
    let selected = choose_candidate(
        vec![runtime(8), runtime(25), runtime(21), runtime(17)],
        &requirement(17, Some(25)),
        None,
    )
    .unwrap();
    assert_eq!(selected.major_version, 21);
    assert!(
        choose_candidate(
            vec![runtime(8), runtime(25)],
            &requirement(17, Some(25)),
            None
        )
        .is_none()
    );
}
#[test]
fn metadata_wins_for_snapshots_and_loader_caps_apply_to_every_mode() {
    let req = java_requirement(&instance("26w01a", "fabric@0.16.0"), &metadata(21));
    assert_eq!(req.min_major, 21);
    assert_eq!(req.max_major, Some(25));
    assert!(!req.accepts(25));
    let req = java_requirement(&instance("1.16.5", "forge@36.2.25"), &metadata(8));
    assert_eq!(req.max_major, Some(9));
}
#[test]
fn range_intersects_game_requirements_and_parses_legacy_updates() {
    let range = parse_range("17-21").unwrap();
    assert_eq!(
        choose_candidate(
            vec![runtime(17), runtime(21)],
            &requirement(21, None),
            Some(&range)
        )
        .unwrap()
        .major_version,
        21
    );
    let old = JavaRuntimeInfo {
        path: "fixture".into(),
        major_version: 8,
        version_text: "java version \"1.8.0_141\"".into(),
    };
    assert!(
        parse_range("[8.0.81,8.0.141]")
            .unwrap()
            .contains(runtime_version(&old))
    );
    for text in ["", "bogus", "[21,17]", "(21,21)", "[8,9,10]", "4294967296"] {
        assert!(parse_range(text).is_err(), "{text}");
    }
    for text in ["21", "21+", ">=21", "≥21", "17~21", "[21,)"] {
        assert!(parse_range(text).is_ok(), "{text}");
    }
}
#[test]
fn explicit_invalid_paths_folders_and_ranges_do_not_fall_back() {
    let root = temp();
    for request in [
        JavaSelectionRequest {
            mode: JavaSelectionMode::Forced,
            path: root.join("missing.exe").display().to_string(),
            ..Default::default()
        },
        JavaSelectionRequest {
            mode: JavaSelectionMode::Folder,
            folder: root.display().to_string(),
            ..Default::default()
        },
        JavaSelectionRequest {
            mode: JavaSelectionMode::Range,
            range: "invalid".into(),
            ..Default::default()
        },
    ] {
        assert!(
            select_java(
                &root,
                &instance("1.21.4", "vanilla"),
                &metadata(21),
                &request
            )
            .is_err()
        );
    }
}
#[test]
fn folder_search_finds_nested_runtimes_in_stable_order() {
    let root = temp();
    for directory in ["z/runtime/bin", "a/jdk/bin"] {
        let path = root.join(directory);
        std::fs::create_dir_all(&path).unwrap();
        std::fs::write(
            path.join(if cfg!(windows) { "java.exe" } else { "java" }),
            b"fixture",
        )
        .unwrap();
    }
    let paths = folder_java_candidates(&root).unwrap();
    assert_eq!(paths.len(), 2);
    assert!(paths[0].to_string_lossy().contains("jdk"));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn mojang_index_version_names_include_legacy_update_notation() {
    assert_eq!(index_runtime_major("8u51-cacert462b08"), Some(8));
    assert_eq!(index_runtime_major("8u202"), Some(8));
    assert_eq!(index_runtime_major("1.8.0_432"), Some(8));
    assert_eq!(index_runtime_major("21.0.7"), Some(21));
    assert_eq!(index_runtime_major("25.0.1"), Some(25));
}

#[test]
fn platform_matching_does_not_silently_substitute_architecture() {
    assert_eq!(mojang_platform("windows", "aarch64"), Some("windows-arm64"));
    assert_eq!(mojang_platform("macos", "aarch64"), Some("mac-os-arm64"));
    assert_eq!(mojang_platform("linux", "riscv64"), None);
}
#[test]
fn paths_urls_and_link_parents_are_rejected_before_writes() {
    for path in [
        "../java",
        "/java",
        "C:/java",
        "bin\\java",
        "bin/../java",
        "NUL.dll",
        "bin/java.",
        "bin/java:ads",
        "bin//java",
        "bin/./java",
    ] {
        assert!(safe_relative(path).is_err(), "{path}");
    }
    for url in [
        "https://evil.example/java",
        "http://piston-data.mojang.com/java",
        "https://piston-data.mojang.com:444/java",
        "https://user@piston-data.mojang.com/java",
    ] {
        assert!(!safe_url(&Url::parse(url).unwrap()), "{url}");
    }
    assert!(link_destination("bin/link", "../../escape").is_err());
    assert_eq!(
        link_destination("bin/link", "../lib/java").unwrap(),
        "lib/java"
    );
    let mut value = manifest("https://piston-data.mojang.com/java", b"ok");
    value.files.insert(
        "bin".into(),
        RuntimeFile::Link {
            target: "lib".into(),
        },
    );
    assert!(validate_manifest(&value).is_err());
}
#[tokio::test]
async fn manifest_hash_and_length_are_both_required() {
    let bytes = b"{\"files\":{}}".to_vec();
    let (url, _, handle) = server(bytes.clone(), false).await;
    let mut download = RuntimeDownload {
        url,
        size: bytes.len() as u64,
        sha1: hex::encode(Sha1::digest(&bytes)),
    };
    assert_eq!(
        fetch_pinned(
            &runtime_client().unwrap(),
            &download,
            &DownloadControl::new()
        )
        .await
        .unwrap(),
        bytes
    );
    download.sha1 = "0".repeat(40);
    assert!(
        fetch_pinned(
            &runtime_client().unwrap(),
            &download,
            &DownloadControl::new()
        )
        .await
        .is_err()
    );
    download.size += 1;
    assert!(
        fetch_pinned(
            &runtime_client().unwrap(),
            &download,
            &DownloadControl::new()
        )
        .await
        .is_err()
    );
    handle.abort();
}
#[tokio::test]
async fn installation_retries_verifies_and_publishes_only_a_complete_tree() {
    let content = b"tiny local Java fixture";
    let (url, calls, handle) = server(content.to_vec(), true).await;
    let root = temp();
    let path = install_manifest(
        &runtime_client().unwrap(),
        &root,
        manifest(&url, content),
        "fixture",
        None,
        &DownloadControl::new(),
        2,
    )
    .await
    .unwrap();
    assert_eq!(fs::read(&path).await.unwrap(), content);
    assert!(calls.load(Ordering::SeqCst) >= 2);
    assert_eq!(
        fs::read_dir(&root)
            .await
            .unwrap()
            .next_entry()
            .await
            .unwrap()
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with("mojang-"),
        true
    );
    assert!(
        path.parent()
            .unwrap()
            .parent()
            .unwrap()
            .join(".mfmcl-complete")
            .is_file()
    );
    fs::remove_dir_all(root).await.unwrap();
    handle.abort();
}
#[tokio::test]
async fn bad_raw_hash_or_short_body_never_publishes() {
    let (url, _, handle) = server(b"wrong bytes".to_vec(), false).await;
    for content in [&b"same length"[..], &b"expected much longer fixture"[..]] {
        let root = temp();
        assert!(
            install_manifest(
                &runtime_client().unwrap(),
                &root,
                manifest(&url, content),
                "fixture",
                None,
                &DownloadControl::new(),
                2,
            )
            .await
            .is_err()
        );
        assert!(
            fs::read_dir(&root)
                .await
                .unwrap()
                .next_entry()
                .await
                .unwrap()
                .is_none()
        );
        fs::remove_dir_all(root).await.unwrap();
    }
    handle.abort();
}
#[tokio::test]
async fn cancellation_removes_partial_tree_and_never_publishes() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/raw", listener.local_addr().unwrap());
    let control = DownloadControl::new();
    let cancel = control.clone();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut request = [0; 2048];
        let _ = stream.read(&mut request).await;
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\n\r\npartial")
            .await
            .unwrap();
        cancel.cancel();
        tokio::time::sleep(Duration::from_secs(1)).await;
    });
    let root = temp();
    assert!(
        install_manifest(
            &runtime_client().unwrap(),
            &root,
            manifest(&url, &[b'x'; 100]),
            "fixture",
            None,
            &control,
            2,
        )
        .await
        .is_err()
    );
    assert!(
        fs::read_dir(&root)
            .await
            .unwrap()
            .next_entry()
            .await
            .unwrap()
            .is_none()
    );
    fs::remove_dir_all(root).await.unwrap();
    server.abort();
}
