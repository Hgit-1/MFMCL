use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use thiserror::Error;
use tokio::process::{Child, Command};
use tokio::sync::Mutex;
use uuid::Uuid;

#[derive(Debug, Error)]
pub enum Aria2Error {
    #[error("could not start aria2: {0}")]
    Io(#[from] std::io::Error),
    #[error("aria2 RPC request failed: {0}")]
    Network(#[from] reqwest::Error),
    #[error("aria2 returned an RPC error: {0}")]
    Rpc(String),
    #[error("aria2 exited before becoming ready")]
    Exited,
    #[error("invalid aria2 response")]
    InvalidResponse,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Aria2Options {
    pub max_concurrent_downloads: u8,
    pub connections_per_file: u8,
}

impl Default for Aria2Options {
    fn default() -> Self {
        Self {
            max_concurrent_downloads: 4,
            connections_per_file: 4,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Aria2TaskStatus {
    pub gid: String,
    pub status: String,
    pub total_length: String,
    pub completed_length: String,
    pub download_speed: String,
    #[serde(default)]
    pub error_message: Option<String>,
}

#[derive(Clone)]
pub struct Aria2Session {
    rpc_url: String,
    secret: String,
    client: Client,
    child: std::sync::Arc<Mutex<Child>>,
    config_path: std::sync::Arc<ConfigFile>,
}

struct ConfigFile(PathBuf);
impl Drop for ConfigFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}
impl std::fmt::Debug for Aria2Session {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Aria2Session")
            .field("secret", &"[REDACTED]")
            .finish_non_exhaustive()
    }
}

/// Best-effort removal when a caller drops an in-flight download future.
pub(crate) struct TaskGuard {
    session: Aria2Session,
    gid: String,
}
impl Drop for TaskGuard {
    fn drop(&mut self) {
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            let session = self.session.clone();
            let gid = self.gid.clone();
            runtime.spawn(async move {
                if session.remove(&gid).await.is_err() {
                    let _ = session.shutdown().await;
                }
            });
        }
    }
}

impl Aria2Session {
    pub async fn start(
        binary: &Path,
        working_dir: &Path,
        options: Aria2Options,
    ) -> Result<Self, Aria2Error> {
        let client = Client::builder()
            .connect_timeout(std::time::Duration::from_secs(2))
            .timeout(std::time::Duration::from_secs(4))
            .redirect(reqwest::redirect::Policy::none())
            .build()?;
        std::fs::create_dir_all(working_dir)?;
        let listener = TcpListener::bind(("127.0.0.1", 0))?;
        let port = listener.local_addr()?.port();
        drop(listener);
        let secret = Uuid::new_v4().simple().to_string();
        let max_downloads = options.max_concurrent_downloads.clamp(1, 16);
        let connections = options.connections_per_file.clamp(1, 16);
        let config_path =
            working_dir.join(format!(".aria2-session-{}.conf", Uuid::new_v4().simple()));
        let config = format!(
            "enable-rpc=true\nrpc-listen-all=false\nrpc-listen-port={port}\nrpc-secret={secret}\nrpc-allow-origin-all=false\ndisable-ipv6=true\ncheck-certificate=true\ncheck-integrity=true\ncontinue=true\nallow-overwrite=true\nauto-file-renaming=false\nmax-concurrent-downloads={max_downloads}\nmax-connection-per-server={connections}\nsplit={connections}\nmin-split-size=1M\nfile-allocation=none\nconsole-log-level=warn\nlog=\n"
        );
        std::fs::write(&config_path, config)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if let Err(error) =
                std::fs::set_permissions(&config_path, std::fs::Permissions::from_mode(0o600))
            {
                let _ = std::fs::remove_file(&config_path);
                return Err(Aria2Error::Io(error));
            }
        }
        let config_path = std::sync::Arc::new(ConfigFile(config_path));
        let mut command = Command::new(binary);
        command
            .kill_on_drop(true)
            .arg(format!("--conf-path={}", config_path.0.to_string_lossy()))
            .current_dir(working_dir)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.as_std_mut().creation_flags(0x08000000);
        }
        let child = command.spawn()?;
        let session = Self {
            rpc_url: format!("http://127.0.0.1:{port}/jsonrpc"),
            secret,
            client,
            child: std::sync::Arc::new(Mutex::new(child)),
            config_path,
        };
        if let Err(error) = session.wait_until_ready().await {
            let _ = session.shutdown().await;
            return Err(error);
        }
        Ok(session)
    }

    #[cfg(all(test, unix))]
    pub(crate) fn failing_test_session(rpc_url: String, working_dir: &Path) -> Self {
        Self {
            rpc_url,
            secret: "test-secret-must-not-appear".into(),
            client: Client::builder()
                .timeout(std::time::Duration::from_secs(1))
                .build()
                .unwrap(),
            child: std::sync::Arc::new(Mutex::new(
                Command::new("/bin/sleep")
                    .arg("30")
                    .kill_on_drop(true)
                    .spawn()
                    .unwrap(),
            )),
            config_path: std::sync::Arc::new(ConfigFile(working_dir.join("test-aria2.conf"))),
        }
    }

    pub async fn add_uri(
        &self,
        urls: &[String],
        directory: &Path,
        file_name: &str,
        checksum: Option<&str>,
    ) -> Result<String, Aria2Error> {
        self.add_uri_bounded(urls, directory, file_name, checksum, 1)
            .await
    }

    pub(crate) fn task_guard(&self, gid: String) -> TaskGuard {
        TaskGuard {
            session: self.clone(),
            gid,
        }
    }

    pub(crate) async fn pause(&self, gid: &str) -> Result<(), Aria2Error> {
        self.rpc("aria2.forcePause", json!([gid])).await?;
        Ok(())
    }
    pub(crate) async fn unpause(&self, gid: &str) -> Result<(), Aria2Error> {
        self.rpc("aria2.unpause", json!([gid])).await?;
        Ok(())
    }

    pub(crate) async fn add_uri_bounded(
        &self,
        urls: &[String],
        directory: &Path,
        file_name: &str,
        checksum: Option<&str>,
        connections: u8,
    ) -> Result<String, Aria2Error> {
        if urls.is_empty()
            || urls.iter().any(|s| {
                reqwest::Url::parse(s)
                    .ok()
                    .is_none_or(|u| !crate::safe_url(&u))
            })
        {
            return Err(Aria2Error::Rpc("unsafe or empty download URLs".to_owned()));
        }
        if urls.len() > 1 && checksum.is_none() {
            return Err(Aria2Error::Rpc(
                "multiple sources require a checksum".to_owned(),
            ));
        }
        let mut options = BTreeMap::<String, Value>::new();
        let connections = connections.clamp(1, 16).to_string();
        options.insert("split".to_owned(), json!(connections));
        options.insert("max-connection-per-server".to_owned(), json!(connections));
        // Native HTTP handles redirects with our HTTPS-only redirect policy.
        options.insert("max-http-redirect".to_owned(), json!("0"));
        options.insert("max-tries".to_owned(), json!("3"));
        options.insert("retry-wait".to_owned(), json!("1"));
        options.insert("connect-timeout".to_owned(), json!("15"));
        options.insert("timeout".to_owned(), json!("60"));
        options.insert("dir".to_owned(), json!(directory.to_string_lossy()));
        options.insert("out".to_owned(), json!(file_name));
        options.insert("continue".to_owned(), json!("true"));
        options.insert("check-integrity".to_owned(), json!("true"));
        options.insert("allow-overwrite".to_owned(), json!("true"));
        options.insert("auto-file-renaming".to_owned(), json!("false"));
        if let Some(checksum) = checksum {
            options.insert("checksum".to_owned(), json!(checksum));
        }
        let result = self.rpc("aria2.addUri", json!([urls, options])).await?;
        result
            .as_str()
            .map(str::to_owned)
            .ok_or(Aria2Error::InvalidResponse)
    }

    pub async fn tell_status(&self, gid: &str) -> Result<Aria2TaskStatus, Aria2Error> {
        let result = self
            .rpc(
                "aria2.tellStatus",
                json!([
                    gid,
                    [
                        "gid",
                        "status",
                        "totalLength",
                        "completedLength",
                        "downloadSpeed",
                        "errorMessage"
                    ]
                ]),
            )
            .await?;
        serde_json::from_value(result).map_err(|_| Aria2Error::InvalidResponse)
    }

    pub async fn remove(&self, gid: &str) -> Result<(), Aria2Error> {
        let _ = self.rpc("aria2.forceRemove", json!([gid])).await?;
        Ok(())
    }

    pub async fn shutdown(&self) -> Result<(), Aria2Error> {
        let _ = self.rpc("aria2.shutdown", json!([])).await;
        let mut child = self.child.lock().await;
        if child.try_wait()?.is_none() {
            child.kill().await?;
        }
        let _ = child.wait().await;
        let _ = std::fs::remove_file(&self.config_path.0);
        Ok(())
    }

    async fn wait_until_ready(&self) -> Result<(), Aria2Error> {
        for _ in 0..24 {
            {
                let mut child = self.child.lock().await;
                if child.try_wait()?.is_some() {
                    return Err(Aria2Error::Exited);
                }
            }
            if self.rpc("aria2.getVersion", json!([])).await.is_ok() {
                return Ok(());
            }
            tokio::time::sleep(std::time::Duration::from_millis(125)).await;
        }
        Err(Aria2Error::Rpc("RPC did not become ready".to_owned()))
    }

    async fn rpc(&self, method: &str, params: Value) -> Result<Value, Aria2Error> {
        let mut call_params = vec![json!(format!("token:{}", self.secret))];
        if let Value::Array(method_params) = params {
            call_params.extend(method_params);
        } else {
            call_params.push(params);
        }
        let response = self
            .client
            .post(&self.rpc_url)
            .json(&json!({
                "jsonrpc": "2.0",
                "id": "mfmcl",
                "method": method,
                "params": call_params,
            }))
            .send()
            .await?
            .error_for_status()?;
        let body: Value = response.json().await?;
        if body.get("error").is_some() {
            // Remote RPC errors may echo request parameters, including the token.
            return Err(Aria2Error::Rpc("remote procedure failed".to_owned()));
        }
        body.get("result")
            .cloned()
            .ok_or(Aria2Error::InvalidResponse)
    }
}

pub fn find_aria2_binary(resource_dir: &Path) -> PathBuf {
    #[cfg(target_os = "windows")]
    let file_name = "aria2c.exe";
    #[cfg(not(target_os = "windows"))]
    let file_name = "aria2c";
    resource_dir
        .join("aria2")
        .join(std::env::consts::OS)
        .join(std::env::consts::ARCH)
        .join(file_name)
}
