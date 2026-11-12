//! Opt-in live Mojang acceptance; never launches Minecraft or opens an account store.
use download_engine::{DownloadStage, verify_file};
use launcher_core::{
    DownloadTaskState, InstanceRecord, InstanceState, LauncherSettings, LauncherStore,
};
use launcher_runtime::{
    Downloads, EventSink, GameLogLine, GameProcessEvent, LaunchProgressEvent, ProgressEvent,
    RuntimeContext, TaskEvent, backup, install,
};
use mc_metadata::{Artifact, AssetIndex, GameVersionMetadata, Library, SourcePreference};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

const VERSION: &str = "1.21.4";

struct TempRoot(PathBuf);
impl Drop for TempRoot {
    fn drop(&mut self) {
        match fs::remove_dir_all(&self.0) {
            Ok(()) => eprintln!("REAL_INSTALL cleanup=removed root={}", self.0.display()),
            Err(error) => eprintln!(
                "REAL_INSTALL cleanup=failed root={} error={error}",
                self.0.display()
            ),
        }
    }
}

struct LiveSink<'a> {
    downloads: &'a Downloads,
    cancel_on_download: AtomicBool,
    tasks: Mutex<Vec<TaskEvent>>,
    received: Mutex<BTreeMap<String, u64>>,
}
impl EventSink for LiveSink<'_> {
    fn download_progress(&self, event: &ProgressEvent) {
        if event.progress.stage == DownloadStage::Downloading && event.progress.received > 0 {
            self.received
                .lock()
                .unwrap()
                .entry(event.progress.artifact_id.clone())
                .and_modify(|bytes| *bytes = (*bytes).max(event.progress.received))
                .or_insert(event.progress.received);
            let source = reqwest::Url::parse(&event.progress.source).unwrap();
            assert!(
                source.scheme() == "https"
                    && [
                        "piston-data.mojang.com",
                        "piston-meta.mojang.com",
                        "libraries.minecraft.net",
                        "resources.download.minecraft.net",
                        "launcher.mojang.com",
                        "launchermeta.mojang.com"
                    ]
                    .contains(&source.host_str().unwrap()),
                "live acceptance must use official hosts: artifact={} source={}",
                event.progress.artifact_id,
                event.progress.source
            );
            if self.cancel_on_download.swap(false, Ordering::SeqCst) {
                self.downloads.control(&event.instance_id).unwrap().cancel();
                eprintln!("REAL_INSTALL cancellation_requested_after_real_bytes=true");
            }
        }
    }
    fn installation_task(&self, event: &TaskEvent) {
        self.tasks.lock().unwrap().push(event.clone());
    }
    fn launch_progress(&self, _: &LaunchProgressEvent) {
        panic!("must not launch Minecraft");
    }
    fn game_log(&self, _: &GameLogLine) {
        panic!("must not launch Minecraft");
    }
    fn game_process(&self, _: &GameProcessEvent) {
        panic!("must not launch Minecraft");
    }
}

async fn phase(
    label: &str,
    context: &RuntimeContext,
    store: &LauncherStore,
    downloads: &Downloads,
    sink: &LiveSink<'_>,
    instance: &InstanceRecord,
) -> launcher_runtime::Result<()> {
    sink.received.lock().unwrap().clear();
    let started = Instant::now();
    let result = install::install_instance(context, store, downloads, sink, &instance.id);
    tokio::pin!(result);
    let deadline = tokio::time::sleep(Duration::from_secs(900));
    tokio::pin!(deadline);
    let mut heartbeat = tokio::time::interval(Duration::from_secs(15));
    loop {
        tokio::select! {
            outcome = &mut result => {
                eprintln!("REAL_INSTALL phase={label} elapsed_ms={} observed_payload_bytes={} result={}",
                    started.elapsed().as_millis(), sink.received.lock().unwrap().values().sum::<u64>(),
                    if outcome.is_ok() { "ok" } else { "error" });
                if let Err(error) = &outcome { eprintln!("REAL_INSTALL phase={label} error={error}"); }
                return outcome;
            }
            _ = &mut deadline => {
                if let Some(control) = downloads.control(&instance.id) { control.cancel(); }
                panic!("REAL_INSTALL phase={label} exceeded 900 seconds");
            }
            _ = heartbeat.tick() => {
                eprintln!("REAL_INSTALL heartbeat phase={label} elapsed_s={} observed_payload_bytes={}",
                    started.elapsed().as_secs(), sink.received.lock().unwrap().values().sum::<u64>());
            }
        }
    }
}

// This verifier deliberately fails closed on rules outside this pinned release's vocabulary.
fn applies(library: &Library) -> bool {
    let os = match std::env::consts::OS {
        "macos" => "osx",
        os => os,
    };
    let arch = match std::env::consts::ARCH {
        "x86_64" => "x64",
        "aarch64" => "arm64",
        "x86" => "x86",
        other => panic!("unsupported arch: {other}"),
    };
    let mut allowed = library.rules.is_empty();
    for rule in &library.rules {
        assert!(rule.features.is_none(), "unexpected library feature rule");
        let matches = rule.os.as_ref().is_none_or(|r| {
            assert!(r.version.is_none(), "unexpected OS-version regex");
            if let Some(a) = &r.arch {
                assert!(
                    ["x86", "x64", "arm64"].contains(&a.as_str()),
                    "unexpected architecture regex: {a}"
                );
            }
            r.name.as_deref().is_none_or(|name| name == os)
                && r.arch.as_deref().is_none_or(|name| name == arch)
        });
        if matches {
            assert!(["allow", "disallow"].contains(&rule.action.as_str()));
            allowed = rule.action == "allow";
        }
    }
    allowed
}

fn insert(files: &mut BTreeMap<PathBuf, Artifact>, path: PathBuf, artifact: Artifact) {
    assert!(artifact.size.is_some(), "official size required");
    assert_eq!(
        artifact.sha1.as_ref().unwrap().len(),
        40,
        "official SHA-1 required"
    );
    files.insert(path, artifact);
}

async fn expected_files(
    root: &Path,
    instance: &InstanceRecord,
    metadata: &GameVersionMetadata,
) -> BTreeMap<PathBuf, Artifact> {
    let mut files = BTreeMap::new();
    insert(
        &mut files,
        instance
            .version_root()
            .join(VERSION)
            .join(format!("{VERSION}.jar")),
        metadata.downloads["client"].clone(),
    );
    for library in &metadata.libraries {
        if !applies(library) {
            continue;
        }
        assert!(
            library.natives.is_empty(),
            "pinned release should use native artifact libraries"
        );
        if let Some(artifact) = &library.downloads.artifact {
            let path = artifact.path.as_ref().expect("official library path");
            insert(
                &mut files,
                root.join("libraries").join(path),
                artifact.clone(),
            );
        }
    }
    let index = metadata.asset_index.as_ref().unwrap();
    let index_path = root
        .join("assets/indexes")
        .join(format!("{}.json", index.id));
    insert(
        &mut files,
        index_path.clone(),
        Artifact {
            path: None,
            url: index.url.clone(),
            size: index.size,
            sha1: index.sha1.clone(),
        },
    );
    verify_file(&index_path, index.size, index.sha1.as_deref(), None)
        .await
        .unwrap();
    let assets: AssetIndex = serde_json::from_slice(&fs::read(index_path).unwrap()).unwrap();
    assert!(!assets.objects.is_empty());
    assert!(
        !assets.virtual_assets && !assets.map_to_resources,
        "unexpected legacy asset layout"
    );
    for object in assets.objects.values() {
        assert_eq!(object.hash.len(), 40);
        insert(
            &mut files,
            root.join("assets/objects")
                .join(&object.hash[..2])
                .join(&object.hash),
            Artifact {
                path: None,
                url: String::new(),
                size: Some(object.size),
                sha1: Some(object.hash.clone()),
            },
        );
    }
    if let Some(logging) = &metadata.logging {
        let file = &logging["client"]["file"];
        let artifact: Artifact = serde_json::from_value(file.clone()).unwrap();
        insert(
            &mut files,
            root.join("assets/log_configs")
                .join(file["id"].as_str().unwrap()),
            artifact,
        );
    }
    files
}

async fn verify_all(files: &BTreeMap<PathBuf, Artifact>, label: &str) {
    let started = Instant::now();
    for (path, artifact) in files {
        verify_file(path, artifact.size, artifact.sha1.as_deref(), None)
            .await
            .unwrap_or_else(|error| panic!("{label}: {error}"));
    }
    eprintln!(
        "REAL_INSTALL verification={label} files={} bytes={} elapsed_ms={}",
        files.len(),
        files.values().map(|a| a.size.unwrap()).sum::<u64>(),
        started.elapsed().as_millis()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "LIVE DOWNLOAD (~1 GB): requires MFMCL_REAL_INSTALL=1; no game launch, no credentials"]
async fn official_install_retry_repair_and_backup_restore() {
    assert_eq!(
        std::env::var("MFMCL_REAL_INSTALL").as_deref(),
        Ok("1"),
        "Explicit consent required: set MFMCL_REAL_INSTALL=1 and pass --ignored"
    );
    let started = Instant::now();
    let root =
        TempRoot(std::env::temp_dir().join(format!("MFMCL real 验收 {}", uuid::Uuid::new_v4())));
    fs::create_dir(&root.0).unwrap();
    eprintln!(
        "REAL_INSTALL platform={}-{} version={VERSION} root={} started={}",
        std::env::consts::OS,
        std::env::consts::ARCH,
        root.0.display(),
        chrono::Utc::now().to_rfc3339()
    );
    let context = RuntimeContext::new(&root.0);
    let store = LauncherStore::open(root.0.join("acceptance.sqlite")).unwrap();
    let mut settings = LauncherSettings::default();
    settings.download.source = SourcePreference::Official;
    settings.download.aria2_enabled = false;
    settings.download.max_concurrent_downloads = 8;
    settings.download.connections_per_file = 1;
    settings.download.timeout_sec = 45;
    settings.download.retries = 2;
    store.save_settings(&settings).unwrap();
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(45))
        .user_agent("MFMCL opt-in live acceptance")
        .build()
        .unwrap();
    let manifest = mc_metadata::fetch_manifest(&client, SourcePreference::Official)
        .await
        .unwrap();
    let metadata = mc_metadata::fetch_version_metadata(
        &client,
        &manifest,
        VERSION,
        SourcePreference::Official,
    )
    .await
    .unwrap();
    let upper_bound = metadata.asset_index.as_ref().unwrap().total_size.unwrap()
        + metadata.downloads["client"].size.unwrap()
        + metadata
            .libraries
            .iter()
            .filter_map(|l| l.downloads.artifact.as_ref()?.size)
            .sum::<u64>();
    assert!(
        upper_bound < 3 * 1024 * 1024 * 1024,
        "unexpectedly large pinned release"
    );
    eprintln!("REAL_INSTALL preflight_payload_upper_bound_bytes={upper_bound} source=official");
    let instance =
        install::create_instance(&context, &store, "Real vanilla 验收", VERSION, None).unwrap();
    assert!(instance.isolated && instance.game_dir.starts_with(&root.0));
    let downloads = Downloads::default();
    let sink = LiveSink {
        downloads: &downloads,
        cancel_on_download: AtomicBool::new(true),
        tasks: Mutex::new(Vec::new()),
        received: Mutex::new(BTreeMap::new()),
    };

    let cancelled = phase(
        "cancel-after-live-bytes",
        &context,
        &store,
        &downloads,
        &sink,
        &instance,
    )
    .await;
    assert!(cancelled.is_err(), "cancellation must fail the install");
    assert!(
        !sink.cancel_on_download.load(Ordering::SeqCst),
        "must receive real bytes before cancellation"
    );
    assert_eq!(
        sink.tasks.lock().unwrap().last().unwrap().task.state,
        DownloadTaskState::Cancelled
    );
    assert_eq!(
        install::find_instance(&store, &instance.id).unwrap().state,
        InstanceState::Failed
    );
    assert!(downloads.control(&instance.id).is_none());

    phase(
        "retry-install",
        &context,
        &store,
        &downloads,
        &sink,
        &instance,
    )
    .await
    .unwrap();
    assert_eq!(
        install::find_instance(&store, &instance.id).unwrap().state,
        InstanceState::Ready
    );
    let installed_metadata: GameVersionMetadata = serde_json::from_slice(
        &fs::read(
            instance
                .version_root()
                .join(VERSION)
                .join(format!("{VERSION}.json")),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(
        serde_json::to_value(installed_metadata).unwrap(),
        serde_json::to_value(&metadata).unwrap(),
        "installed profile must match the hash-verified official metadata"
    );
    let files = expected_files(&root.0, &instance, &metadata).await;
    verify_all(&files, "after-install").await;

    let client_path = instance
        .version_root()
        .join(VERSION)
        .join(format!("{VERSION}.jar"));
    let asset_path = files
        .keys()
        .find(|p| p.starts_with(root.0.join("assets/objects")))
        .unwrap()
        .clone();
    // Same-size corruption forces a hash check instead of merely a length check.
    use std::io::{Read, Seek, SeekFrom, Write};
    let mut jar = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&client_path)
        .unwrap();
    let mut byte = [0_u8];
    jar.read_exact(&mut byte).unwrap();
    jar.seek(SeekFrom::Start(0)).unwrap();
    jar.write_all(&[byte[0] ^ 0xff]).unwrap();
    drop(jar);
    fs::remove_file(&asset_path).unwrap();
    let client_artifact = &files[&client_path];
    assert!(
        verify_file(
            &client_path,
            client_artifact.size,
            client_artifact.sha1.as_deref(),
            None
        )
        .await
        .is_err()
    );
    assert!(!asset_path.exists());
    phase(
        "repair-corrupt-client-missing-asset",
        &context,
        &store,
        &downloads,
        &sink,
        &instance,
    )
    .await
    .unwrap();
    verify_all(&files, "after-repair").await;
    phase(
        "idempotent-retry",
        &context,
        &store,
        &downloads,
        &sink,
        &instance,
    )
    .await
    .unwrap();
    assert!(
        sink.received.lock().unwrap().is_empty(),
        "already verified artifacts must not be downloaded again"
    );
    verify_all(&files, "after-idempotent-retry").await;

    let world_path = "saves/Acceptance world/acceptance.txt";
    let world = b"Original acceptance data, not a real player's world";
    fs::create_dir_all(instance.game_dir.join("saves/Acceptance world")).unwrap();
    fs::write(instance.game_dir.join(world_path), world).unwrap();
    fs::write(instance.game_dir.join("options.txt"), b"lang:zh_cn\n").unwrap();
    let backups = root.0.join("private-backups");
    fs::create_dir(&backups).unwrap();
    let archive = backup::create(&store, &instance.id, backups.to_str().unwrap())
        .await
        .unwrap();
    let preview = backup::preview(archive.to_str().unwrap()).await.unwrap();
    assert_eq!(
        preview.file_count, 2,
        "official binaries must not be redistributed in the backup"
    );
    let refused = backup::restore(
        &context,
        &store,
        archive.to_str().unwrap(),
        &"0".repeat(64),
        "Must not exist",
    )
    .await;
    assert!(
        refused.is_err(),
        "restore must reject a mismatched preview hash"
    );
    assert_eq!(store.list_instances().unwrap().len(), 1);
    let restored = backup::restore(
        &context,
        &store,
        archive.to_str().unwrap(),
        &preview.archive_sha256,
        "Restored 验收",
    )
    .await
    .unwrap();
    assert_ne!(restored.id, instance.id);
    assert!(restored.isolated && restored.game_dir.starts_with(&root.0));
    assert_eq!(fs::read(restored.game_dir.join(world_path)).unwrap(), world);
    assert_eq!(
        fs::read(restored.game_dir.join("options.txt")).unwrap(),
        b"lang:zh_cn\n"
    );
    fs::write(
        restored.game_dir.join(world_path),
        b"changed restored instance",
    )
    .unwrap();
    assert_eq!(fs::read(instance.game_dir.join(world_path)).unwrap(), world);
    assert_eq!(store.list_instances().unwrap().len(), 2);
    assert!(store.list_accounts().unwrap().is_empty());
    assert!(!instance.game_dir.join("eula.txt").exists());
    eprintln!(
        "REAL_INSTALL backup_files={} backup_payload_bytes={} archive_bytes={} archive_sha256={} restored_independently=true",
        preview.file_count,
        preview.total_bytes,
        fs::metadata(archive).unwrap().len(),
        preview.archive_sha256
    );
    eprintln!(
        "REAL_INSTALL result=PASS elapsed_ms={} game_launched=false credentials_accessed=false",
        started.elapsed().as_millis()
    );
}
