//! Opt-in real JVM checks. This runs an original tiny fixture, not Minecraft.
use launcher_core::{
    AccountKind, AccountProfile, InstanceRecord, InstanceState, LaunchIdentity,
    prepare_authenticated_launch, prepare_offline_launch,
};
use std::{fs, path::PathBuf, process::Command};
use tokio::io::BufReader;

struct Fixture {
    root: PathBuf,
    instance: InstanceRecord,
    metadata: mc_metadata::GameVersionMetadata,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("MFMCL JVM 测试 {}", uuid::Uuid::new_v4()));
        let instance = InstanceRecord {
            id: "test".into(),
            name: "JVM fixture".into(),
            game_version: "1.21.4".into(),
            loader: "vanilla".into(),
            instance_dir: root.join("instance"),
            game_dir: root.join("instance/game"),
            isolated: true,
            state: InstanceState::Ready,
            created_at: String::new(),
            last_launched_at: None,
        };
        fs::create_dir_all(&instance.game_dir).unwrap();
        fs::create_dir_all(root.join("classes")).unwrap();
        let source = root.join("Smoke.java");
        fs::write(
            &source,
            r#"import java.nio.file.Path;
public final class Smoke {
 public static void main(String[] args) throws Exception {
   System.out.println("cwd=" + Path.of("").toAbsolutePath());
   System.out.println("literal=" + System.getProperty("fixture.literal"));
   for(String arg : args) System.out.println("argument=" + arg);
   System.out.println("fixture-ready"); System.out.flush();
   if(args.length > 0 && args[0].equals("wait")) Thread.sleep(60000);
 }
}"#,
        )
        .unwrap();
        assert!(
            Command::new("javac")
                .arg("--release")
                .arg("17")
                .arg("-d")
                .arg(root.join("classes"))
                .arg(source)
                .status()
                .expect("Install a JDK with javac for JVM smoke tests")
                .success()
        );
        let dir = instance.instance_dir.join("versions/1.21.4");
        fs::create_dir_all(&dir).unwrap();
        assert!(
            Command::new("jar")
                .arg("--create")
                .arg("--file")
                .arg(dir.join("1.21.4.jar"))
                .arg("-C")
                .arg(root.join("classes"))
                .arg(".")
                .status()
                .expect("Install JDK jar tool")
                .success()
        );
        let metadata = serde_json::from_value(serde_json::json!({"id":"1.21.4","mainClass":"Smoke","javaVersion":{"component":"java-runtime-delta","majorVersion":17},"arguments":{"jvm":["-Dfixture.literal=0"],"game":["--username=${auth_player_name}","--uuid=${auth_uuid}","--accessToken=${auth_access_token}","--userType=${user_type}"]}})).unwrap();
        Self {
            root,
            instance,
            metadata,
        }
    }
}

#[test]
#[ignore = "requires a real JDK (java, javac, jar); runs no Minecraft code"]
fn authentic_and_offline_arguments_reach_a_real_jvm() {
    let fixture = Fixture::new();
    let account = AccountProfile {
        id: "offline".into(),
        display_name: "Offline_Name".into(),
        kind: AccountKind::Offline,
        auth_server: None,
        credential_ref: None,
        selected: true,
        created_at: String::new(),
    };
    let offline = prepare_offline_launch(
        &fixture.root,
        &fixture.instance,
        &account,
        &fixture.metadata,
        None,
    )
    .unwrap();
    let identity = LaunchIdentity {
        kind: AccountKind::Microsoft,
        profile_id: "123456781234423481231234567890ab",
        player_name: "Online_Name",
        access_token: "fixture-secret-${user_type}",
        client_id: "configured-id",
        xuid: "123",
    };
    let online = prepare_authenticated_launch(
        &fixture.root,
        &fixture.instance,
        &identity,
        &fixture.metadata,
        None,
    )
    .unwrap();
    for (prepared, expected_name, expected_type) in [
        (&offline, "Offline_Name", "legacy"),
        (&online, "Online_Name", "msa"),
    ] {
        let result = Command::new(&prepared.java_path)
            .args(&prepared.args)
            .current_dir(&prepared.working_directory)
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "JVM fixture failed without printing launch arguments"
        );
        let text = String::from_utf8_lossy(&result.stdout);
        assert!(text.contains(&format!("--username={expected_name}")));
        assert!(text.contains(&format!("--userType={expected_type}")));
        assert!(text.contains("literal=0"));
        assert!(text.contains("fixture-ready"));
        let safe = launcher_core::logs::redact_log(&text, &[identity.access_token]);
        assert!(!safe.contains("fixture-secret"));
    }
}

#[tokio::test]
#[ignore = "requires a real JDK; validates draining and killing the original Java fixture"]
async fn a_real_java_process_can_be_drained_and_stopped() {
    let fixture = Fixture::new();
    let mut child = tokio::process::Command::new("java")
        .arg("-cp")
        .arg(
            fixture
                .instance
                .instance_dir
                .join("versions/1.21.4/1.21.4.jar"),
        )
        .arg("Smoke")
        .arg("wait")
        .current_dir(&fixture.instance.game_dir)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let mut reader = BufReader::new(child.stdout.take().unwrap());
    let ready = tokio::time::timeout(std::time::Duration::from_secs(10), async {
        while let Some(line) = launcher_core::logs::bounded_log_line(&mut reader)
            .await
            .unwrap()
        {
            if line == "fixture-ready" {
                return true;
            }
        }
        false
    })
    .await
    .unwrap();
    assert!(ready);
    child.kill().await.unwrap();
    assert!(child.wait().await.is_ok());
}
