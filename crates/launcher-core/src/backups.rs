//! Private, hash-checked game-data backups. Restoration always targets a new instance.
use crate::{InstanceRecord, InstanceState, loaders::safe_version, modpacks::safe_pack_path};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, HashSet},
    fs::{self, File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
};
use zip::{ZipArchive, ZipWriter, write::SimpleFileOptions};

const INDEX: &str = "mfmcl-backup.json";
#[cfg(test)]
#[path = "backups_tests.rs"]
mod tests;
const MAX_FILES: usize = 50_000;
const MAX_FILE: u64 = 2 * 1024 * 1024 * 1024;
const MAX_TOTAL: u64 = 8 * 1024 * 1024 * 1024;
const MAX_INDEX: u64 = 16 * 1024 * 1024;
const MAX_ARCHIVE: u64 = MAX_TOTAL + 64 * 1024 * 1024;

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Entry {
    size: u64,
    sha256: String,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Manifest {
    format_version: u32,
    name: String,
    minecraft: String,
    loader: String,
    created_at: String,
    files: BTreeMap<String, Entry>,
}
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupPreview {
    pub name: String,
    pub minecraft: String,
    pub loader: String,
    pub created_at: String,
    pub file_count: usize,
    pub total_bytes: u64,
    pub archive_sha256: String,
}
pub struct PreparedBackup {
    archive: ZipArchive<File>,
    manifest: Manifest,
    preview: BackupPreview,
}
impl PreparedBackup {
    pub fn preview(&self) -> &BackupPreview {
        &self.preview
    }
}

// These roots contain game data, not launcher profiles, tokens, or machine paths.
fn permitted(path: &str) -> bool {
    matches!(path, "options.txt" | "optionsof.txt" | "servers.dat")
        || [
            "saves/",
            "mods/",
            "config/",
            "defaultconfigs/",
            "resourcepacks/",
            "shaderpacks/",
            "kubejs/",
            "screenshots/",
        ]
        .iter()
        .any(|prefix| path.starts_with(prefix))
}
fn loader_valid(loader: &str) -> Result<(), String> {
    if loader == "vanilla" {
        return Ok(());
    }
    let (kind, version) = loader.split_once('@').ok_or("备份加载器标识无效。")?;
    if !matches!(kind, "fabric" | "quilt") {
        return Err("此备份使用尚不支持恢复的加载器。".into());
    }
    safe_version(version)
}
fn ordinary_dir(path: &Path) -> Result<(), String> {
    let meta = fs::symlink_metadata(path).map_err(|e| e.to_string())?;
    if !meta.file_type().is_dir() {
        return Err("备份目录必须为普通目录，不能是符号链接。".into());
    }
    Ok(())
}
struct Cleanup(PathBuf);
impl Drop for Cleanup {
    fn drop(&mut self) {
        if self.0.is_dir() {
            let _ = fs::remove_dir_all(&self.0);
        } else {
            let _ = fs::remove_file(&self.0);
        }
    }
}
fn output_file(path: &Path) -> Result<File, String> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path).map_err(|e| e.to_string())
}

fn collect(
    root: &Path,
    relative: &str,
    files: &mut Vec<(String, PathBuf)>,
    registry: &mut HashSet<String>,
    total: &mut u64,
    visited: &mut usize,
) -> Result<(), String> {
    for item in fs::read_dir(root).map_err(|e| e.to_string())? {
        let item = item.map_err(|e| e.to_string())?;
        *visited += 1;
        if *visited > MAX_FILES {
            return Err("备份目录条目超过 50000 个。".into());
        }
        let name = item
            .file_name()
            .into_string()
            .map_err(|_| "备份文件名不是有效的 Unicode。")?;
        let relative = if relative.is_empty() {
            name
        } else {
            format!("{relative}/{name}")
        };
        safe_pack_path(&relative)?;
        let meta = fs::symlink_metadata(item.path()).map_err(|e| e.to_string())?;
        let allowed_dir = [
            "saves",
            "mods",
            "config",
            "defaultconfigs",
            "resourcepacks",
            "shaderpacks",
            "kubejs",
            "screenshots",
        ]
        .contains(&relative.as_str())
            || permitted(&relative);
        if !permitted(&relative) && !allowed_dir {
            continue;
        }
        if !registry.insert(relative.to_lowercase()) {
            return Err("备份存在大小写路径冲突。".into());
        }
        if meta.file_type().is_symlink() {
            return Err("备份数据包含符号链接，请先移除或复制为普通文件。".into());
        }
        if meta.is_dir() {
            if relative.split('/').count() > 32 {
                return Err("备份目录层级过深。".into());
            }
            collect(&item.path(), &relative, files, registry, total, visited)?;
        } else if meta.is_file() {
            if meta.len() > MAX_FILE {
                return Err("备份单个文件超过 2 GB。".into());
            }
            *total = total
                .checked_add(meta.len())
                .filter(|n| *n <= MAX_TOTAL)
                .ok_or("备份数据超过 8 GB。")?;
            files.push((relative, item.path()));
        } else {
            return Err("备份包含非普通文件。".into());
        }
    }
    Ok(())
}

fn transfer<R: Read, W: Write>(
    reader: &mut R,
    writer: &mut W,
    expected: u64,
    total: &mut u64,
) -> Result<String, String> {
    let mut hash = Sha256::new();
    let mut copied = 0_u64;
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = reader.read(&mut buffer).map_err(|e| e.to_string())?;
        if read == 0 {
            break;
        }
        copied = copied.checked_add(read as u64).ok_or("备份大小溢出。")?;
        *total = total.checked_add(read as u64).ok_or("备份大小溢出。")?;
        if copied > expected || copied > MAX_FILE || *total > MAX_TOTAL {
            return Err("备份实际文件大小超过限制或文件正在变化。".into());
        }
        hash.update(&buffer[..read]);
        writer
            .write_all(&buffer[..read])
            .map_err(|e| e.to_string())?;
    }
    if copied != expected {
        return Err("备份文件大小发生变化或归档已损坏。".into());
    }
    Ok(hex::encode(hash.finalize()))
}

/// Destination is a directory chosen by the user. No existing archive is replaced.
pub fn create_backup(instance: &InstanceRecord, directory: &Path) -> Result<PathBuf, String> {
    ordinary_dir(&instance.instance_dir)?;
    ordinary_dir(&instance.game_dir)?;
    ordinary_dir(directory)?;
    if directory
        .canonicalize()
        .map_err(|e| e.to_string())?
        .starts_with(
            instance
                .game_dir
                .canonicalize()
                .map_err(|e| e.to_string())?,
        )
    {
        return Err("请选择游戏目录之外的备份位置。".into());
    }
    safe_version(&instance.game_version)?;
    loader_valid(&instance.loader)?;
    let mut files = Vec::new();
    let mut declared = 0;
    let mut visited = 0;
    collect(
        &instance.game_dir,
        "",
        &mut files,
        &mut HashSet::new(),
        &mut declared,
        &mut visited,
    )?;
    files.sort_by(|a, b| a.0.cmp(&b.0));
    let id = uuid::Uuid::new_v4();
    let final_path = directory.join(format!(
        "MFMCL-backup-{}-{id}.mfmclbackup",
        chrono::Utc::now().format("%Y%m%d-%H%M%S")
    ));
    let temp = directory.join(format!(".mfmcl-backup-{id}.pending"));
    let _cleanup = Cleanup(temp.clone());
    let mut zip = ZipWriter::new(output_file(&temp)?);
    let options = SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated)
        .unix_permissions(0o600);
    let mut manifest = Manifest {
        format_version: 1,
        name: instance.name.clone(),
        minecraft: instance.game_version.clone(),
        loader: instance.loader.clone(),
        created_at: chrono::Utc::now().to_rfc3339(),
        files: BTreeMap::new(),
    };
    let mut total = 0;
    for (name, path) in files {
        let meta = fs::symlink_metadata(&path).map_err(|e| e.to_string())?;
        if !meta.file_type().is_file() {
            return Err("备份过程中发现文件被替换。".into());
        }
        zip.start_file(format!("game/{name}"), options)
            .map_err(|e| e.to_string())?;
        let sha256 = transfer(
            &mut File::open(path).map_err(|e| e.to_string())?,
            &mut zip,
            meta.len(),
            &mut total,
        )?;
        manifest.files.insert(
            name,
            Entry {
                size: meta.len(),
                sha256,
            },
        );
    }
    let json = serde_json::to_vec(&manifest).map_err(|e| e.to_string())?;
    if json.len() as u64 > MAX_INDEX {
        return Err("备份索引过大。".into());
    }
    zip.start_file(INDEX, options).map_err(|e| e.to_string())?;
    zip.write_all(&json).map_err(|e| e.to_string())?;
    let output = zip.finish().map_err(|e| e.to_string())?;
    output.sync_all().map_err(|e| e.to_string())?;
    drop(output);
    fs::hard_link(&temp, &final_path).map_err(|e| e.to_string())?;
    Ok(final_path)
}

pub fn prepare_backup(
    path: &Path,
    expected_sha256: Option<&str>,
) -> Result<PreparedBackup, String> {
    let meta = fs::symlink_metadata(path).map_err(|e| e.to_string())?;
    if !meta.file_type().is_file() || meta.len() > MAX_ARCHIVE {
        return Err("备份必须为有限大小的普通归档文件。".into());
    }
    let mut file = File::open(path).map_err(|e| e.to_string())?;
    let count = crate::modpacks::zip_entry_count(&mut file)?;
    if count == 0 || count > MAX_FILES as u64 + 1 {
        return Err("备份条目数量无效。".into());
    }
    file.seek(SeekFrom::Start(0)).map_err(|e| e.to_string())?;
    let mut hash = Sha256::new();
    let mut bytes = 0_u64;
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer).map_err(|e| e.to_string())?;
        if read == 0 {
            break;
        }
        bytes += read as u64;
        if bytes > MAX_ARCHIVE {
            return Err("备份归档过大。".into());
        }
        hash.update(&buffer[..read]);
    }
    let fingerprint = hex::encode(hash.finalize());
    if expected_sha256.is_some_and(|expected| expected != fingerprint) {
        return Err("备份文件与预览时不一致，请重新选择并确认。".into());
    }
    file.seek(SeekFrom::Start(0)).map_err(|e| e.to_string())?;
    let mut archive = ZipArchive::new(file).map_err(|e| e.to_string())?;
    if archive.len() as u64 != count {
        return Err("备份包含重复 ZIP 条目。".into());
    }
    let manifest: Manifest = {
        let mut index = archive
            .by_name(INDEX)
            .map_err(|_| "缺少 MFMCL 备份索引。")?;
        if index.size() > MAX_INDEX {
            return Err("备份索引超过限制。".into());
        }
        let mut bytes = Vec::new();
        (&mut index)
            .take(MAX_INDEX + 1)
            .read_to_end(&mut bytes)
            .map_err(|e| e.to_string())?;
        if bytes.len() as u64 > MAX_INDEX {
            return Err("备份索引超过限制。".into());
        }
        serde_json::from_slice(&bytes).map_err(|_| "备份索引格式无效。")?
    };
    safe_version(&manifest.minecraft)?;
    loader_valid(&manifest.loader)?;
    if manifest.format_version != 1
        || manifest.name.trim().is_empty()
        || manifest.name.chars().count() > 48
        || manifest.name.chars().any(char::is_control)
        || manifest.files.len() > MAX_FILES
    {
        return Err("备份格式或实例名称无效。".into());
    }
    let mut paths = HashSet::new();
    let mut parents = HashSet::new();
    let mut spelling = BTreeMap::new();
    let mut total = 0_u64;
    for (path, entry) in &manifest.files {
        safe_pack_path(path)?;
        if !permitted(path)
            || !paths.insert(path.to_lowercase())
            || entry.size > MAX_FILE
            || entry.sha256.len() != 64
            || !entry.sha256.bytes().all(|b| b.is_ascii_hexdigit())
        {
            return Err("备份索引包含不安全路径或无效校验值。".into());
        }
        let mut parts = path.split('/').collect::<Vec<_>>();
        if parts.len() > 32 {
            return Err("备份路径层级过深。".into());
        }
        let mut prefix = String::new();
        for part in &parts {
            if !prefix.is_empty() {
                prefix.push('/');
            }
            prefix.push_str(part);
            if spelling
                .insert(prefix.to_lowercase(), prefix.clone())
                .is_some_and(|prior| prior != prefix)
            {
                return Err("备份存在大小写目录别名。".into());
            }
        }
        parts.pop();
        while !parts.is_empty() {
            parents.insert(parts.join("/").to_lowercase());
            parts.pop();
        }
        total = total
            .checked_add(entry.size)
            .filter(|n| *n <= MAX_TOTAL)
            .ok_or("备份展开大小超过 8 GB。")?;
    }
    if !paths.is_disjoint(&parents) {
        return Err("备份存在父目录与文件路径冲突。".into());
    }
    if archive.len() != manifest.files.len() + 1 {
        return Err("备份存在索引未声明的条目。".into());
    }
    let mut seen = HashSet::new();
    for i in 0..archive.len() {
        let entry = archive.by_index(i).map_err(|e| e.to_string())?;
        if entry.is_dir()
            || entry
                .unix_mode()
                .is_some_and(|mode| mode & 0o170000 == 0o120000)
            || !seen.insert(entry.name().to_owned())
        {
            return Err("备份包含目录、符号链接或重复条目。".into());
        }
        if entry.name() == INDEX {
            continue;
        }
        let name = entry
            .name()
            .strip_prefix("game/")
            .ok_or("备份条目不在游戏目录。")?;
        let expected = manifest.files.get(name).ok_or("备份条目未在索引中声明。")?;
        if entry.size() != expected.size {
            return Err("备份条目大小与索引不符。".into());
        }
    }
    let preview = BackupPreview {
        name: manifest.name.clone(),
        minecraft: manifest.minecraft.clone(),
        loader: manifest.loader.clone(),
        created_at: manifest.created_at.clone(),
        file_count: manifest.files.len(),
        total_bytes: total,
        archive_sha256: fingerprint,
    };
    Ok(PreparedBackup {
        archive,
        manifest,
        preview,
    })
}

pub fn preview_backup(path: &Path) -> Result<BackupPreview, String> {
    Ok(prepare_backup(path, None)?.preview)
}

/// No overwrite mode exists: the target must be an uninstalled, unused instance.
pub fn restore_backup(mut prepared: PreparedBackup, target: &InstanceRecord) -> Result<(), String> {
    if target.game_dir != target.instance_dir.join("game") {
        return Err("恢复目标必须使用隔离的默认游戏目录。".into());
    }
    if target.state != InstanceState::NotInstalled
        || target.game_version != prepared.manifest.minecraft
        || target.loader != "vanilla"
    {
        return Err("请恢复到相同游戏版本的全新未安装实例。".into());
    }
    if fs::symlink_metadata(&target.instance_dir).is_ok() {
        return Err("恢复目标目录已经存在，不会覆盖任何内容。".into());
    }
    let parent = target.instance_dir.parent().ok_or("实例路径无效。")?;
    fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    ordinary_dir(parent)?;
    let stage = parent.join(format!(".restore-{}", uuid::Uuid::new_v4()));
    fs::create_dir(&stage).map_err(|e| e.to_string())?;
    let _cleanup = Cleanup(stage.clone());
    fs::create_dir(stage.join("game")).map_err(|e| e.to_string())?;
    let mut total = 0;
    for (name, expected) in &prepared.manifest.files {
        let destination = stage.join("game").join(safe_pack_path(name)?);
        fs::create_dir_all(destination.parent().ok_or("恢复路径无效。")?)
            .map_err(|e| e.to_string())?;
        let mut entry = prepared
            .archive
            .by_name(&format!("game/{name}"))
            .map_err(|e| e.to_string())?;
        let mut output = output_file(&destination)?;
        let hash = transfer(&mut entry, &mut output, expected.size, &mut total)?;
        if !hash.eq_ignore_ascii_case(&expected.sha256) {
            return Err("备份文件 SHA-256 校验失败，恢复已取消，原实例保持不变。".into());
        }
        output.sync_all().map_err(|e| e.to_string())?;
    }
    let mut marker = output_file(&stage.join("pack-loader.txt"))?;
    marker
        .write_all(prepared.manifest.loader.as_bytes())
        .map_err(|e| e.to_string())?;
    marker.sync_all().map_err(|e| e.to_string())?;
    drop(marker);
    // Create an exclusive target directory then move contents; rollback only our newly-created target.
    fs::create_dir(&target.instance_dir).map_err(|e| format!("目标已存在或不可写：{e}"))?;
    let result = (|| {
        fs::rename(stage.join("game"), &target.game_dir).map_err(|e| e.to_string())?;
        fs::rename(
            stage.join("pack-loader.txt"),
            target.instance_dir.join("pack-loader.txt"),
        )
        .map_err(|e| e.to_string())?;
        Ok::<(), String>(())
    })();
    if let Err(error) = result {
        let _ = fs::remove_dir_all(&target.instance_dir);
        return Err(error);
    }
    Ok(())
}
