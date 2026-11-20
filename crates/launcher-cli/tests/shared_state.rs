use launcher_core::{
    CreateInstanceInput, DownloadTask, DownloadTaskState, InstanceState, LauncherStore,
};
use std::{
    path::PathBuf,
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

struct TempRoot(PathBuf);
impl Drop for TempRoot {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

struct InstallProcess(Child);
impl Drop for InstallProcess {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
fn hold_paused_install() {
    let Some(root) = std::env::var_os("MFMCL_TEST_INSTALL_OWNER_DIR") else {
        return;
    };
    let root = PathBuf::from(root);
    let store = LauncherStore::open(root.join("mfmcl.sqlite")).unwrap();
    let instance = store.list_instances().unwrap().pop().unwrap();
    let _activity = store.lock_install_activity().unwrap();
    let _exclusive = store.lock_instance_operation(&instance.id).unwrap();
    store
        .set_instance_state(&instance.id, InstanceState::Installing)
        .unwrap();
    let now = chrono::Utc::now().to_rfc3339();
    store
        .save_download_task(&DownloadTask {
            id: uuid::Uuid::new_v4().to_string(),
            instance_id: instance.id.clone(),
            label: "Fixture".into(),
            source: None,
            state: DownloadTaskState::Paused,
            completed_bytes: 0,
            total_bytes: None,
            files: None,
            completed_files: None,
            created_at: now.clone(),
            updated_at: now,
        })
        .unwrap();
    std::fs::write(root.join("owner-ready"), b"ready").unwrap();
    loop {
        std::thread::park();
    }
}

#[test]
fn other_process_can_list_but_cannot_recover_or_remove_live_install() {
    let root =
        TempRoot(std::env::temp_dir().join(format!("mfmcl-shared-state-{}", uuid::Uuid::new_v4())));
    let store = LauncherStore::open(root.0.join("mfmcl.sqlite")).unwrap();
    let instance = store
        .create_isolated_instance(
            &CreateInstanceInput {
                name: "In progress".into(),
                game_version: "1.21.4".into(),
            },
            &root.0,
        )
        .unwrap();
    let mut owner = InstallProcess(
        Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "hold_paused_install"])
            .env("MFMCL_TEST_INSTALL_OWNER_DIR", &root.0)
            .stdout(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let start = Instant::now();
    while !root.0.join("owner-ready").is_file() {
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "install fixture timed out"
        );
        assert!(
            owner.0.try_wait().unwrap().is_none(),
            "install fixture exited early"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    let cli = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_mfmcl-cli"))
            .arg("--data-dir")
            .arg(&root.0)
            .args(args)
            .output()
            .unwrap()
    };
    assert!(cli(&["list", "instances"]).status.success());
    assert_eq!(
        store.list_instances().unwrap()[0].state,
        InstanceState::Installing
    );
    assert_eq!(
        store.list_download_tasks().unwrap()[0].state,
        DownloadTaskState::Paused
    );
    assert!(
        !cli(&["--yes", "remove", "--instance", &instance.id])
            .status
            .success()
    );
    assert_eq!(
        store.list_instances().unwrap()[0].state,
        InstanceState::Installing
    );
    assert_eq!(
        store.list_download_tasks().unwrap()[0].state,
        DownloadTaskState::Paused
    );

    owner.0.kill().unwrap();
    owner.0.wait().unwrap();
    assert!(store.recover_interrupted_work().unwrap());
    assert_eq!(
        store.list_instances().unwrap()[0].state,
        InstanceState::Failed
    );
    assert_eq!(
        store.list_download_tasks().unwrap()[0].state,
        DownloadTaskState::Interrupted
    );
}
