use super::*;
use std::collections::{BTreeMap, BTreeSet};

#[path = "catalog_solver.rs"]
mod solver;
pub use solver::{DependencyOptions, ProjectVersionLock};

#[derive(Clone, Deserialize)]
struct DependencyRef {
    #[serde(default)]
    project_id: Option<String>,
    #[serde(default)]
    version_id: Option<String>,
    dependency_type: String,
}

fn identifier(value: &str) -> Result<&str, String> {
    if value.is_empty()
        || value.len() > 128
        || !value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
    {
        return Err("模组依赖标识无效".into());
    }
    Ok(value)
}

fn compatible(value: &serde_json::Value, instance: &InstanceRecord, loader: &str) -> bool {
    value
        .get("game_versions")
        .and_then(serde_json::Value::as_array)
        .is_some_and(|versions| {
            versions
                .iter()
                .any(|v| v.as_str() == Some(instance.game_version.as_str()))
        })
        && value
            .get("loaders")
            .and_then(serde_json::Value::as_array)
            .is_some_and(|loaders| {
                loaders.iter().any(|v| {
                    v.as_str() == Some(loader)
                        || (loader == "quilt" && v.as_str() == Some("fabric"))
                })
            })
}

#[cfg(test)]
async fn plan(
    client: &reqwest::Client,
    base: &str,
    instance: &InstanceRecord,
    target: &CatalogVersion,
) -> Result<Vec<CatalogVersion>, String> {
    solver::resolve(
        client,
        base,
        instance,
        std::slice::from_ref(target),
        &DependencyOptions::default(),
    )
    .await
}

/// Resolve Modrinth required/incompatible edges without downloading JARs. Roots
/// are pinned by version ID and verified file identity; locks constrain project
/// IDs, never version numbers. Only this catalog's metadata is queried.
///
/// Bounds: 128 requests / 32 MiB metadata, 64 newest candidates per project,
/// 4096 cached versions/search states, 128 selected projects, 512 edges per
/// version / 4096 edges per plan / 65536 queued-state edges, and 30 seconds per
/// request including its body.
/// Failure is not proof that no combination outside these bounds exists.
pub async fn resolve_install(
    client: &reqwest::Client,
    instance: &InstanceRecord,
    targets: &[CatalogVersion],
    options: &DependencyOptions,
) -> Result<Vec<CatalogVersion>, String> {
    solver::resolve(client, API, instance, targets, options).await
}

struct Stage(std::path::PathBuf);
impl Drop for Stage {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn publish(stage: &Path, directory: &Path, files: &[String]) -> Result<(), String> {
    let mut linked = Vec::new();
    for file in files {
        let destination = directory.join(file);
        if let Err(error) = std::fs::hard_link(stage.join(file), &destination) {
            let mut rollback_errors = Vec::new();
            for path in linked {
                if let Err(rollback) = std::fs::remove_file(path) {
                    rollback_errors.push(rollback.to_string());
                }
            }
            return Err(if rollback_errors.is_empty() {
                format!("无法安装模组依赖：{error}；已回滚本次新文件，旧文件未改动")
            } else {
                format!(
                    "无法安装模组依赖：{error}；清理本次新文件失败：{}",
                    rollback_errors.join("；")
                )
            });
        }
        linked.push(destination);
    }
    Ok(())
}

/// Resolve mandatory Modrinth dependencies before publishing any new mod files.
pub async fn install(
    instance: &InstanceRecord,
    target: &CatalogVersion,
    engine: &DownloadEngine,
) -> Result<String, String> {
    install_with_options(instance, target, engine, &DependencyOptions::default()).await
}

/// Install one explicitly chosen root and a bounded, backtracked dependency plan.
/// Existing files are never upgraded, deleted or replaced. A lock is a Modrinth
/// project ID + version ID, not a local filename or version-number heuristic.
pub async fn install_with_options(
    instance: &InstanceRecord,
    target: &CatalogVersion,
    engine: &DownloadEngine,
    options: &DependencyOptions,
) -> Result<String, String> {
    let client = engine
        .settings()
        .configure_metadata_client(
            reqwest::Client::builder()
                .https_only(true)
                .redirect(reqwest::redirect::Policy::none()),
            &["api.modrinth.com"],
        )
        .map_err(|e| e.to_string())?
        .build()
        .map_err(|e| e.to_string())?;
    let versions = solver::resolve(
        &client,
        API,
        instance,
        std::slice::from_ref(target),
        options,
    )
    .await?;
    let directory = instance.game_dir.join("mods");
    for path in [&instance.game_dir, &directory] {
        match std::fs::symlink_metadata(path) {
            Ok(metadata) if metadata.file_type().is_dir() => {}
            Ok(_) => return Err("游戏或模组目录必须是普通目录，不能是符号链接".into()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.to_string()),
        }
    }
    std::fs::create_dir_all(&directory).map_err(|e| e.to_string())?;
    let stage = Stage(directory.join(format!(".dependencies-{}", uuid::Uuid::new_v4())));
    std::fs::create_dir(&stage.0).map_err(|e| e.to_string())?;
    let existing: Vec<_> = std::fs::read_dir(&directory)
        .map_err(|e| e.to_string())?
        .map(|entry| entry.map(|entry| entry.file_name().to_string_lossy().into_owned()))
        .collect::<Result<_, _>>()
        .map_err(|e| e.to_string())?;
    let mut files = Vec::new();
    for version in &versions {
        engine
            .control()
            .checkpoint()
            .await
            .map_err(|e| e.to_string())?;
        let destination = directory.join(&version.file_name);
        if existing
            .iter()
            .any(|name| name.eq_ignore_ascii_case(&version.file_name))
        {
            let metadata = std::fs::symlink_metadata(&destination)
                .map_err(|_| format!("模组文件名冲突：{}", version.file_name))?;
            if metadata.file_type().is_file()
                && download_engine::verify_file(
                    &destination,
                    Some(version.size),
                    Some(&version.sha1),
                    None,
                )
                .await
                .is_ok()
            {
                continue;
            }
            return Err(format!("已存在不同的模组文件：{}", version.file_name));
        }
        install_mod_version(version, &stage.0, engine).await?;
        files.push(version.file_name.clone());
    }
    engine
        .control()
        .checkpoint()
        .await
        .map_err(|e| e.to_string())?;
    publish(&stage.0, &directory, &files)?;
    Ok(directory
        .join(&versions[0].file_name)
        .to_string_lossy()
        .into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    fn value(id: &str, project: &str, dependencies: serde_json::Value) -> serde_json::Value {
        serde_json::json!({"id":id,"project_id":project,"version_number":"1","game_versions":["1.21.1"],"loaders":["fabric"],"dependencies":dependencies,"files":[{"url":format!("https://cdn.modrinth.com/{id}.jar"),"filename":format!("{id}.jar"),"size":1,"hashes":{"sha1":"a".repeat(40)}}]})
    }
    #[tokio::test]
    async fn required_dependencies_are_deduplicated_cycles_bounded_and_optionals_skipped() {
        let a = value(
            "A",
            "PA",
            serde_json::json!([{"version_id":"B","project_id":"PB","dependency_type":"required"},{"project_id":"PO","dependency_type":"optional"}]),
        );
        let b = value(
            "B",
            "PB",
            serde_json::json!([{"version_id":"A","project_id":"PA","dependency_type":"required"}]),
        );
        let target = parse_versions(&serde_json::to_vec(&vec![&a]).unwrap())
            .unwrap()
            .remove(0);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            for _ in 0..2 {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut request = [0; 2048];
                let n = stream.read(&mut request).await.unwrap();
                let body = if String::from_utf8_lossy(&request[..n]).contains("/version/A ") {
                    &a
                } else {
                    &b
                };
                let body = serde_json::to_vec(body).unwrap();
                stream
                    .write_all(
                        format!(
                            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                            body.len()
                        )
                        .as_bytes(),
                    )
                    .await
                    .unwrap();
                stream.write_all(&body).await.unwrap();
            }
        });
        let instance: InstanceRecord = serde_json::from_value(serde_json::json!({"id":"i","name":"i","gameVersion":"1.21.1","loader":"fabric@1","instanceDir":"fixture","gameDir":"fixture/game","isolated":true,"state":"ready","createdAt":""})).unwrap();
        let versions = plan(&reqwest::Client::new(), &base, &instance, &target)
            .await
            .unwrap();
        assert_eq!(versions.len(), 2);
        server.await.unwrap();
    }
    #[test]
    fn failed_publication_rolls_back_new_files_and_preserves_existing_files() {
        let root =
            std::env::temp_dir().join(format!("mfmcl-dependencies-{}", uuid::Uuid::new_v4()));
        let stage = root.join("stage");
        std::fs::create_dir_all(&stage).unwrap();
        for file in ["a.jar", "b.jar"] {
            std::fs::write(stage.join(file), b"new").unwrap();
        }
        std::fs::write(root.join("b.jar"), b"existing").unwrap();
        assert!(publish(&stage, &root, &["a.jar".into(), "b.jar".into()]).is_err());
        assert!(!root.join("a.jar").exists());
        assert_eq!(std::fs::read(root.join("b.jar")).unwrap(), b"existing");
        std::fs::remove_dir_all(root).unwrap();
    }
}
