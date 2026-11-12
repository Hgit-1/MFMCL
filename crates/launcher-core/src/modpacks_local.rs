//! Local supplements are matched against server metadata, never an archive-supplied digest or URL.
use super::*;
use std::collections::HashMap;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PackResolution {
    pub source_path: PathBuf,
    pub preview: PackPreview,
    pub unresolved_files: Vec<MissingPackFile>,
    #[serde(skip)]
    pub prepared: Option<PreparedPack>,
}

/// Metadata retains modId, which the general catalog's version projection intentionally omits.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CurseForgePackFile {
    pub id: u64,
    pub mod_id: u64,
    pub file_name: String,
    pub file_length: u64,
    #[serde(default)]
    pub download_url: Option<String>,
    #[serde(default)]
    pub hashes: Vec<CurseForgePackHash>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CurseForgePackHash {
    pub algo: u32,
    pub value: String,
}

#[derive(Default)]
pub(super) struct LocalFiles {
    root: Option<PathBuf>,
    files: HashMap<String, PathBuf>,
}
impl LocalFiles {
    pub(super) fn contains(&self, path: &str) -> bool {
        self.files.contains_key(path)
    }
    pub(super) fn get(&self, path: &str) -> Option<&Path> {
        self.files.get(path).map(PathBuf::as_path)
    }
    async fn snapshot(&mut self, source: &Path, file: &PackFile) -> Result<(), String> {
        if self.root.is_none() {
            let root =
                std::env::temp_dir().join(format!("mfmcl-pack-files-{}", uuid::Uuid::new_v4()));
            fs::create_dir(&root).map_err(|e| e.to_string())?;
            self.root = Some(root);
        }
        let path = self
            .root
            .as_ref()
            .unwrap()
            .join(uuid::Uuid::new_v4().to_string());
        copy_local(source, &path, file).await?;
        self.files.insert(file.path.clone(), path);
        Ok(())
    }
}
impl Drop for LocalFiles {
    fn drop(&mut self) {
        if let Some(root) = &self.root {
            let _ = fs::remove_dir_all(root);
        }
    }
}

pub(super) async fn copy_local(
    source: &Path,
    target: &Path,
    file: &PackFile,
) -> Result<(), String> {
    let meta = tokio::fs::symlink_metadata(source)
        .await
        .map_err(|e| e.to_string())?;
    if !meta.file_type().is_file() || meta.len() != file.file_size {
        return Err("本地补件不是普通文件或大小不符。".into());
    }
    validate_file_hashes(&file.hashes)?;
    let mut input = tokio::fs::File::open(source)
        .await
        .map_err(|e| e.to_string())?;
    if let Some(parent) = target.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| e.to_string())?;
    }
    let mut output = tokio::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(target)
        .await
        .map_err(|e| e.to_string())?;
    use tokio::io::AsyncReadExt;
    let copied = tokio::io::copy(&mut (&mut input).take(file.file_size + 1), &mut output)
        .await
        .map_err(|e| e.to_string())?;
    output.sync_all().await.map_err(|e| e.to_string())?;
    drop(output);
    if copied != file.file_size {
        return Err("本地补件在读取期间发生变化。".into());
    }
    download_engine::verify_file(
        target,
        Some(file.file_size),
        file.hashes.get("sha1").map(String::as_str),
        file.hashes.get("sha256").map(String::as_str),
    )
    .await
    .map_err(|_| format!("{}：本地文件校验不符。", file.path))
}

fn candidates(paths: &[PathBuf]) -> Result<Vec<PathBuf>, String> {
    let mut out = BTreeSet::new();
    if paths.len() > MAX_ARCHIVE_ENTRIES {
        return Err("选择的补件数量超过限制。".into());
    }
    for path in paths {
        let meta = match fs::symlink_metadata(path) {
            Ok(meta) => meta,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error.to_string()),
        };
        if meta.file_type().is_file() {
            out.insert(path.clone());
        } else if meta.file_type().is_dir() {
            // Only the selected directory, not an unbounded recursive walk through Downloads.
            for (count, entry) in fs::read_dir(path).map_err(|e| e.to_string())?.enumerate() {
                if count >= MAX_ARCHIVE_ENTRIES {
                    return Err("补件目录的文件数量超过限制。".into());
                }
                let entry = entry.map_err(|e| e.to_string())?;
                if entry.file_type().map_err(|e| e.to_string())?.is_file() {
                    out.insert(entry.path());
                }
            }
        } else {
            return Err("补件来源不能是符号链接或特殊文件。".into());
        }
        if out.len() > MAX_ARCHIVE_ENTRIES {
            return Err("选择的补件数量超过限制。".into());
        }
    }
    Ok(out.into_iter().collect())
}

pub async fn prepare_pack_for_import(
    path: &Path,
    local_paths: &[PathBuf],
    key: Option<&str>,
) -> Result<PackResolution, String> {
    match prepare_pack(path)? {
        PackPreparation::Ready(prepared) => Ok(PackResolution {
            source_path: path.to_path_buf(),
            preview: prepared.preview().clone(),
            unresolved_files: vec![],
            prepared: Some(prepared),
        }),
        PackPreparation::NeedsOnlineLookup { manifest, .. } => {
            let details = match lookup_files(&manifest.file_ids(), key).await {
                Ok(details) => details,
                Err(reason) => {
                    let mut resolution =
                        resolve_curseforge_pack(path, &manifest, &HashMap::new(), local_paths)
                            .await?;
                    for file in &mut resolution.unresolved_files {
                        file.reason = reason.clone();
                    }
                    return Ok(resolution);
                }
            };
            resolve_curseforge_pack(path, &manifest, &details, local_paths).await
        }
    }
}

pub async fn resolve_curseforge_pack(
    path: &Path,
    manifest: &CurseForgeManifest,
    details: &HashMap<u64, CurseForgePackFile>,
    local_paths: &[PathBuf],
) -> Result<PackResolution, String> {
    let candidates = candidates(local_paths)?;
    let mut files = Vec::new();
    let mut missing = Vec::new();
    let mut local_files = LocalFiles::default();
    let mut seen = BTreeSet::new();
    let mut bindings = HashMap::new();
    let mut resolved_bytes = 0_u64;
    for entry in &manifest.files {
        if let Some(project) = bindings.insert(entry.file_id, entry.project_id)
            && project != entry.project_id
        {
            return Err("同一 fileID 对应多个 projectID。".into());
        }
    }
    for entry in &manifest.files {
        if !seen.insert(entry.file_id) {
            continue;
        }
        let required = manifest
            .files
            .iter()
            .any(|f| f.file_id == entry.file_id && f.required);
        let detail = details.get(&entry.file_id);
        let resolved = (|| {
            let detail = detail.ok_or("API 未返回文件详情")?;
            if detail.id != entry.file_id || detail.mod_id != entry.project_id {
                return Err("API 返回的 projectID/fileID 不匹配");
            }
            let path =
                classify_curseforge_file(&detail.file_name).ok_or("无法可靠确定文件安装目录")?;
            safe_pack_path(&path).map_err(|_| "文件名不安全")?;
            if detail.file_length == 0 || detail.file_length > MAX_ENTRY {
                return Err("文件大小无效");
            }
            let sha1 = detail
                .hashes
                .iter()
                .find(|hash| hash.algo == 1)
                .ok_or("文件缺少 SHA-1 校验值")?;
            let hashes = HashMap::from([("sha1".to_owned(), sha1.value.clone())]);
            validate_file_hashes(&hashes).map_err(|_| "文件 SHA-1 校验值无效")?;
            Ok(PackFile {
                path,
                hashes,
                downloads: detail
                    .download_url
                    .clone()
                    .filter(|url| !url.is_empty())
                    .into_iter()
                    .collect(),
                file_size: detail.file_length,
                env: HashMap::from([(
                    "client".to_owned(),
                    if required { "required" } else { "optional" }.to_owned(),
                )]),
            })
        })();
        let mut can_select_local = false;
        let reason = match resolved {
            Err(reason) => reason.to_owned(),
            Ok(file) => {
                if required {
                    resolved_bytes = resolved_bytes
                        .checked_add(file.file_size)
                        .filter(|bytes| *bytes <= MAX_TOTAL)
                        .ok_or("整合包过大。")?;
                }
                if !file.downloads.is_empty() {
                    files.push(file);
                    continue;
                }
                can_select_local = true;
                let mut found = false;
                if required {
                    for source in &candidates {
                        if fs::symlink_metadata(source).is_ok_and(|meta| {
                            meta.file_type().is_file() && meta.len() == file.file_size
                        }) && download_engine::verify_file(
                            source,
                            Some(file.file_size),
                            file.hashes.get("sha1").map(String::as_str),
                            None,
                        )
                        .await
                        .is_ok()
                        {
                            local_files.snapshot(source, &file).await?;
                            found = true;
                            break;
                        }
                    }
                }
                if found {
                    files.push(file);
                    continue;
                }
                if candidates.is_empty() {
                    "需手动下载".to_owned()
                } else {
                    "未找到大小与 SHA-1 匹配的本地文件".to_owned()
                }
            }
        };
        missing.push(MissingPackFile {
            project_id: entry.project_id,
            file_id: entry.file_id,
            file_name: detail.map(|f| f.file_name.clone()),
            required,
            reason,
            can_select_local,
        });
    }
    let mut preview = manifest.to_preview();
    let prepared = if missing.iter().any(|file| file.required) {
        None
    } else {
        let prepared = super::prepare_completed_index(
            path,
            manifest.to_pack_index(files),
            local_files,
            Some(manifest),
        )?;
        preview = prepared.preview().clone();
        Some(prepared)
    };
    Ok(PackResolution {
        source_path: path.to_path_buf(),
        preview,
        unresolved_files: missing,
        prepared,
    })
}

async fn lookup_files(
    ids: &[u64],
    key: Option<&str>,
) -> Result<HashMap<u64, CurseForgePackFile>, String> {
    let key = key.map(str::trim).filter(|key| !key.is_empty());
    let base = if key.is_some() {
        "https://api.curseforge.com"
    } else {
        "https://mod.mcimirror.top/curseforge"
    };
    let client = reqwest::Client::builder()
        .user_agent(concat!("MFMCL/", env!("CARGO_PKG_VERSION")))
        .timeout(std::time::Duration::from_secs(30))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|e| e.to_string())?;
    let mut out = HashMap::new();
    for chunk in ids.chunks(200) {
        let mut request = client
            .post(format!("{base}/v1/mods/files"))
            .json(&serde_json::json!({"fileIds":chunk}));
        if let Some(key) = key {
            request = request.header("x-api-key", key);
        }
        let mut response = request
            .send()
            .await
            .map_err(|_| "无法查询 CurseForge 文件详情。")?
            .error_for_status()
            .map_err(|_| "CurseForge 文件详情请求失败。")?;
        let mut bytes = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| "读取 CurseForge 文件详情失败。")?
        {
            if bytes.len().saturating_add(chunk.len()) > MAX_INDEX as usize {
                return Err("CurseForge 文件详情过大。".into());
            }
            bytes.extend_from_slice(&chunk);
        }
        #[derive(Deserialize)]
        struct Response {
            data: Vec<CurseForgePackFile>,
        }
        let response: Response =
            serde_json::from_slice(&bytes).map_err(|_| "CurseForge 文件详情格式错误。")?;
        for file in response.data {
            if !chunk.contains(&file.id) || out.insert(file.id, file).is_some() {
                return Err("CurseForge 文件详情标识重复或不匹配。".into());
            }
        }
    }
    Ok(out)
}
