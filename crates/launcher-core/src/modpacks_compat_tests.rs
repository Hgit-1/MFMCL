use super::*;
use crate::{CreateInstanceInput, LauncherStore};

struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("mfmcl-pack-compat-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        Self(root)
    }
    fn pack(&self, entries: &[(&str, &[u8])]) -> PathBuf {
        let path = self.0.join(format!("{}.zip", uuid::Uuid::new_v4()));
        let mut zip = ZipWriter::new(File::create(&path).unwrap());
        for (name, bytes) in entries {
            if name.ends_with('/') {
                zip.add_directory(*name, SimpleFileOptions::default())
                    .unwrap();
            } else {
                zip.start_file(*name, SimpleFileOptions::default()).unwrap();
                zip.write_all(bytes).unwrap();
            }
        }
        zip.finish().unwrap();
        path
    }
    fn target(&self, version: &str) -> InstanceRecord {
        let store = LauncherStore::open(":memory:").unwrap();
        store
            .create_instance(
                &CreateInstanceInput {
                    name: "Imported".into(),
                    game_version: version.into(),
                },
                &self.0,
            )
            .unwrap()
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn engine() -> DownloadEngine {
    DownloadEngine::new(download_engine::DownloadSettings::default()).unwrap()
}
fn index() -> PackIndex {
    PackIndex {
        format_version: 1,
        game: "minecraft".into(),
        version_id: "1".into(),
        name: "Wrapped".into(),
        files: vec![],
        dependencies: std::collections::HashMap::from([("minecraft".into(), "1.21.4".into())]),
    }
}

#[tokio::test]
async fn standard_overrides_accept_game_options_and_bundled_worlds() {
    let temp = Temp::new();
    let bytes = serde_json::to_vec(&index()).unwrap();
    let path = temp.pack(&[
        ("minecraft/modrinth.index.json", &bytes),
        ("minecraft/overrides/options.txt", b"music:0.5"),
        ("minecraft/overrides/saves/World/level.dat", b"world"),
    ]);
    let target = temp.target("1.21.4");
    import_pack(&path, &target, &engine(), None).await.unwrap();
    assert_eq!(
        fs::read(target.game_dir.join("options.txt")).unwrap(),
        b"music:0.5"
    );
    assert_eq!(
        fs::read(target.game_dir.join("saves/World/level.dat")).unwrap(),
        b"world"
    );
}

#[tokio::test]
async fn nested_modrinth_import_uses_real_default_store_creation() {
    for prefix in ["Pack/", "Export/Folder/Pack/"] {
        let temp = Temp::new();
        let bytes = serde_json::to_vec(&index()).unwrap();
        let path = temp.pack(&[
            (prefix, b""),
            (&format!("{prefix}modrinth.index.json"), &bytes),
            (&format!("{prefix}overrides/config/test.txt"), b"config"),
        ]);
        let target = temp.target("1.21.4");
        assert!(target.isolated);
        import_pack(&path, &target, &engine(), None).await.unwrap();
        assert_eq!(
            fs::read(target.game_dir.join("config/test.txt")).unwrap(),
            b"config"
        );
    }
}

#[tokio::test]
async fn all_four_loaders_roundtrip_with_modrinth_dependency_keys() {
    for (kind, key, version) in [
        ("fabric", "fabric-loader", "0.16.10"),
        ("quilt", "quilt-loader", "0.27.1"),
        ("forge", "forge", "54.0.0"),
        ("neoforge", "neoforge", "21.4.1"),
    ] {
        let temp = Temp::new();
        let mut target = temp.target("1.21.4");
        fs::create_dir_all(&target.game_dir).unwrap();
        target.loader = format!("{kind}@{version}");
        let path = export_pack(&target, &temp.0).unwrap();
        let prepared = prepare_pack_ready(&path).unwrap();
        assert_eq!(prepared.preview().loader, target.loader);
        assert_eq!(prepared.index.dependencies.get(key).unwrap(), version);
        let store = LauncherStore::open(":memory:").unwrap();
        let fresh = store
            .create_instance(
                &CreateInstanceInput {
                    name: "Fresh".into(),
                    game_version: "1.21.4".into(),
                },
                &temp.0,
            )
            .unwrap();
        import_prepared_pack(prepared, &fresh, &engine(), None)
            .await
            .unwrap();
        let saved: PackIndex = serde_json::from_slice(
            &fs::read(fresh.instance_dir.join("modrinth.index.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(saved.dependencies[key], version);
    }
    let mut invalid = index();
    invalid.dependencies.insert("forge".into(), "54.0.0".into());
    invalid
        .dependencies
        .insert("neoforge".into(), "21.4.1".into());
    assert!(validate_index(&invalid).is_err());
    invalid.dependencies.remove("forge");
    invalid
        .dependencies
        .insert("neoforge".into(), "../escape".into());
    assert!(validate_index(&invalid).is_err());
}

#[tokio::test]
async fn nested_curseforge_custom_override_directory_imports_after_lookup() {
    for loader in ["forge-54.0.0", "neoforge-21.4.1"] {
        let temp = Temp::new();
        let manifest = serde_json::json!({"minecraft":{"version":"1.21.4","modLoaders":[{"id":loader,"primary":true}]},"name":"CF","version":"1","overrides":"custom/data","files":[]});
        let bytes = serde_json::to_vec(&manifest).unwrap();
        let path = temp.pack(&[
            ("outer/inner/manifest.json", &bytes),
            ("outer/inner/custom/data/config/cf.txt", b"cf"),
            ("outer/inner/modlist.html", b"list"),
        ]);
        let PackPreparation::NeedsOnlineLookup { manifest, .. } = prepare_pack(&path).unwrap()
        else {
            panic!("CF requires lookup")
        };
        let expected_loader = loader.replacen('-', "@", 1);
        assert_eq!(manifest.to_preview().loader, expected_loader);
        let prepared = prepare_from_completed_index(&path, manifest.to_pack_index(vec![])).unwrap();
        assert_eq!(prepared.preview().loader, expected_loader);
        let target = temp.target("1.21.4");
        import_prepared_pack(prepared, &target, &engine(), None)
            .await
            .unwrap();
        assert_eq!(
            fs::read(target.game_dir.join("config/cf.txt")).unwrap(),
            b"cf"
        );
    }
}

#[tokio::test]
async fn standard_multimc_existing_instance_archive_copies_game_data_not_launch_commands() {
    for game in ["minecraft", ".minecraft"] {
        let temp = Temp::new();
        let components = br#"{"formatVersion":1,"components":[{"uid":"net.minecraft","version":"1.21.4"},{"uid":"net.neoforged","version":"21.4.1"}]}"#;
        let path = temp.pack(&[
            ("outer/existing/mmc-pack.json", components),
            (
                "outer/existing/instance.cfg",
                b"[General]\nInstanceType=OneSix\nname=Existing\n",
            ),
            (&format!("outer/existing/{game}/config/mod.json"), b"{}"),
            (
                &format!("outer/existing/{game}/saves/World/level.dat"),
                b"world",
            ),
            (&format!("outer/existing/{game}/options.txt"), b"music:0.5"),
        ]);
        let prepared = prepare_pack_ready(&path).unwrap();
        assert_eq!(prepared.preview().loader, "neoforge@21.4.1");
        let target = temp.target("1.21.4");
        import_prepared_pack(prepared, &target, &engine(), None)
            .await
            .unwrap();
        assert_eq!(
            fs::read(target.game_dir.join("saves/World/level.dat")).unwrap(),
            b"world"
        );
        assert_eq!(
            fs::read(target.game_dir.join("options.txt")).unwrap(),
            b"music:0.5"
        );
        assert!(!target.instance_dir.join("instance.cfg").exists());
    }
}

#[test]
fn unsupported_multimc_components_patches_and_malformed_hmcl_are_not_fake_successes() {
    let temp = Temp::new();
    let components =
        br#"{"formatVersion":1,"components":[{"uid":"net.minecraft","version":"1.21.4"}]}"#;
    for (extra, data) in [
        ("patches/net.minecraft.json", b"{}".as_slice()),
        ("jarmods/a.jar", b"x".as_slice()),
    ] {
        let path = temp.pack(&[
            ("mmc-pack.json", components),
            ("instance.cfg", b"InstanceType=OneSix"),
            (extra, data),
        ]);
        assert!(prepare_pack(&path).is_err());
    }
    let unsupported =
        br#"{"formatVersion":1,"components":[{"uid":"net.minecraft","version":"1.21.4"},{"uid":"example.untrusted.component","version":"1"}]}"#;
    let path = temp.pack(&[
        ("mmc-pack.json", unsupported),
        ("instance.cfg", b"InstanceType=OneSix"),
    ]);
    assert!(prepare_pack(&path).is_err());
    let path = temp.pack(&[("modpack.json", b"{}"), ("minecraft/pack.json", b"{}")]);
    let error = match prepare_pack(&path) {
        Err(error) => error,
        Ok(_) => panic!("unsupported HMCL must be explicit"),
    };
    assert!(error.contains("HMCL"));
}

#[test]
fn nested_roots_still_reject_ambiguous_paths_and_bad_loaders() {
    let temp = Temp::new();
    let bytes = serde_json::to_vec(&index()).unwrap();
    for entries in [
        vec![
            ("one/modrinth.index.json", bytes.as_slice()),
            ("two/modrinth.index.json", bytes.as_slice()),
        ],
        vec![
            ("pack/modrinth.index.json", bytes.as_slice()),
            ("pack/overrides/config/../../evil", b"x"),
        ],
        vec![
            ("pack/modrinth.index.json", bytes.as_slice()),
            ("pack/overrides/config/A", b"x"),
            ("pack/overrides/config/a", b"x"),
        ],
    ] {
        assert!(prepare_pack(&temp.pack(&entries)).is_err());
    }
    let manifest = serde_json::json!({"minecraft":{"version":"1.21.4","modLoaders":[{"id":"unknown-1","primary":true}]},"name":"Bad"});
    assert!(
        prepare_pack(&temp.pack(&[("manifest.json", &serde_json::to_vec(&manifest).unwrap())]))
            .is_err()
    );
}

#[test]
fn default_store_restore_roundtrips_forge_and_neoforge_without_shared_writes() {
    let temp = Temp::new();
    let store = LauncherStore::open(":memory:").unwrap();
    for loader in ["forge@54.0.0", "neoforge@21.4.1"] {
        let source = store
            .create_instance(
                &CreateInstanceInput {
                    name: "Source".into(),
                    game_version: "1.21.4".into(),
                },
                &temp.0,
            )
            .unwrap();
        let mut backed_up = source.clone();
        backed_up.loader = loader.into();
        fs::create_dir_all(source.game_dir.join("saves/World")).unwrap();
        fs::write(source.game_dir.join("saves/World/level.dat"), b"world").unwrap();
        let path = crate::backups::create_backup(&backed_up, &temp.0).unwrap();
        let preview = crate::backups::preview_backup(&path).unwrap();
        let fresh = store
            .create_instance(
                &CreateInstanceInput {
                    name: "Restore".into(),
                    game_version: preview.minecraft.clone(),
                },
                &temp.0,
            )
            .unwrap();
        crate::backups::restore_backup(
            crate::backups::prepare_backup(&path, Some(&preview.archive_sha256)).unwrap(),
            &fresh,
        )
        .unwrap();
        assert!(fresh.isolated);
        assert_eq!(
            fs::read(fresh.game_dir.join("saves/World/level.dat")).unwrap(),
            b"world"
        );
        assert_eq!(
            fs::read_to_string(fresh.instance_dir.join("pack-loader.txt")).unwrap(),
            loader
        );
        assert_eq!(
            fs::read(source.game_dir.join("saves/World/level.dat")).unwrap(),
            b"world"
        );
    }
}

#[tokio::test]
async fn existing_multimc_directory_is_snapshotted_and_source_is_not_moved() {
    let temp = Temp::new();
    let source = temp.0.join("Existing");
    fs::create_dir_all(source.join("minecraft/saves/World")).unwrap();
    fs::write(
        source.join("mmc-pack.json"),
        br#"{"formatVersion":1,"components":[{"uid":"net.minecraft","version":"1.21.4"}]}"#,
    )
    .unwrap();
    fs::write(
        source.join("instance.cfg"),
        b"InstanceType=OneSix\nname=Existing",
    )
    .unwrap();
    fs::write(source.join("minecraft/saves/World/level.dat"), b"world").unwrap();
    let prepared = prepare_pack_ready(&source).unwrap();
    let snapshot = prepared._snapshot.as_ref().unwrap().archive_path();
    assert!(snapshot.exists());
    let target = temp.target("1.21.4");
    import_prepared_pack(prepared, &target, &engine(), None)
        .await
        .unwrap();
    assert!(!snapshot.exists());
    assert_eq!(
        fs::read(source.join("minecraft/saves/World/level.dat")).unwrap(),
        b"world"
    );
    assert_eq!(
        fs::read(target.game_dir.join("saves/World/level.dat")).unwrap(),
        b"world"
    );
}

#[test]
fn export_preserves_pending_loader_and_rejects_conflicting_requests() {
    let temp = Temp::new();
    let mut target = temp.target("1.21.4");
    fs::create_dir_all(&target.game_dir).unwrap();
    fs::write(
        target.instance_dir.join("pack-loader.txt"),
        b"neoforge@21.4.1",
    )
    .unwrap();
    let path = export_pack(&target, &temp.0).unwrap();
    assert_eq!(preview_pack(&path).unwrap().loader, "neoforge@21.4.1");
    let backup = crate::backups::create_backup(&target, &temp.0).unwrap();
    assert_eq!(
        crate::backups::preview_backup(&backup).unwrap().loader,
        "neoforge@21.4.1"
    );
    target.loader = "forge@54.0.0".into();
    assert!(export_pack(&target, &temp.0).is_err());
}

fn restricted_fixture(
    temp: &Temp,
) -> (
    PathBuf,
    CurseForgeManifest,
    std::collections::HashMap<u64, CurseForgePackFile>,
) {
    use sha1::{Digest, Sha1};
    let manifest: CurseForgeManifest = serde_json::from_value(serde_json::json!({
        "minecraft":{"version":"1.21.4"},"name":"Restricted","version":"1",
        "files":[{"projectID":123,"fileID":456,"required":true}]
    }))
    .unwrap();
    let bytes = serde_json::to_vec(&manifest).unwrap();
    let path = temp.pack(&[
        ("manifest.json", &bytes),
        ("overrides/config/test.txt", b"config"),
    ]);
    let detail = serde_json::from_value(serde_json::json!({
        "id":456,"modId":123,"fileName":"expected.jar","fileLength":7,"downloadUrl":null,
        "hashes":[{"algo":1,"value":hex::encode(Sha1::digest(b"fixture"))}]
    }))
    .unwrap();
    (
        path,
        manifest,
        std::collections::HashMap::from([(456, detail)]),
    )
}

#[tokio::test]
async fn restricted_pack_wrong_same_name_retries_directory_and_imports_without_network() {
    let temp = Temp::new();
    let (path, manifest, details) = restricted_fixture(&temp);
    let wrong = temp.0.join("expected.jar");
    fs::write(&wrong, b"wrong!!").unwrap();
    let resolution = resolve_curseforge_pack(&path, &manifest, &details, &[wrong.clone()])
        .await
        .unwrap();
    assert!(resolution.prepared.is_none());
    assert!(resolution.unresolved_files[0].can_select_local);
    assert!(require_complete_curseforge_files(&resolution.unresolved_files).is_err());
    let selected = temp.0.join("Downloads");
    fs::create_dir(&selected).unwrap();
    let renamed = selected.join("renamed-download.bin");
    fs::write(&renamed, b"fixture").unwrap();
    let mut resolution = resolve_curseforge_pack(&path, &manifest, &details, &[selected])
        .await
        .unwrap();
    assert!(resolution.unresolved_files.is_empty());
    // Import must consume the verified snapshot rather than reread a subsequently changed selection.
    fs::write(&renamed, b"changed").unwrap();
    let target = temp.target("1.21.4");
    import_prepared_pack(
        resolution.prepared.take().unwrap(),
        &target,
        &engine(),
        None,
    )
    .await
    .unwrap();
    assert_eq!(
        fs::read(target.game_dir.join("mods/expected.jar")).unwrap(),
        b"fixture"
    );
    assert_eq!(
        fs::read(target.game_dir.join("config/test.txt")).unwrap(),
        b"config"
    );
    assert_eq!(fs::read(wrong).unwrap(), b"wrong!!");
    let saved = fs::read_to_string(target.instance_dir.join("modrinth.index.json")).unwrap();
    assert!(!saved.contains("mfmcl-pack-files-"));
    assert!(
        !saved.contains("forgecdn"),
        "restricted files never gain a synthesized CDN URL"
    );
}

#[tokio::test]
async fn restricted_pack_accumulates_selections_before_creating_one_instance() {
    let temp = Temp::new();
    let (_, mut manifest, mut details) = restricted_fixture(&temp);
    manifest.files.push(CurseForgeManifestFile {
        project_id: 124,
        file_id: 457,
        required: true,
    });
    let mut second = details[&456].clone();
    second.id = 457;
    second.mod_id = 124;
    second.file_name = "second.jar".into();
    {
        use sha1::{Digest, Sha1};
        second.hashes[0].value = hex::encode(Sha1::digest(b"second!"));
    }
    details.insert(457, second);
    let bytes = serde_json::to_vec(&manifest).unwrap();
    let path = temp.pack(&[("manifest.json", &bytes)]);
    let store = LauncherStore::open(":memory:").unwrap();
    let first_selection = resolve_curseforge_pack(&path, &manifest, &details, &[])
        .await
        .unwrap();
    assert_eq!(first_selection.unresolved_files.len(), 2);
    assert!(store.list_instances().unwrap().is_empty());
    let source = temp.0.join("downloaded.bin");
    fs::write(&source, b"fixture").unwrap();
    let partial = resolve_curseforge_pack(&path, &manifest, &details, &[source.clone()])
        .await
        .unwrap();
    assert!(partial.prepared.is_none());
    assert_eq!(partial.unresolved_files.len(), 1);
    assert_eq!(partial.unresolved_files[0].file_id, 457);
    assert!(store.list_instances().unwrap().is_empty());
    let second = temp.0.join("downloaded-second.bin");
    fs::write(&second, b"second!").unwrap();
    let ready = resolve_curseforge_pack(&path, &manifest, &details, &[source, second])
        .await
        .unwrap();
    assert!(ready.unresolved_files.is_empty());
    assert!(store.list_instances().unwrap().is_empty());
    let target = store
        .create_instance(
            &CreateInstanceInput {
                name: "Complete".into(),
                game_version: "1.21.4".into(),
            },
            &temp.0,
        )
        .unwrap();
    import_prepared_pack(ready.prepared.unwrap(), &target, &engine(), None)
        .await
        .unwrap();
    assert_eq!(store.list_instances().unwrap().len(), 1);
    assert_eq!(
        fs::read(target.game_dir.join("mods/expected.jar")).unwrap(),
        b"fixture"
    );
    assert_eq!(
        fs::read(target.game_dir.join("mods/second.jar")).unwrap(),
        b"second!"
    );
}

#[tokio::test]
async fn restricted_pack_rejects_missing_hash_wrong_size_identity_and_changed_manifest() {
    let temp = Temp::new();
    let (path, manifest, details) = restricted_fixture(&temp);
    let selected = temp.0.join("expected.jar");
    fs::write(&selected, b"fixture").unwrap();
    for variant in 0..5 {
        let mut bad = details.clone();
        let detail = bad.get_mut(&456).unwrap();
        match variant {
            0 => detail.hashes.clear(),
            1 => detail.file_length += 1,
            2 => detail.mod_id += 1,
            3 => detail.id += 1,
            _ => detail.file_name = "../expected.jar".into(),
        }
        let resolution = resolve_curseforge_pack(&path, &manifest, &bad, &[selected.clone()])
            .await
            .unwrap();
        assert!(resolution.prepared.is_none());
        assert_eq!(resolution.unresolved_files[0].project_id, 123);
        assert_eq!(resolution.unresolved_files[0].file_id, 456);
        if variant != 1 {
            assert!(!resolution.unresolved_files[0].can_select_local);
        }
    }
    let mut changed = manifest.clone();
    changed.files[0].project_id = 789;
    let mut bad = details.clone();
    bad.get_mut(&456).unwrap().mod_id = 789;
    assert!(
        resolve_curseforge_pack(&path, &changed, &bad, &[selected.clone()])
            .await
            .is_err()
    );
    let resolution = resolve_curseforge_pack(&path, &manifest, &details, &[])
        .await
        .unwrap();
    assert!(resolution.prepared.is_none());
    assert!(
        !temp.0.join("instances").exists(),
        "cancel before import creates no instance"
    );
}

#[tokio::test]
async fn restricted_pack_cancellation_and_existing_target_preserve_source_and_target() {
    let temp = Temp::new();
    let (path, manifest, details) = restricted_fixture(&temp);
    let selected = temp.0.join("input.bin");
    fs::write(&selected, b"fixture").unwrap();
    let target = temp.target("1.21.4");
    let resolution = resolve_curseforge_pack(&path, &manifest, &details, &[selected.clone()])
        .await
        .unwrap();
    let cancelled = engine();
    cancelled.control().cancel();
    assert!(
        import_prepared_pack(resolution.prepared.unwrap(), &target, &cancelled, None)
            .await
            .is_err()
    );
    assert!(!target.game_dir.exists());
    assert!(!target.instance_dir.join("modrinth.index.json").exists());
    fs::create_dir_all(target.game_dir.join("mods")).unwrap();
    fs::write(target.game_dir.join("mods/expected.jar"), b"existing").unwrap();
    let resolution = resolve_curseforge_pack(&path, &manifest, &details, &[selected.clone()])
        .await
        .unwrap();
    assert!(
        import_prepared_pack(resolution.prepared.unwrap(), &target, &engine(), None)
            .await
            .is_err()
    );
    assert_eq!(
        fs::read(target.game_dir.join("mods/expected.jar")).unwrap(),
        b"existing"
    );
    assert_eq!(fs::read(selected).unwrap(), b"fixture");
}

#[test]
fn missing_optional_curseforge_files_are_locatable_but_not_required_errors() {
    let manifest: CurseForgeManifest = serde_json::from_value(serde_json::json!({"minecraft":{"version":"1.21.4"},"files":[{"projectID":123,"fileID":456,"required":false}]})).unwrap();
    let (_, missing) = curseforge_files_to_pack_files(&manifest, &Default::default());
    assert_eq!(missing[0].project_id, 123);
    assert_eq!(missing[0].file_id, 456);
    assert!(!missing[0].required);
    assert!(require_complete_curseforge_files(&missing).is_ok());
}

// Shapes reconstructed from the official exporter, not a guessed dependency manifest:
// https://github.com/HMCL-dev/HMCL/blob/880b8cb072349e76dea19b129e787254cd530133/HMCL/src/main/java/org/jackhuang/hmcl/game/HMCLModpackExportTask.java
// https://github.com/HMCL-dev/HMCL/blob/30acc53ee864861a116817ee76fdf0bb7f29d500/HMCL/src/main/java/org/jackhuang/hmcl/setting/VersionSetting.java
fn hmcl_fixture() -> (Vec<u8>, Vec<u8>, Vec<u8>) {
    let manifest = serde_json::json!({
        "name":"HMCL Native", "author":"Fixture Author", "version":"2.0",
        "gameVersion":"1.20.1", "description":"Official native export shape"
    });
    let version = serde_json::json!({
        "id":"Custom Pack", "jar":"1.20.1", "type":"release",
        "mainClass":"net.fabricmc.loader.impl.launch.knot.KnotClient",
        "arguments":{
            "jvm":["-Djava.library.path=${natives_directory}","-cp","${classpath}"],
            "game":["--username","${auth_player_name}","--accessToken","${auth_access_token}"]
        },
        "assetIndex":{"id":"5","url":"https://untrusted.invalid/assets.json"},
        "libraries":[
            {"name":"net.fabricmc:fabric-loader:0.16.10","url":"https://untrusted.invalid/"},
            {"name":"net.fabricmc:intermediary:1.20.1"},
            {"name":"org.lwjgl:lwjgl:3.3.1"}
        ]
    });
    let settings = serde_json::json!({
        "usesGlobal":false,"javaArgs":"-Dfile.encoding=UTF-8 -XX:MaxGCPauseMillis=100",
        "minecraftArgs":"--width 1280 --height 720 --demo", "autoMemory":false,
        "maxMemory":4096,"minMemory":1024,"width":854,"height":480,
        "fullscreen":false,"showLogs":true,"java":"Custom","javaVersionType":"CUSTOM",
        "javaDir":"C:/untrusted/java.exe","gameDirType":2,"gameDir":"C:/outside",
        "precalledCommand":"untrusted-pre-command", "postExitCommand":"untrusted-post-command",
        "wrapper":"untrusted-wrapper", "environmentVariables":"JAVA_TOOL_OPTIONS=-javaagent:bad.jar",
        "accounts":[{"accessToken":"private-token"}], "unknownPreference":{"anything":true}
    });
    (
        serde_json::to_vec(&manifest).unwrap(),
        serde_json::to_vec(&version).unwrap(),
        serde_json::to_vec(&settings).unwrap(),
    )
}

#[tokio::test]
async fn hmcl_official_native_shape_imports_offline_and_exposes_only_portable_preferences() {
    for root in ["", "Export/Folder/Pack/"] {
        let temp = Temp::new();
        let (manifest, version, settings) = hmcl_fixture();
        let path = temp.pack(&[
            (&format!("{root}modpack.json"), &manifest),
            (&format!("{root}minecraft/pack.json"), &version),
            (&format!("{root}minecraft/hmclversion.cfg"), &settings),
            (&format!("{root}minecraft/config/test.json"), b"{}"),
            (&format!("{root}minecraft/mods/example.jar"), b"bundled mod"),
            (&format!("{root}minecraft/saves/World/level.dat"), b"world"),
            (
                &format!("{root}minecraft/launcher_accounts.json"),
                b"private-token",
            ),
            (
                &format!("{root}minecraft/.hmcl/config/authlib-injector-servers.json"),
                b"private-token",
            ),
            (
                &format!("{root}minecraft/libraries/untrusted.jar"),
                b"never execute",
            ),
        ]);
        let mut prepared = prepare_pack_ready(&path).unwrap();
        assert_eq!(prepared.preview().name, "HMCL Native");
        assert_eq!(prepared.preview().version, "2.0");
        assert_eq!(prepared.preview().minecraft, "1.20.1");
        assert_eq!(prepared.preview().loader, "fabric@0.16.10");
        let expected = prepared.imported_overrides().unwrap().clone();
        assert_eq!(expected.memory_mode, Some(crate::MemoryMode::Custom));
        assert_eq!(expected.memory_mb, Some(4096));
        assert_eq!(
            (expected.window_width, expected.window_height),
            (Some(1280), Some(720))
        );
        assert_eq!(expected.game_args.as_deref(), Some("--demo"));
        assert_eq!(expected.show_game_log, Some(true));
        assert!(
            expected
                .extra_jvm_args
                .as_deref()
                .unwrap()
                .contains("-Xms1024M")
        );
        assert!(expected.pre_launch_command.is_none());
        assert!(expected.game_dir.is_none());
        assert_eq!(prepared.take_imported_overrides(), Some(expected));
        assert!(prepared.imported_overrides().is_none());
        let target = temp.target("1.20.1");
        import_prepared_pack(prepared, &target, &engine(), None)
            .await
            .unwrap();
        assert_eq!(
            fs::read(target.game_dir.join("mods/example.jar")).unwrap(),
            b"bundled mod"
        );
        assert_eq!(
            fs::read(target.game_dir.join("saves/World/level.dat")).unwrap(),
            b"world"
        );
        for name in [
            "pack.json",
            "hmclversion.cfg",
            "launcher_accounts.json",
            ".hmcl",
            "libraries",
        ] {
            assert!(!target.game_dir.join(name).exists(), "{name}");
        }
        let saved = fs::read_to_string(target.instance_dir.join("modrinth.index.json")).unwrap();
        for secret in [
            "mainClass",
            "untrusted.invalid",
            "auth_access_token",
            "private-token",
            "untrusted-pre-command",
            "javaDir",
        ] {
            assert!(!saved.contains(secret), "{secret}");
        }
        assert!(path.exists(), "native source is never moved");
    }
}

#[test]
fn hmcl_real_version_metadata_detects_loader_coordinates_and_patch_profiles() {
    let temp = Temp::new();
    for (libraries, arguments, expected) in [
        (serde_json::json!([]), serde_json::json!([]), "vanilla"),
        (
            serde_json::json!([{"name":"net.minecraftforge:forge:1.20.1-47.3.0"}]),
            serde_json::json!([]),
            "forge@47.3.0",
        ),
        (
            serde_json::json!([{"name":"net.neoforged:neoforge:20.1.5"}]),
            serde_json::json!([]),
            "neoforge@20.1.5",
        ),
        (
            serde_json::json!([{"name":"net.neoforged.fancymodloader:loader:4.0.0"}]),
            serde_json::json!(["--fml.neoForgeVersion", "20.1.5"]),
            "neoforge@20.1.5",
        ),
        (
            serde_json::json!([{"name":"org.quiltmc:quilt-loader:0.27.1"}]),
            serde_json::json!([]),
            "quilt@0.27.1",
        ),
        (
            serde_json::json!([{"name":"com.mumfrey:liteloader:1.12.2-SNAPSHOT"}]),
            serde_json::json!([]),
            "liteloader@1.12.2-SNAPSHOT",
        ),
        (
            serde_json::json!([{"name":"com.cleanroommc:cleanroom:0.3.0"}]),
            serde_json::json!([]),
            "cleanroom@0.3.0",
        ),
        (
            serde_json::json!([{"name":"net.fabricmc:fabric-loader:0.16.10"},{"name":"net.legacyfabric:intermediary:1.20.1"}]),
            serde_json::json!([]),
            "legacyfabric@0.16.10",
        ),
    ] {
        let version = serde_json::to_vec(&serde_json::json!({"id":"UntrustedName","jar":"1.20.1","mainClass":"net.minecraft.client.main.Main","libraries":libraries,"arguments":{"game":arguments}})).unwrap();
        let path = temp.pack(&[
            ("modpack.json", br#"{"name":"Native","version":"1"}"#),
            ("minecraft/pack.json", &version),
        ]);
        assert_eq!(
            prepare_pack_ready(&path).unwrap().preview().loader,
            expected
        );
    }
    let version = serde_json::to_vec(&serde_json::json!({
        "id":"Custom","jar":"1.20.1","mainClass":"arbitrary.UntrustedMain",
        "patches":[{"id":"game","version":"1.20.1","hidden":true},{"id":"fabric","version":"0.16.10","libraries":[{"name":"net.fabricmc:fabric-loader:0.16.10"}]}]
    })).unwrap();
    let path = temp.pack(&[
        ("modpack.json", br#"{"name":"Native"}"#),
        ("minecraft/pack.json", &version),
    ]);
    assert_eq!(
        prepare_pack_ready(&path).unwrap().preview().loader,
        "fabric@0.16.10"
    );
}

#[tokio::test]
async fn hmcl_single_version_tree_flattens_shared_and_isolated_data_without_launch_metadata() {
    for game in ["minecraft", ".minecraft", "game_root"] {
        let temp = Temp::new();
        let version = br#"{"id":"Custom","inheritsFrom":"1.20.1","mainClass":"net.minecraft.client.main.Main"}"#;
        let path = temp.pack(&[
            (
                &format!("Export/{game}/versions/Custom/Custom.json"),
                version,
            ),
            (
                &format!("Export/{game}/versions/Custom/Custom.jar"),
                b"never execute jar",
            ),
            (
                &format!("Export/{game}/versions/Custom/hmclversion.cfg"),
                br#"{"usesGlobal":false,"maxMemory":2048,"width":0,"height":0}"#,
            ),
            (
                &format!("Export/{game}/versions/Custom/config/isolated.txt"),
                b"isolated",
            ),
            (&format!("Export/{game}/config/shared.txt"), b"shared"),
            (
                &format!("Export/{game}/versions/Custom/options.txt"),
                b"music:0.5",
            ),
        ]);
        let prepared = prepare_pack_ready(&path).unwrap();
        assert_eq!(prepared.imported_overrides().unwrap().memory_mb, Some(2048));
        let target = temp.target("1.20.1");
        import_prepared_pack(prepared, &target, &engine(), None)
            .await
            .unwrap();
        assert_eq!(
            fs::read(target.game_dir.join("config/isolated.txt")).unwrap(),
            b"isolated"
        );
        assert_eq!(
            fs::read(target.game_dir.join("config/shared.txt")).unwrap(),
            b"shared"
        );
        assert!(!target.game_dir.join("versions").exists());
        assert!(!target.game_dir.join("Custom.jar").exists());
        assert!(!target.game_dir.join("hmclversion.cfg").exists());
    }
}

#[tokio::test]
async fn hmcl_flattened_version_folder_and_current_settings_are_convertible() {
    let temp = Temp::new();
    let version =
        br#"{"id":"Custom","inheritsFrom":"1.20.1","mainClass":"net.minecraft.client.main.Main"}"#;
    let settings = br#"{"overrideProperties":["autoMemory","maxMemory","width","height","jvmOptions"],"autoMemory":false,"maxMemory":2048,"width":1280,"height":720,"jvmOptions":"-Dfile.encoding=UTF-8"}"#;
    let path = temp.pack(&[
        ("Export/Custom/Custom.json", version),
        ("Export/Custom/Custom.jar", b"never execute"),
        (
            "Export/Custom/.hmcl/config/instance-game-settings.json",
            settings,
        ),
        ("Export/Custom/config/test", b"flat"),
    ]);
    let prepared = prepare_pack_ready(&path).unwrap();
    assert_eq!(prepared.imported_overrides().unwrap().memory_mb, Some(2048));
    let target = temp.target("1.20.1");
    import_prepared_pack(prepared, &target, &engine(), None)
        .await
        .unwrap();
    assert_eq!(
        fs::read(target.game_dir.join("config/test")).unwrap(),
        b"flat"
    );
    assert!(!target.game_dir.join("Custom.jar").exists());
    assert!(!target.game_dir.join("Custom.json").exists());
    assert!(!target.game_dir.join(".hmcl").exists());

    let source = temp.0.join("flat-directory");
    fs::create_dir_all(source.join("config")).unwrap();
    fs::write(source.join("Custom.json"), version).unwrap();
    fs::write(source.join("hmclversion.cfg"), br#"{"maxMemory":2048}"#).unwrap();
    fs::write(source.join("config/test"), b"flat-directory").unwrap();
    assert!(prepare_pack_ready(&source).unwrap()._snapshot.is_some());
}

#[test]
fn hmcl_settings_schema_respects_inheritance_unknown_fields_and_rejects_unknown_versions() {
    let modern = serde_json::json!({
        "overrideProperties":["maxMemory","autoMemory","windowType","width","height","jvmOptions","gameArguments","preLaunchCommand","unknown"],
        "autoMemory":false,"maxMemory":3072,"windowType":"FULLSCREEN","width":1280.0,"height":720.0,
        "jvmOptions":"-Duser.language=zh -XX:+AlwaysPreTouch","gameArguments":"--disableChat",
        "showLogs":true,"preLaunchCommand":"never-copy", "unknown":{"secret":"never-copy"}
    });
    let overrides = hmcl::settings_to_overrides(&modern).unwrap();
    assert_eq!(overrides.memory_mb, Some(3072));
    assert_eq!(overrides.fullscreen, Some(true));
    assert_eq!(overrides.show_game_log, None);
    assert_eq!(overrides.game_args.as_deref(), Some("--disableChat"));
    assert!(overrides.pre_launch_command.is_none());
    assert_eq!(
        hmcl::settings_to_overrides(
            &serde_json::json!({"usesGlobal":true,"maxMemory":1,"javaArgs":"-javaagent:bad.jar"})
        )
        .unwrap(),
        crate::InstanceOverrides::default()
    );
    assert_eq!(hmcl::settings_to_overrides(&serde_json::json!({"overrideProperties":[],"maxMemory":1,"jvmOptions":"-javaagent:bad.jar"})).unwrap(), crate::InstanceOverrides::default());
    assert!(hmcl::settings_to_overrides(&serde_json::json!({"formatVersion":2})).is_err());
    assert!(hmcl::settings_to_overrides(&serde_json::json!({"width":1280})).is_err());
    assert!(
        hmcl::settings_to_overrides(&serde_json::json!({"maxMemory":1024,"javaArgs":"-Xmx2G"}))
            .is_err()
    );
    assert!(
        hmcl::settings_to_overrides(&serde_json::json!({"maxMemory":1024,"minMemory":2048}))
            .is_err()
    );
    assert_eq!(
        hmcl::settings_to_overrides(&serde_json::json!({"javaArgs":"-Xmx2G"}))
            .unwrap()
            .memory_mb,
        Some(2048)
    );
}

#[test]
fn hmcl_customargs_cannot_import_agents_tokens_commands_or_external_paths() {
    for text in [
        "-javaagent:bad.jar",
        "-agentlib:jdwp=transport=dt_socket,server=y",
        "-agentpath:C:/bad.dll",
        "@external.args",
        "-cp bad.jar",
        "--class-path bad.jar",
        "-jar evil.jar",
        "-Djava.library.path=C:/evil",
        "-Dlog4j.configurationFile=https://evil.invalid/x",
        "-XX:OnError=calc.exe",
        "-XX:OnOutOfMemoryError=calc.exe",
        "-XX:HeapDumpPath=C:/outside",
        "-Xms1G\n-javaagent:bad.jar",
        "'-Dfile.encoding=UTF-8",
        "-Duser.language='zh -javaagent:x'",
    ] {
        assert!(
            hmcl::settings_to_overrides(&serde_json::json!({"javaArgs":text})).is_err(),
            "{text}"
        );
    }
    for text in [
        "--accessToken private-token",
        "--username someone",
        "--uuid bad",
        "--gameDir C:/outside",
        "--assetsDir ../outside",
        "@args.txt",
        "--server host;calc",
        "--port 65536",
        "--demo\n--username bad",
    ] {
        let error =
            hmcl::settings_to_overrides(&serde_json::json!({"minecraftArgs":text})).unwrap_err();
        assert!(!error.contains("private-token"));
    }
    let safe = hmcl::settings_to_overrides(&serde_json::json!({"javaArgs":"-Xms512M -Xss1M -Dfile.encoding=UTF-8","minecraftArgs":"--server example.org --port 25565 --fullscreen"})).unwrap();
    assert_eq!(safe.fullscreen, Some(true));
    assert_eq!(
        safe.game_args.as_deref(),
        Some("--server example.org --port 25565")
    );
}

#[test]
fn hmcl_rejects_ambiguous_versions_path_aliases_custom_patches_and_fake_formats() {
    let temp = Temp::new();
    let (manifest, version, _) = hmcl_fixture();
    for extra in [
        "minecraft/config/../../escape",
        "minecraft/config/A.txt",
        "minecraft/mods/CON.jar",
        "minecraft/config/x:stream",
        "minecraft/config/a\\b",
        "minecraft/prelaunch.cmd",
    ] {
        let path = temp.pack(&[
            ("modpack.json", &manifest),
            ("minecraft/pack.json", &version),
            ("minecraft/config/a.txt", b"a"),
            (extra, b"bad"),
        ]);
        assert!(prepare_pack(&path).is_err(), "{extra}");
    }
    for entries in [
        vec![
            ("minecraft/versions/One/One.json", version.as_slice()),
            ("minecraft/versions/Two/Two.json", version.as_slice()),
        ],
        vec![
            ("minecraft/versions/One/One.json", version.as_slice()),
            ("minecraft/versions/One/config/a", b"one"),
            ("minecraft/config/a", b"shared"),
        ],
        vec![
            ("modpack.json", manifest.as_slice()),
            ("minecraft/pack.json", version.as_slice()),
            (".minecraft/pack.json", version.as_slice()),
        ],
    ] {
        assert!(prepare_pack(&temp.pack(&entries)).is_err());
    }
    for metadata in [
        serde_json::json!({"jar":"../escape"}),
        serde_json::json!({"jar":"1.20.1","patches":[{"id":"unknown","version":"1"}]}),
        serde_json::json!({"jar":"1.20.1","mainClass":"untrusted.CustomMain"}),
        serde_json::json!({"jar":"1.20.1","formatVersion":99}),
        serde_json::json!({"jar":"1.21.4"}),
    ] {
        let bytes = serde_json::to_vec(&metadata).unwrap();
        assert!(
            prepare_pack(
                &temp.pack(&[("modpack.json", &manifest), ("minecraft/pack.json", &bytes)])
            )
            .is_err()
        );
    }
    assert!(
        prepare_pack(&temp.pack(&[
            ("modpack.json", br#"{"name":"Fake","formatVersion":1}"#),
            ("minecraft/pack.json", &version)
        ]))
        .is_err()
    );
    let mut large = version.clone();
    large.resize(MAX_INDEX as usize + 1, b' ');
    assert!(
        prepare_pack(&temp.pack(&[("modpack.json", &manifest), ("minecraft/pack.json", &large)]))
            .is_err()
    );
}

#[tokio::test]
async fn hmcl_cancellation_corruption_and_metadata_conflicts_roll_back_without_persisting_preferences()
 {
    let temp = Temp::new();
    let (manifest, version, settings) = hmcl_fixture();
    for failure in ["cancel", "corrupt", "metadata"] {
        let path = temp.pack(&[
            ("modpack.json", &manifest),
            ("minecraft/pack.json", &version),
            ("minecraft/hmclversion.cfg", &settings),
            ("minecraft/config/test.txt", b"original data"),
        ]);
        let mut prepared = prepare_pack_ready(&path).unwrap();
        let pending_preferences = prepared.take_imported_overrides().unwrap();
        assert_eq!(pending_preferences.memory_mb, Some(4096));
        let target = temp.target("1.20.1");
        let engine = engine();
        if failure == "cancel" {
            engine.control().cancel();
        }
        if failure == "corrupt" {
            let offset = prepared
                .archive
                .by_name("minecraft/config/test.txt")
                .unwrap()
                .data_start()
                .unwrap();
            let mut file = OpenOptions::new().write(true).open(&path).unwrap();
            file.seek(SeekFrom::Start(offset)).unwrap();
            file.write_all(b"CORRUPT").unwrap();
        }
        if failure == "metadata" {
            fs::create_dir_all(&target.instance_dir).unwrap();
            fs::write(
                target.instance_dir.join("modrinth.index.json"),
                b"keep existing",
            )
            .unwrap();
        }
        assert!(
            import_prepared_pack(prepared, &target, &engine, None)
                .await
                .is_err()
        );
        assert!(!target.game_dir.exists());
        if failure == "metadata" {
            assert_eq!(
                fs::read(target.instance_dir.join("modrinth.index.json")).unwrap(),
                b"keep existing"
            );
            assert_eq!(fs::read_dir(&target.instance_dir).unwrap().count(), 1);
        } else {
            assert!(!target.instance_dir.exists());
        }
        assert!(path.exists());
    }
}

#[tokio::test]
async fn hmcl_directory_snapshot_is_immutable_and_rejects_symlinked_metadata() {
    let temp = Temp::new();
    let source = temp.0.join("hmcl-existing");
    fs::create_dir_all(source.join("versions/Custom/config")).unwrap();
    fs::write(
        source.join("versions/Custom/Custom.json"),
        br#"{"id":"Custom","inheritsFrom":"1.20.1","mainClass":"net.minecraft.client.main.Main"}"#,
    )
    .unwrap();
    fs::write(source.join("versions/Custom/config/test"), b"original").unwrap();
    let prepared = prepare_pack_ready(&source).unwrap();
    let snapshot = prepared._snapshot.as_ref().unwrap().archive_path();
    fs::write(source.join("versions/Custom/config/test"), b"changed").unwrap();
    let target = temp.target("1.20.1");
    import_prepared_pack(prepared, &target, &engine(), None)
        .await
        .unwrap();
    assert!(!snapshot.exists());
    assert_eq!(
        fs::read(target.game_dir.join("config/test")).unwrap(),
        b"original"
    );
    assert_eq!(
        fs::read(source.join("versions/Custom/config/test")).unwrap(),
        b"changed"
    );
    let path = temp.0.join("symlink.zip");
    let mut zip = ZipWriter::new(File::create(&path).unwrap());
    zip.start_file("modpack.json", SimpleFileOptions::default())
        .unwrap();
    zip.write_all(br#"{"name":"Symlink"}"#).unwrap();
    zip.add_symlink(
        "minecraft/pack.json",
        "../outside",
        SimpleFileOptions::default(),
    )
    .unwrap();
    zip.finish().unwrap();
    assert!(prepare_pack(&path).is_err());
}

#[test]
fn multimc_additional_uids_and_safe_custom_settings_are_converted_without_commands() {
    let temp = Temp::new();
    for (uid, expected) in [
        ("com.mumfrey.liteloader", "liteloader"),
        ("com.cleanroommc.cleanroom", "cleanroom"),
        ("net.legacyfabric.fabric-loader", "legacyfabric"),
    ] {
        let components = serde_json::to_vec(&serde_json::json!({"formatVersion":1,"components":[{"uid":"net.minecraft","version":"1.12.2"},{"uid":uid,"version":"1.0"}]})).unwrap();
        let path = temp.pack(&[("mmc-pack.json", &components), ("instance.cfg", b"InstanceType=OneSix\nOverrideMemory=true\nMaxMemAlloc=4096\nMinMemAlloc=1024\nOverrideWindow=true\nMinecraftWinWidth=1280\nMinecraftWinHeight=720\nLaunchMaximized=false\nOverrideJavaArgs=true\nJvmArgs=-Dfile.encoding=UTF-8\nOverrideCommands=true\nPreLaunchCommand=never-copy\nWrapperCommand=never-copy\nJavaPath=C:/untrusted/java.exe")]);
        let prepared = prepare_pack_ready(&path).unwrap();
        assert_eq!(prepared.preview().loader, format!("{expected}@1.0"));
        let settings = prepared.imported_overrides().unwrap();
        assert_eq!(settings.memory_mb, Some(4096));
        assert_eq!(settings.window_width, Some(1280));
        assert_eq!(settings.window_height, Some(720));
        assert!(
            settings
                .extra_jvm_args
                .as_ref()
                .unwrap()
                .contains("-Xms1024M")
        );
        assert!(settings.pre_launch_command.is_none());
        assert!(settings.game_dir.is_none());
    }
    let components = br#"{"formatVersion":1,"components":[{"uid":"net.minecraft","version":"1.12.2"},{"uid":"net.fabricmc.fabric-loader","version":"0.16.10"},{"uid":"net.legacyfabric.intermediary","version":"1.12.2"}]}"#;
    let path = temp.pack(&[
        ("mmc-pack.json", components),
        ("instance.cfg", b"InstanceType=OneSix"),
    ]);
    assert_eq!(
        prepare_pack_ready(&path).unwrap().preview().loader,
        "legacyfabric@0.16.10"
    );
    let path = temp.pack(&[
        ("mmc-pack.json", components),
        (
            "instance.cfg",
            b"InstanceType=OneSix\nOverrideJavaArgs=true\nJvmArgs=-javaagent:bad.jar",
        ),
    ]);
    assert!(prepare_pack(&path).is_err());
}
