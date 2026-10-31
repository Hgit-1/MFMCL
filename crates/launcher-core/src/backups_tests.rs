use super::*;
struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("mfmcl-backup-test-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn instance(root: &Path, id: &str) -> InstanceRecord {
    InstanceRecord {
        id: id.into(),
        name: "我的世界".into(),
        game_version: "1.21.4".into(),
        loader: "vanilla".into(),
        instance_dir: root.join(id),
        game_dir: root.join(id).join("game"),
        isolated: true,
        state: InstanceState::NotInstalled,
        created_at: String::new(),
        last_launched_at: None,
    }
}
fn archive(path: &Path, files: &[(&str, &[u8])], expected: &[(&str, &[u8])]) {
    let mut manifest = Manifest {
        format_version: 1,
        name: "Test".into(),
        minecraft: "1.21.4".into(),
        loader: "fabric@0.16.10".into(),
        created_at: String::new(),
        files: BTreeMap::new(),
    };
    for (name, data) in expected {
        manifest.files.insert(
            (*name).into(),
            Entry {
                size: data.len() as u64,
                sha256: hex::encode(Sha256::digest(data)),
            },
        );
    }
    let mut zip = ZipWriter::new(File::create(path).unwrap());
    for (name, data) in files {
        zip.start_file(*name, SimpleFileOptions::default()).unwrap();
        zip.write_all(data).unwrap();
    }
    zip.start_file(INDEX, SimpleFileOptions::default()).unwrap();
    zip.write_all(&serde_json::to_vec(&manifest).unwrap())
        .unwrap();
    zip.finish().unwrap();
}
#[test]
fn roundtrip_preserves_world_mods_and_options_without_launcher_secrets() {
    let temp = Temp::new();
    let mut original = instance(&temp.0, "source");
    original.loader = "quilt@0.27.1".into();
    fs::create_dir_all(original.game_dir.join("saves/世界")).unwrap();
    fs::create_dir(original.game_dir.join("config")).unwrap();
    fs::write(original.game_dir.join("saves/世界/level.dat"), b"world").unwrap();
    fs::write(original.game_dir.join("config/settings.json"), b"{}").unwrap();
    fs::write(original.game_dir.join("options.txt"), b"music:0.5").unwrap();
    fs::write(original.game_dir.join("launcher-accounts.json"), b"secret").unwrap();
    let path = create_backup(&original, &temp.0).unwrap();
    let preview = preview_backup(&path).unwrap();
    assert_eq!(preview.file_count, 3);
    let target = instance(&temp.0, "new");
    restore_backup(
        prepare_backup(&path, Some(&preview.archive_sha256)).unwrap(),
        &target,
    )
    .unwrap();
    assert_eq!(
        fs::read(target.game_dir.join("saves/世界/level.dat")).unwrap(),
        b"world"
    );
    assert!(!target.game_dir.join("launcher-accounts.json").exists());
    assert_eq!(
        fs::read_to_string(target.instance_dir.join("pack-loader.txt")).unwrap(),
        "quilt@0.27.1"
    );
    assert!(create_backup(&original, &original.game_dir).is_err());
}
#[test]
fn corruption_and_archive_mutation_never_publish_a_target() {
    let temp = Temp::new();
    let path = temp.0.join("bad.zip");
    archive(
        &path,
        &[("game/saves/world/a", b"evil")],
        &[("saves/world/a", b"good")],
    );
    let preview = preview_backup(&path).unwrap();
    let target = instance(&temp.0, "new");
    assert!(restore_backup(prepare_backup(&path, None).unwrap(), &target).is_err());
    assert!(!target.instance_dir.exists());
    assert!(!fs::read_dir(&temp.0).unwrap().any(|entry| {
        entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".restore-")
    }));
    archive(
        &path,
        &[("game/saves/world/a", b"good")],
        &[("saves/world/a", b"good")],
    );
    assert!(prepare_backup(&path, Some(&preview.archive_sha256)).is_err());
}
#[test]
fn rejects_unsafe_undeclared_and_case_conflicting_paths() {
    let temp = Temp::new();
    let path = temp.0.join("bad.zip");
    for name in [
        "../escape",
        "saves/../escape",
        "saves/CON",
        "mods/a.jar:evil",
        "launcher.sqlite",
    ] {
        archive(
            &path,
            &[(&format!("game/{name}"), b"data")],
            &[(name, b"data")],
        );
        assert!(preview_backup(&path).is_err(), "{name}");
    }
    archive(
        &path,
        &[("game/mods/A/a.jar", b"a"), ("game/mods/a/b.jar", b"b")],
        &[("mods/A/a.jar", b"a"), ("mods/a/b.jar", b"b")],
    );
    assert!(preview_backup(&path).is_err());
    archive(
        &path,
        &[("game/mods/a.jar", b"a"), ("extra", b"secret")],
        &[("mods/a.jar", b"a")],
    );
    assert!(preview_backup(&path).is_err());
}
#[test]
fn restoration_refuses_existing_or_incompatible_instances() {
    let temp = Temp::new();
    let path = temp.0.join("good.zip");
    archive(&path, &[("game/mods/a.jar", b"a")], &[("mods/a.jar", b"a")]);
    let mut target = instance(&temp.0, "new");
    target.game_version = "1.20.1".into();
    assert!(restore_backup(prepare_backup(&path, None).unwrap(), &target).is_err());
    target.game_version = "1.21.4".into();
    fs::create_dir_all(&target.game_dir).unwrap();
    fs::write(target.game_dir.join("keep"), b"original").unwrap();
    assert!(restore_backup(prepare_backup(&path, None).unwrap(), &target).is_err());
    assert_eq!(fs::read(target.game_dir.join("keep")).unwrap(), b"original");
}
#[cfg(unix)]
#[test]
fn backup_refuses_symlinks_even_when_dangling() {
    let temp = Temp::new();
    let original = instance(&temp.0, "source");
    fs::create_dir_all(&original.game_dir).unwrap();
    std::os::unix::fs::symlink(temp.0.join("missing"), original.game_dir.join("saves")).unwrap();
    assert!(create_backup(&original, &temp.0).is_err());
}
#[test]
fn streamed_limits_and_disk_errors_are_propagated() {
    struct Full;
    impl Write for Full {
        fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
            Err(std::io::Error::other("disk full"))
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    assert!(transfer(&mut &b"long"[..], &mut Vec::new(), 2, &mut 0).is_err());
    assert!(transfer(&mut &b"a"[..], &mut Full, 1, &mut 0).is_err());
    let mut budget = MAX_TOTAL;
    assert!(transfer(&mut &b"a"[..], &mut Vec::new(), 1, &mut budget).is_err());
}
