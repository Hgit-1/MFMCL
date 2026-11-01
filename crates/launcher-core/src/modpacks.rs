//! Validated Modrinth pack preview/import and explicitly user-authorized local export.
use crate::{InstanceRecord, InstanceState, loaders::safe_version};
use download_engine::{ArtifactSpec, DownloadEngine, DownloadProgress};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeSet,
    fs::{self, File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
};
use tokio::sync::mpsc;
use zip::{ZipArchive, ZipWriter, write::SimpleFileOptions};

const MAX_ENTRY: u64 = 512 * 1024 * 1024;
const MAX_TOTAL: u64 = 4 * 1024 * 1024 * 1024;
const MAX_INDEX: u64 = 4 * 1024 * 1024;
const MAX_ARCHIVE_ENTRIES: usize = 20_000;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PackIndex {
    pub format_version: u32,
    pub game: String,
    pub version_id: String,
    pub name: String,
    #[serde(default)]
    pub files: Vec<PackFile>,
    pub dependencies: std::collections::HashMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PackFile {
    pub path: String,
    pub hashes: std::collections::HashMap<String, String>,
    pub downloads: Vec<String>,
    pub file_size: u64,
    #[serde(default)]
    pub env: std::collections::HashMap<String, String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PackPreview {
    pub name: String,
    pub version: String,
    pub minecraft: String,
    pub loader: String,
    pub required_files: usize,
    pub optional_files: usize,
    pub total_bytes: u64,
    pub override_files: usize,
}

/// A validated archive kept open between preview metadata and import extraction.
pub struct PreparedPack {
    archive: ZipArchive<File>,
    index: PackIndex,
    preview: PackPreview,
    download_bytes: u64,
}

impl PreparedPack {
    pub fn preview(&self) -> &PackPreview {
        &self.preview
    }
}

pub fn safe_pack_path(value: &str) -> Result<PathBuf, String> {
    // Check both slash conventions on every host, including Windows reserved names.
    if value.is_empty()
        || value.len() > 400
        || value.contains(['\\', ':', '\0'])
        || value.starts_with('/')
    {
        return Err("整合包包含不安全的文件路径。".into());
    }
    for part in value.split('/') {
        let stem = part.split('.').next().unwrap_or("").to_ascii_uppercase();
        if part.is_empty()
            || part == "."
            || part == ".."
            || part.ends_with([' ', '.'])
            || part
                .chars()
                .any(|c| c.is_control() || "<>\"|?*".contains(c))
            || matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
            || ((stem.starts_with("COM") || stem.starts_with("LPT"))
                && stem.len() == 4
                && matches!(stem.as_bytes()[3], b'1'..=b'9'))
        {
            return Err("整合包路径不适用于安全的跨平台安装。".into());
        }
    }
    Ok(PathBuf::from(value))
}

fn eligible_file(path: &str) -> bool {
    [
        "mods/",
        "config/",
        "resourcepacks/",
        "shaderpacks/",
        "defaultconfigs/",
        "kubejs/",
    ]
    .iter()
    .any(|prefix| path.starts_with(prefix))
}

/// `prepare_pack` 的结果。
///
/// CurseForge 格式需要联网补全文件地址，而 `prepare_pack` 是同步且不触网的。
/// 所以它把「待补全」作为一等结果返回，由调用方决定是否继续：
///   - `Ready` —— 可以直接导入（Modrinth，或已补全的 CurseForge）；
///   - `NeedsOnlineLookup` —— CurseForge 格式，需要先查 API 再调
///     [`prepare_from_completed_index`]。
pub enum PackPreparation {
    Ready(PreparedPack),
    NeedsOnlineLookup {
        path: PathBuf,
        manifest: CurseForgeManifest,
    },
}

/// 把查到的 CurseForge 文件详情组装成 `PackFile`。
///
/// 落盘路径按内容类型决定：
///   - `.jar` → `mods/`（绝大多数整合包文件）；
///   - `.zip` 且文件名含 resourcepack 字样 → `resourcepacks/`。
///
/// 其余类型（世界、光影）的归类没有可靠信号：CurseForge 的文件对象不带「这个文件
/// 属于哪一类」的信息，只能靠扩展名与文件名推断。推断错会让文件落到游戏读不到的
/// 位置，所以拿不准时**跳过**并在结果里报告数量，而不是硬塞进 `mods/`——一个放错
/// 的 `.zip` 会让游戏在启动时报错，比少装一个文件更难排查。
pub fn curseforge_files_to_pack_files(
    manifest: &CurseForgeManifest,
    details: &std::collections::HashMap<u64, crate::curseforge::CurseForgeVersion>,
) -> (Vec<PackFile>, usize) {
    let mut out = Vec::new();
    let mut skipped = 0_usize;
    for entry in &manifest.files {
        let Some(version) = details.get(&entry.file_id) else {
            skipped += 1;
            continue;
        };
        let Some(url) = version.url.clone() else {
            // 作者禁止第三方分发：无法自动下载，只能跳过并让调用方提示用户。
            skipped += 1;
            continue;
        };
        let Some(sha1) = version.sha1.clone() else {
            skipped += 1;
            continue;
        };
        let some_path = classify_curseforge_file(&version.file_name);
        let Some(path) = some_path else {
            skipped += 1;
            continue;
        };
        out.push(PackFile {
            path,
            hashes: std::collections::HashMap::from([("sha1".to_owned(), sha1)]),
            downloads: vec![url],
            file_size: version.size,
            // CurseForge 的 `required` 语义与 Modrinth 的 `env.client` 对齐：
            // 必选 → required；可选 → optional（下载但允许缺失）。
            env: std::collections::HashMap::from([(
                "client".to_owned(),
                if entry.required {
                    "required"
                } else {
                    "optional"
                }
                .to_owned(),
            )]),
        });
    }
    (out, skipped)
}

/// 推断一个 CurseForge 文件该放到实例的哪个目录；无法可靠判断时返回 `None`。
fn classify_curseforge_file(file_name: &str) -> Option<String> {
    let lower = file_name.to_ascii_lowercase();
    if !crate::mod_updates::is_safe_mod_name(file_name) {
        // 非 `.jar`：再看是否资源包/光影包。
        if lower.ends_with(".zip") {
            if lower.contains("resourcepack") || lower.contains("resource_pack") {
                return Some(format!("resourcepacks/{file_name}"));
            }
            if lower.contains("shader") {
                return Some(format!("shaderpacks/{file_name}"));
            }
        }
        return None;
    }
    Some(format!("mods/{file_name}"))
}

/// Prepare an archive once so import consumes the same open archive that supplied its metadata.
pub fn prepare_pack(path: &Path) -> Result<PackPreparation, String> {
    let mut file = File::open(path).map_err(|e| e.to_string())?;
    let raw_entry_count = zip_entry_count(&mut file)?;
    if raw_entry_count > MAX_ARCHIVE_ENTRIES as u64 {
        return Err("整合包文件数量超过限制。".into());
    }
    file.seek(SeekFrom::Start(0)).map_err(|e| e.to_string())?;
    let mut archive = ZipArchive::new(file).map_err(|e| e.to_string())?;
    if archive.len() as u64 != raw_entry_count {
        return Err("ZIP 中存在重复路径，或目录条目数量不一致。".into());
    }
    if archive.len() > MAX_ARCHIVE_ENTRIES {
        return Err("整合包文件数量超过限制。".into());
    }
    match read_pack_index_source(&mut archive)? {
        PackIndexSource::Modrinth(index) => {
            Ok(PackPreparation::Ready(prepare_from_index(archive, index)?))
        }
        PackIndexSource::CurseForge(manifest) => Ok(PackPreparation::NeedsOnlineLookup {
            path: path.to_path_buf(),
            manifest,
        }),
    }
}

/// 用已补全的索引再次准备压缩包。
///
/// 调用方拿到 CurseForge 的文件地址后有两条路：重新走 `prepare_pack`（会再次
/// 解析清单，浪费一次 ZIP 扫描），或者把补全好的 `PackIndex` 直接交回来。后者
/// 更直接，也避免中间态差异。
pub fn prepare_from_completed_index(path: &Path, index: PackIndex) -> Result<PreparedPack, String> {
    let mut file = File::open(path).map_err(|e| e.to_string())?;
    let raw_entry_count = zip_entry_count(&mut file)?;
    if raw_entry_count > MAX_ARCHIVE_ENTRIES as u64 {
        return Err("整合包文件数量超过限制。".into());
    }
    file.seek(SeekFrom::Start(0)).map_err(|e| e.to_string())?;
    // 只读长度、不解析条目，因此不需要可变借用。
    let archive = ZipArchive::new(file).map_err(|e| e.to_string())?;
    if archive.len() as u64 != raw_entry_count {
        return Err("ZIP 中存在重复路径，或目录条目数量不一致。".into());
    }
    prepare_from_index(archive, index)
}

fn prepare_from_index(
    mut archive: ZipArchive<File>,
    index: PackIndex,
) -> Result<PreparedPack, String> {
    let (preview, download_bytes, output_paths) = validate_index(&index)?;
    let (zip_bytes, override_files) =
        scan_archive(&mut archive, download_bytes, &output_paths, None)?;
    let mut preview = preview;
    preview.total_bytes = download_bytes
        .checked_add(zip_bytes)
        .ok_or("整合包过大。")?;
    preview.override_files = override_files;
    Ok(PreparedPack {
        archive,
        index,
        preview,
        download_bytes,
    })
}

#[cfg(test)]
pub fn prepare_pack_ready(path: &Path) -> Result<PreparedPack, String> {
    match prepare_pack(path)? {
        PackPreparation::Ready(prepared) => Ok(prepared),
        PackPreparation::NeedsOnlineLookup { .. } => {
            Err("测试只支持 Modrinth 格式的整合包。".to_owned())
        }
    }
}

pub fn preview_pack(path: &Path) -> Result<PackPreview, String> {
    match prepare_pack(path)? {
        PackPreparation::Ready(prepared) => Ok(prepared.preview),
        PackPreparation::NeedsOnlineLookup { manifest, .. } => {
            // 预览在补全前也能给出准确信息：版本、加载器、文件数都来自清单本身，
            // 只有总字节数必须等查到每个文件才知道。
            Ok(CurseForgeManifest::to_preview(&manifest))
        }
    }
}

/// Count central-directory records before zip parses them, since the zip crate stores names in a map.
pub(crate) fn zip_entry_count(file: &mut File) -> Result<u64, String> {
    let length = file.metadata().map_err(|e| e.to_string())?.len();
    if length < 22 {
        return Err("ZIP 文件过短或缺少结束目录。".into());
    }
    let tail_length = length.min(22 + u16::MAX as u64) as usize;
    file.seek(SeekFrom::End(-(tail_length as i64)))
        .map_err(|e| e.to_string())?;
    let mut tail = vec![0; tail_length];
    file.read_exact(&mut tail).map_err(|e| e.to_string())?;
    let eocd = tail
        .windows(4)
        .enumerate()
        .rev()
        .find_map(|(index, signature)| {
            if signature == b"PK\x05\x06" && index + 22 <= tail.len() {
                let comment_length =
                    u16::from_le_bytes([tail[index + 20], tail[index + 21]]) as usize;
                (index + 22 + comment_length == tail.len()).then_some(index)
            } else {
                None
            }
        })
        .ok_or("ZIP 结束目录无效。")?;
    let disk = u16::from_le_bytes([tail[eocd + 4], tail[eocd + 5]]);
    let central_disk = u16::from_le_bytes([tail[eocd + 6], tail[eocd + 7]]);
    let disk_count = u16::from_le_bytes([tail[eocd + 8], tail[eocd + 9]]);
    let total_count = u16::from_le_bytes([tail[eocd + 10], tail[eocd + 11]]);
    if disk != 0 || central_disk != 0 {
        return Err("不支持多卷 ZIP。".into());
    }
    if total_count != u16::MAX {
        if disk_count != total_count {
            return Err("ZIP 目录条目数量不一致。".into());
        }
        return Ok(total_count as u64);
    }

    let eocd_offset = length - tail_length as u64 + eocd as u64;
    if eocd_offset < 20 {
        return Err("ZIP64 定位信息缺失。".into());
    }
    file.seek(SeekFrom::Start(eocd_offset - 20))
        .map_err(|e| e.to_string())?;
    let mut locator = [0_u8; 20];
    file.read_exact(&mut locator).map_err(|e| e.to_string())?;
    if &locator[..4] != b"PK\x06\x07"
        || u32::from_le_bytes(locator[4..8].try_into().unwrap()) != 0
        || u32::from_le_bytes(locator[16..20].try_into().unwrap()) != 1
    {
        return Err("ZIP64 定位信息无效或使用多卷格式。".into());
    }
    let zip64_offset = u64::from_le_bytes(locator[8..16].try_into().unwrap());
    file.seek(SeekFrom::Start(zip64_offset))
        .map_err(|e| e.to_string())?;
    let mut zip64 = [0_u8; 56];
    file.read_exact(&mut zip64).map_err(|e| e.to_string())?;
    if &zip64[..4] != b"PK\x06\x06"
        || u64::from_le_bytes(zip64[4..12].try_into().unwrap()) < 44
        || u32::from_le_bytes(zip64[16..20].try_into().unwrap()) != 0
        || u32::from_le_bytes(zip64[20..24].try_into().unwrap()) != 0
    {
        return Err("ZIP64 结束目录无效。".into());
    }
    let disk_count = u64::from_le_bytes(zip64[24..32].try_into().unwrap());
    let total_count = u64::from_le_bytes(zip64[32..40].try_into().unwrap());
    if disk_count != total_count {
        return Err("ZIP64 目录条目数量不一致。".into());
    }
    Ok(total_count)
}

/// `read_pack_index` 的结果：要么是可直接使用的 Modrinth 索引，要么是需要联网
/// 补全的 CurseForge 清单。
///
/// 用枚举而不是「一个可能为空的 PackIndex」，是为了让调用方在类型层面被要求处理
/// CurseForge 分支——否则很容易只改 Modrinth 路径而忘了另一边仍然返回占位错误。
pub enum PackIndexSource {
    Modrinth(PackIndex),
    CurseForge(CurseForgeManifest),
}

fn read_pack_index_source(archive: &mut ZipArchive<File>) -> Result<PackIndexSource, String> {
    /*
     * 两种索引格式，按入口文件区分：
     *   - `modrinth.index.json` —— Modrinth 的 `.mrpack`，自描述，可直接用；
     *   - `manifest.json`       —— CurseForge 的整合包，需要联网补全文件地址。
     *
     * 先试 Modrinth（更常见且无需网络），再试 CurseForge，都不匹配时明确说明
     * 「这不是有效的整合包」，而不是报「缺少 modrinth.index.json」——后者只有
     * 开发者能读懂。
     */
    if let Some(index) = try_read_modrinth_index(archive)? {
        return Ok(PackIndexSource::Modrinth(index));
    }
    if let Some(manifest) = try_read_curseforge_manifest(archive)? {
        return Ok(PackIndexSource::CurseForge(manifest));
    }
    Err(
        "这不是有效的整合包：压缩包里既没有 Modrinth 的 modrinth.index.json，\
         也没有 CurseForge 的 manifest.json。"
            .into(),
    )
}

/// CurseForge 的 `manifest.json` 原始结构。
///
/// 它只列出 `projectID` / `fileID`，**不含下载地址与校验值**——那些要再用
/// CurseForge API 按 fileID 查回来。所以解析分成两步：这里只负责把本地文件读成
/// 结构，补全 URL/哈希由调用方异步完成。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CurseForgeManifest {
    pub minecraft: CurseForgeManifestMinecraft,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub author: String,
    #[serde(default)]
    pub files: Vec<CurseForgeManifestFile>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CurseForgeManifestMinecraft {
    pub version: String,
    #[serde(default)]
    pub mod_loaders: Vec<CurseForgeManifestLoader>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CurseForgeManifestLoader {
    /// `forge` / `neoforge` / `fabric` / `quilt`。
    pub id: String,
    #[serde(default)]
    pub primary: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CurseForgeManifestFile {
    #[serde(rename = "projectID")]
    pub project_id: u64,
    #[serde(rename = "fileID")]
    pub file_id: u64,
    #[serde(default)]
    pub required: bool,
}

/// 读完 CurseForge 的 `manifest.json`；不存在时返回 `Ok(None)`。
///
/// 只做到「读出结构」为止。补全文件地址需要网络，而 `prepare_pack` 是同步且
/// 不触网的（要求它在无网络时也能给出可预期的错误），所以补全放在调用方。
fn try_read_curseforge_manifest(
    archive: &mut ZipArchive<File>,
) -> Result<Option<CurseForgeManifest>, String> {
    let mut entry = match archive.by_name("manifest.json") {
        Ok(entry) => entry,
        Err(zip::result::ZipError::FileNotFound) => return Ok(None),
        Err(_) => return Err("读取整合包索引失败。".into()),
    };
    if entry.is_dir() {
        return Err("整合包索引不是普通文件。".into());
    }
    if entry.size() > MAX_INDEX {
        return Err("整合包索引过大。".into());
    }
    let mut bytes = Vec::with_capacity(entry.size() as usize);
    let mut total = 0;
    let declared = entry.size();
    copy_bounded(&mut entry, &mut bytes, declared, MAX_INDEX, &mut total)?;
    let manifest: CurseForgeManifest =
        serde_json::from_slice(&bytes).map_err(|_| "CurseForge 整合包清单格式错误。".to_owned())?;
    // 与 Modrinth 侧一致的边界检查：条目数与版本号长度都必须有限。
    if manifest.files.len() > MAX_ARCHIVE_ENTRIES {
        return Err("整合包索引文件数量超过限制。".into());
    }
    if manifest.minecraft.version.trim().is_empty() {
        return Err("整合包未声明 Minecraft 版本。".into());
    }
    safe_version(&manifest.minecraft.version)?;
    Ok(Some(manifest))
}

impl CurseForgeManifest {
    /// 补全前就能给出的预览信息。
    ///
    /// 版本、加载器、文件数都来自清单本身，只有总字节数必须等查到每个文件才知道——
    /// 所以这里的 `total_bytes` 是 0，界面据此显示「未知」而不是「0 字节」。
    pub fn to_preview(&self) -> PackPreview {
        let mut required = 0;
        let mut optional = 0;
        for file in &self.files {
            if file.required {
                required += 1;
            } else {
                optional += 1;
            }
        }
        PackPreview {
            name: if self.name.trim().is_empty() {
                "CurseForge 整合包".to_owned()
            } else {
                self.name.trim().to_owned()
            },
            version: self.version.trim().to_owned(),
            minecraft: self.minecraft.version.trim().to_owned(),
            loader: self.loader_string(),
            required_files: required,
            optional_files: optional,
            total_bytes: 0,
            override_files: 0,
        }
    }

    /// 清单声明的加载器，形如 `fabric@0.16.10`；没有或无法识别时回到 `vanilla`。
    ///
    /// 优先取 `primary: true` 的那条——一个整合包可能同时声明多个加载器的兼容性，
    /// 但真正安装用的是主加载器。
    fn loader_string(&self) -> String {
        let chosen = self
            .minecraft
            .mod_loaders
            .iter()
            .find(|loader| loader.primary)
            .or_else(|| self.minecraft.mod_loaders.first());
        let Some(loader) = chosen else {
            return "vanilla".to_owned();
        };
        let id = loader.id.trim();
        // 清单里常见的写法是 `fabric-0.16.10` / `forge-47.2.0`，拆成
        // `fabric@0.16.10` 以匹配实例记录里 loader 字段的既有格式。
        let (kind, version) = match id.split_once('-') {
            Some((kind, version)) => (kind, version),
            None => (id, ""),
        };
        let kind = kind.to_ascii_lowercase();
        let canonical = match kind.as_str() {
            "fabric" => "fabric",
            "quilt" => "quilt",
            "forge" => "forge",
            "neoforge" => "neoforge",
            _ => return "vanilla".to_owned(),
        };
        if version.is_empty() {
            canonical.to_owned()
        } else {
            format!("{canonical}@{version}")
        }
    }

    /// 把清单本身能确定的部分转成 `PackIndex` 骨架。
    ///
    /// `files` 留空——它们需要联网按 fileID 查询后才能填，见 [`Self::to_pack_index`]。
    /// 这样转换逻辑只有一份，且「还没补全」是一个类型上可见的中间态。
    pub fn to_index_skeleton(&self) -> PackIndex {
        let mut dependencies = std::collections::HashMap::new();
        dependencies.insert(
            "minecraft".to_owned(),
            self.minecraft.version.trim().to_owned(),
        );
        // Modrinth 的 `PackIndex` 用 `fabric-loader` / `quilt-loader` 作为键。
        // CurseForge 的 `forge` / `neoforge` 在 Modrinth 里没有对应键——它们由
        // `PackIndex` 之外的安装流程处理，所以这里只映射 loader 家族。
        let loader = self.loader_string();
        if let Some((kind, version)) = loader.split_once('@') {
            match kind {
                "fabric" => {
                    dependencies.insert("fabric-loader".to_owned(), version.to_owned());
                }
                "quilt" => {
                    dependencies.insert("quilt-loader".to_owned(), version.to_owned());
                }
                _ => {}
            }
        }
        PackIndex {
            format_version: 1,
            game: "minecraft".to_owned(),
            version_id: self.version.trim().to_owned(),
            name: self.to_preview().name,
            files: Vec::new(),
            dependencies,
        }
    }

    /// 把清单转成 `PackIndex`，并填入已补全的文件。
    ///
    /// `files` 由调用方在查到每个 fileID 的下载地址与哈希后组装。
    pub fn to_pack_index(&self, files: Vec<PackFile>) -> PackIndex {
        let mut index = self.to_index_skeleton();
        index.files = files;
        index
    }

    /// 需要查询详情清单里不会给出的部分：fileID 列表（去重、保序）。
    /// 去重是必要的：清单里同一个 fileID 出现两次是常见的手误，而重复提交会让
    /// 服务端返回重复条目、进而让 `files` 里出现同路径两次，被 `validate_index`
    /// 当成「索引路径重复」拒掉。
    pub fn file_ids(&self) -> Vec<u64> {
        let mut seen = std::collections::BTreeSet::new();
        self.files
            .iter()
            .map(|file| file.file_id)
            .filter(|id| seen.insert(*id))
            .collect()
    }
}

/// 读 Modrinth 的 `modrinth.index.json`；不存在时返回 `Ok(None)`。
fn try_read_modrinth_index(archive: &mut ZipArchive<File>) -> Result<Option<PackIndex>, String> {
    let mut entry = match archive.by_name("modrinth.index.json") {
        Ok(entry) => entry,
        Err(zip::result::ZipError::FileNotFound) => return Ok(None),
        Err(_) => return Err("读取整合包索引失败。".into()),
    };
    if entry.is_dir() {
        return Err("整合包索引不是普通文件。".into());
    }
    if entry.size() > MAX_INDEX {
        return Err("整合包索引过大。".into());
    }
    let mut bytes = Vec::with_capacity(entry.size() as usize);
    let mut total = 0;
    let declared = entry.size();
    copy_bounded(&mut entry, &mut bytes, declared, MAX_INDEX, &mut total)?;
    Ok(Some(
        serde_json::from_slice(&bytes).map_err(|_| "整合包索引格式错误。".to_owned())?,
    ))
}

fn validate_index(index: &PackIndex) -> Result<(PackPreview, u64, PathRegistry), String> {
    if index.format_version != 1
        || index.game != "minecraft"
        || index.name.trim().is_empty()
        || index.name.chars().count() > 128
        || index.name.chars().any(char::is_control)
    {
        return Err("仅支持 Minecraft Modrinth 格式版本 1。".into());
    }
    if index.version_id.len() > 1024 || index.version_id.chars().any(char::is_control) {
        return Err("整合包版本标识无效。".into());
    }
    let minecraft = index
        .dependencies
        .get("minecraft")
        .ok_or("整合包未声明 Minecraft 版本。")?
        .clone();
    safe_version(&minecraft)?;
    let mut loader = "vanilla".to_owned();
    for (kind, version) in &index.dependencies {
        safe_version(version)?;
        match kind.as_str() {
            "minecraft" => {}
            "fabric-loader" | "quilt-loader" => {
                if loader != "vanilla" {
                    return Err("整合包声明了多个加载器。".into());
                }
                loader = format!(
                    "{}@{version}",
                    if kind == "fabric-loader" {
                        "fabric"
                    } else {
                        "quilt"
                    }
                );
            }
            _ => {
                return Err(format!("暂不支持整合包依赖 {kind}，不会创建不完整实例。"));
            }
        }
    }

    if index.files.len() > MAX_ARCHIVE_ENTRIES {
        return Err("整合包索引文件数量超过限制。".into());
    }
    let mut required = 0;
    let mut optional = 0;
    let mut download_bytes = 0_u64;
    let mut output_paths = PathRegistry::default();
    for file in &index.files {
        safe_pack_path(&file.path)?;
        if !eligible_file(&file.path) {
            return Err("索引路径重复或试图写入不允许的游戏文件。".into());
        }
        output_paths.insert(&file.path, false)?;
        let client_env = file
            .env
            .get("client")
            .map(String::as_str)
            .unwrap_or("required");
        match client_env {
            "unsupported" => continue,
            "optional" => {
                optional += 1;
                continue;
            }
            "required" => required += 1,
            _ => return Err("未知的整合包客户端依赖类型。".into()),
        }
        if file.file_size == 0 || file.file_size > MAX_ENTRY {
            return Err("整合包依赖文件大小无效。".into());
        }
        download_bytes = download_bytes
            .checked_add(file.file_size)
            .filter(|size| *size <= MAX_TOTAL)
            .ok_or("整合包过大。")?;
        validate_file_hashes(&file.hashes)?;
        if file.downloads.is_empty() || file.downloads.len() > 8 {
            return Err("依赖下载地址数量无效。".into());
        }
        for value in &file.downloads {
            if value.len() > 2048 {
                return Err("下载地址过长。".into());
            }
            let url = reqwest::Url::parse(value).map_err(|_| "下载地址无效。")?;
            if url.scheme() != "https"
                || !url.username().is_empty()
                || url.password().is_some()
                || !matches!(
                    url.host_str(),
                    Some(
                        "cdn.modrinth.com"
                            | "github.com"
                            | "raw.githubusercontent.com"
                            | "objects.githubusercontent.com"
                            // CurseForge 格式的整合包由 `curseforge_files_to_pack_files`
                            // 组装，地址只来自这两个官方分发域名（上游 `parse_files`
                            // 已经做过同样的白名单校验）。不放进这里的话，CurseForge
                            // 整合包会在 `validate_index` 被拒。
                            | "edge.forgecdn.net"
                            | "mediafilez.forgecdn.net"
                    )
                )
            {
                return Err("整合包引用了未获准的下载域名。".into());
            }
        }
    }
    let preview = PackPreview {
        name: index.name.clone(),
        version: index.version_id.clone(),
        minecraft,
        loader,
        required_files: required,
        optional_files: optional,
        total_bytes: 0,
        override_files: 0,
    };
    Ok((preview, download_bytes, output_paths))
}

fn validate_file_hashes(hashes: &std::collections::HashMap<String, String>) -> Result<(), String> {
    let mut supported = false;
    for (kind, expected_length) in [("sha1", 40), ("sha256", 64)] {
        if let Some(value) = hashes.get(kind) {
            supported = true;
            if value.len() != expected_length || !value.chars().all(|c| c.is_ascii_hexdigit()) {
                return Err("依赖校验值无效。".into());
            }
        }
    }
    if supported {
        Ok(())
    } else {
        Err("依赖缺少支持的校验值（SHA-1/SHA-256）。".into())
    }
}

#[derive(Default, Clone)]
struct PathRegistry {
    all: BTreeSet<String>,
    files: BTreeSet<String>,
    spelling: std::collections::BTreeMap<String, String>,
}

impl PathRegistry {
    /// Register a file or directory using case-insensitive path comparisons.
    fn insert(&mut self, path: &str, is_dir: bool) -> Result<(), String> {
        let key = path.to_lowercase();
        let mut original = String::new();
        for component in path.split('/') {
            if !original.is_empty() {
                original.push('/');
            }
            original.push_str(component);
            let folded = original.to_lowercase();
            if self
                .spelling
                .get(&folded)
                .is_some_and(|previous| previous != &original)
            {
                return Err("路径存在大小写别名冲突。".into());
            }
            self.spelling.insert(folded, original.clone());
        }
        if key.is_empty() || self.all.contains(&key) {
            return Err("路径重复或存在大小写冲突。".into());
        }
        let mut ancestor = key.as_str();
        while let Some((parent, _)) = ancestor.rsplit_once('/') {
            if self.files.contains(parent) {
                return Err("文件路径与目录路径冲突。".into());
            }
            ancestor = parent;
        }
        if !is_dir {
            let child_prefix = format!("{key}/");
            if self
                .all
                .range(child_prefix.clone()..)
                .next()
                .is_some_and(|child| child.starts_with(&child_prefix))
            {
                return Err("文件路径与目录路径冲突。".into());
            }
            self.files.insert(key.clone());
        }
        self.all.insert(key);
        Ok(())
    }
}

#[derive(Debug)]
enum ArchiveEntryKind {
    Index,
    Override { relative: String, client: bool },
    OverrideDirectory(String),
    Ignored,
}

fn archive_entry_kind(name: &str, is_dir: bool) -> Result<ArchiveEntryKind, String> {
    let logical_name = if is_dir && !name.ends_with('/') {
        format!("{name}/")
    } else {
        name.to_owned()
    };
    if !is_dir && logical_name == "modrinth.index.json" {
        return Ok(ArchiveEntryKind::Index);
    }
    for prefix in ["overrides/", "client-overrides/", "server-overrides/"] {
        if let Some(relative) = logical_name.strip_prefix(prefix) {
            let relative = if is_dir {
                relative.strip_suffix('/').unwrap_or(relative)
            } else {
                relative
            };
            if relative.is_empty() {
                return Ok(ArchiveEntryKind::Ignored);
            }
            safe_pack_path(relative)?;
            if prefix == "server-overrides/" {
                return Ok(ArchiveEntryKind::Ignored);
            }
            if is_dir {
                return Ok(ArchiveEntryKind::OverrideDirectory(relative.to_owned()));
            }
            if !eligible_file(relative) {
                return Err(format!("覆盖文件 {relative} 不在允许的配置/模组目录中。"));
            }
            return Ok(ArchiveEntryKind::Override {
                relative: relative.to_owned(),
                client: prefix == "client-overrides/",
            });
        }
    }
    if is_dir {
        return Err("整合包包含不支持的目录。".into());
    }
    Err("整合包包含不支持的顶层文件。".into())
}

/// Read and optionally extract every archive file while enforcing actual streamed byte counts.
fn scan_archive(
    archive: &mut ZipArchive<File>,
    download_bytes: u64,
    output_paths: &PathRegistry,
    output_dir: Option<&Path>,
) -> Result<(u64, usize), String> {
    if archive.len() > MAX_ARCHIVE_ENTRIES {
        return Err("整合包文件数量超过限制。".into());
    }
    let mut archive_paths = PathRegistry::default();
    let mut destinations = output_paths.clone();
    let mut total = download_bytes;
    let mut zip_bytes = 0_u64;
    let mut override_files = 0;
    let mut override_origins = std::collections::HashMap::<String, (String, bool)>::new();
    // Client overrides may replace an exact generic path, never case aliases or index downloads.
    let mut indices = Vec::with_capacity(archive.len());
    for index in 0..archive.len() {
        let entry = archive.by_index(index).map_err(|e| e.to_string())?;
        indices.push((entry.name().starts_with("client-overrides/"), index));
    }
    indices.sort_unstable();

    for (_, index) in indices {
        let mut entry = archive.by_index(index).map_err(|e| e.to_string())?;
        let name = entry.name().to_owned();
        let is_dir = entry.is_dir();
        let normalized_name = if is_dir {
            name.strip_suffix('/').unwrap_or(&name)
        } else {
            name.as_str()
        };
        safe_pack_path(normalized_name)?;
        if entry
            .unix_mode()
            .is_some_and(|mode| mode & 0o170000 == 0o120000)
        {
            return Err("整合包不允许包含符号链接。".into());
        }
        archive_paths.insert(normalized_name, is_dir)?;
        let kind = archive_entry_kind(&name, is_dir)?;
        let declared = entry.size();
        if declared > MAX_ENTRY {
            return Err("ZIP 文件超过单文件大小限制。".into());
        }
        if is_dir && declared != 0 {
            return Err("ZIP 目录包含文件数据。".into());
        }
        if total
            .checked_add(declared)
            .is_none_or(|size| size > MAX_TOTAL)
        {
            return Err("整合包展开与下载总大小超过 4 GB。".into());
        }

        match kind {
            ArchiveEntryKind::Override { relative, client } => {
                let key = relative.to_lowercase();
                let replace_existing =
                    if let Some((prior_path, prior_client)) = override_origins.get(&key) {
                        if prior_path != &relative || *prior_client || !client {
                            return Err("覆盖文件路径重复或存在大小写冲突。".into());
                        }
                        true
                    } else {
                        destinations.insert(&relative, false)?;
                        override_origins.insert(key, (relative.clone(), client));
                        override_files += 1;
                        false
                    };
                if let Some(root) = output_dir {
                    let target = root.join(safe_pack_path(&relative)?);
                    if let Some(parent) = target.parent() {
                        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
                    }
                    let mut options = OpenOptions::new();
                    options.write(true);
                    if replace_existing {
                        options.truncate(true);
                    } else {
                        options.create_new(true);
                    }
                    let mut output = options.open(&target).map_err(|e| e.to_string())?;
                    let copied =
                        copy_bounded(&mut entry, &mut output, declared, MAX_ENTRY, &mut total)?;
                    zip_bytes = zip_bytes.checked_add(copied).ok_or("整合包过大。")?;
                    output.flush().map_err(|e| e.to_string())?;
                } else {
                    let copied = copy_bounded(
                        &mut entry,
                        &mut std::io::sink(),
                        declared,
                        MAX_ENTRY,
                        &mut total,
                    )?;
                    zip_bytes = zip_bytes.checked_add(copied).ok_or("整合包过大。")?;
                }
            }
            ArchiveEntryKind::OverrideDirectory(relative) => {
                // Empty directories do not get extracted, but must not alias files or casing.
                let key = relative.to_lowercase();
                if !destinations.all.contains(&key) {
                    destinations.insert(&relative, true)?;
                } else if destinations.files.contains(&key)
                    || destinations.spelling.get(&key) != Some(&relative)
                {
                    return Err("覆盖目录与文件路径冲突。".into());
                }
                copy_bounded(
                    &mut entry,
                    &mut std::io::sink(),
                    declared,
                    MAX_ENTRY,
                    &mut total,
                )?;
            }
            ArchiveEntryKind::Index | ArchiveEntryKind::Ignored => {
                let copied = copy_bounded(
                    &mut entry,
                    &mut std::io::sink(),
                    declared,
                    MAX_ENTRY,
                    &mut total,
                )?;
                zip_bytes = zip_bytes.checked_add(copied).ok_or("整合包过大。")?;
            }
        }
    }
    Ok((zip_bytes, override_files))
}

fn copy_bounded<R: Read, W: Write>(
    reader: &mut R,
    writer: &mut W,
    declared: u64,
    entry_limit: u64,
    total: &mut u64,
) -> Result<u64, String> {
    let mut copied = 0_u64;
    let mut buffer = [0_u8; 32 * 1024];
    loop {
        let read = reader.read(&mut buffer).map_err(|e| e.to_string())?;
        if read == 0 {
            break;
        }
        let read = read as u64;
        copied = copied.checked_add(read).ok_or("ZIP 文件大小溢出。")?;
        if copied > declared || copied > entry_limit {
            return Err("ZIP 实际展开大小超过声明或单文件限制。".into());
        }
        *total = total.checked_add(read).ok_or("整合包大小溢出。")?;
        if *total > MAX_TOTAL {
            return Err("整合包展开与下载总大小超过 4 GB。".into());
        }
        writer
            .write_all(&buffer[..read as usize])
            .map_err(|e| e.to_string())?;
    }
    if copied != declared {
        return Err("ZIP 实际展开大小与声明不一致。".into());
    }
    Ok(copied)
}

pub async fn import_pack(
    path: &Path,
    instance: &InstanceRecord,
    engine: &DownloadEngine,
    progress: Option<mpsc::UnboundedSender<DownloadProgress>>,
) -> Result<PackPreview, String> {
    let prepared = match prepare_pack(path)? {
        PackPreparation::Ready(prepared) => prepared,
        PackPreparation::NeedsOnlineLookup { manifest, .. } => {
            /*
             * 这条入口不做联网补全：它没有 CurseForge key，也没有进度回调的归属。
             * 需要它的人应改用 `prepare_pack` 拿到清单、自行补全后再调
             * `prepare_from_completed_index` + `import_prepared_pack`。
             */
            let count = manifest.files.len();
            return Err(format!(
                "这是 CurseForge 格式的整合包（含 {count} 个文件引用），\
                 需要联网查询下载地址后才能导入。请使用启动器内的「下载并导入」，\
                 或改用 Modrinth 的 .mrpack。"
            ));
        }
    };
    import_prepared_pack(prepared, instance, engine, progress).await
}

pub async fn import_prepared_pack(
    mut prepared: PreparedPack,
    instance: &InstanceRecord,
    engine: &DownloadEngine,
    progress: Option<mpsc::UnboundedSender<DownloadProgress>>,
) -> Result<PackPreview, String> {
    if instance.game_version != prepared.preview.minecraft {
        return Err("整合包版本与刚创建的实例不一致；请重新预览并导入。".into());
    }
    if instance.state != InstanceState::NotInstalled {
        return Err("整合包只能导入新实例，现有世界不会被覆盖。".into());
    }
    if instance.game_dir.parent() != Some(instance.instance_dir.as_path())
        || instance
            .game_dir
            .file_name()
            .is_none_or(|name| name != "game")
    {
        return Err("实例目录结构无效。".into());
    }
    match fs::symlink_metadata(&instance.game_dir) {
        Ok(_) => return Err("整合包只能导入新实例，现有世界不会被覆盖。".into()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.to_string()),
    }

    if let Some(parent) = instance.instance_dir.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let mut created_instance_dir = false;
    match fs::symlink_metadata(&instance.instance_dir) {
        Ok(metadata) if metadata.file_type().is_dir() && !metadata.file_type().is_symlink() => {}
        Ok(_) => return Err("实例目录不能是符号链接或普通文件。".into()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir(&instance.instance_dir).map_err(|e| e.to_string())?;
            created_instance_dir = true;
        }
        Err(error) => return Err(error.to_string()),
    }
    let metadata = fs::symlink_metadata(&instance.instance_dir).map_err(|e| e.to_string())?;
    if !metadata.file_type().is_dir() || metadata.file_type().is_symlink() {
        return Err("实例目录不能是符号链接或普通文件。".into());
    }

    let staging = instance
        .instance_dir
        .join(format!(".import-{}", uuid::Uuid::new_v4()));
    fs::create_dir(&staging).map_err(|e| e.to_string())?;
    let mut metadata_published = false;
    let result = async {
        for file in &prepared.index.files {
            if matches!(
                file.env.get("client").map(String::as_str),
                Some("optional" | "unsupported")
            ) {
                continue;
            }
            let destination = staging.join(safe_pack_path(&file.path)?);
            let mut success = false;
            let mut failure = String::new();
            for url in &file.downloads {
                let spec = ArtifactSpec {
                    id: format!("pack:{}", file.path),
                    label: file.path.clone(),
                    url: url.clone(),
                    mirror_url: None,
                    destination: destination.clone(),
                    size: Some(file.file_size),
                    sha1: file.hashes.get("sha1").cloned(),
                    sha256: file.hashes.get("sha256").cloned(),
                };
                match engine.download(&spec, progress.clone()).await {
                    Ok(_) => {
                        success = true;
                        break;
                    }
                    Err(error) => failure = error.to_string(),
                }
            }
            if !success {
                return Err(format!("整合包依赖下载失败：{failure}"));
            }
        }

        let (_, override_files) = scan_archive(
            &mut prepared.archive,
            prepared.download_bytes,
            &validate_index(&prepared.index)?.2,
            Some(&staging),
        )?;
        if override_files != prepared.preview.override_files {
            return Err("整合包覆盖文件在导入期间发生变化。".into());
        }
        let index_path = staging.join("modrinth.index.json");
        let mut index_file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(index_path)
            .map_err(|e| e.to_string())?;
        index_file
            .write_all(&serde_json::to_vec_pretty(&prepared.index).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
        index_file.sync_all().map_err(|e| e.to_string())?;
        drop(index_file);
        fs::hard_link(
            staging.join("modrinth.index.json"),
            instance.instance_dir.join("modrinth.index.json"),
        )
        .map_err(|e| e.to_string())?;
        metadata_published = true;
        fs::remove_file(staging.join("modrinth.index.json")).map_err(|e| e.to_string())?;
        match fs::symlink_metadata(&instance.game_dir) {
            Ok(_) => return Err("目标游戏目录已存在；没有覆盖任何文件。".into()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.to_string()),
        }
        fs::rename(&staging, &instance.game_dir).map_err(|e| e.to_string())?;
        Ok(prepared.preview)
    }
    .await;

    if let Err(mut error) = result {
        if metadata_published
            && let Err(rollback) =
                fs::remove_file(instance.instance_dir.join("modrinth.index.json"))
        {
            error = format!("{error}；回滚整合包索引失败：{rollback}");
        }
        if let Err(cleanup) = fs::remove_dir_all(&staging)
            && cleanup.kind() != std::io::ErrorKind::NotFound
        {
            return Err(format!("{error}；清理导入临时文件失败：{cleanup}"));
        }
        if created_instance_dir {
            match fs::remove_dir(&instance.instance_dir) {
                Ok(()) => {}
                Err(cleanup) if cleanup.kind() == std::io::ErrorKind::NotFound => {}
                Err(cleanup) => error = format!("{error}；回滚新建实例目录失败：{cleanup}"),
            }
        }
        return Err(error);
    }
    result
}

/// Local files are included only after caller confirms redistribution rights. Saves, tokens and launcher settings are never exported.
pub fn export_pack(instance: &InstanceRecord, directory: &Path) -> Result<PathBuf, String> {
    let mut dependencies =
        std::collections::HashMap::from([("minecraft".to_owned(), instance.game_version.clone())]);
    if instance.loader != "vanilla" {
        let (kind, version) = instance.loader.split_once('@').ok_or("加载器记录无效。")?;
        safe_version(version)?;
        let key = match kind {
            "fabric" => "fabric-loader",
            "quilt" => "quilt-loader",
            _ => return Err("暂不支持导出此加载器。".into()),
        };
        dependencies.insert(key.into(), version.into());
    }
    safe_version(&instance.game_version)?;
    if instance.name.trim().is_empty()
        || instance.name.chars().count() > 128
        || instance.name.chars().any(char::is_control)
    {
        return Err("实例名称无效，无法导出整合包。".into());
    }
    if !directory.is_dir() {
        return Err("请选择存在的导出文件夹。".into());
    }
    let game_metadata = fs::symlink_metadata(&instance.game_dir).map_err(|e| e.to_string())?;
    if game_metadata.file_type().is_symlink() || !game_metadata.file_type().is_dir() {
        return Err("游戏目录不能是符号链接或普通文件。".into());
    }
    let target = directory.join(format!("MFMCL-pack-{}.mrpack", uuid::Uuid::new_v4()));
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&target)
        .map_err(|e| e.to_string())?;
    let result = (|| {
        let mut writer = ZipWriter::new(file);
        let options = SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated)
            .unix_permissions(0o600);
        let index = PackIndex {
            format_version: 1,
            game: "minecraft".into(),
            version_id: "1.0.0".into(),
            name: instance.name.clone(),
            files: vec![],
            dependencies,
        };
        writer
            .start_file("modrinth.index.json", options)
            .map_err(|e| e.to_string())?;
        let index_bytes = serde_json::to_vec_pretty(&index).map_err(|e| e.to_string())?;
        writer.write_all(&index_bytes).map_err(|e| e.to_string())?;
        let mut files = Vec::new();
        let mut paths = PathRegistry::default();
        for folder in [
            "mods",
            "config",
            "resourcepacks",
            "shaderpacks",
            "defaultconfigs",
            "kubejs",
        ] {
            collect_export_files(
                &instance.game_dir,
                &instance.game_dir.join(folder),
                &mut files,
                &mut paths,
                0,
            )?;
        }
        let mut total = index_bytes.len() as u64;
        for (path, relative) in files {
            let metadata = fs::symlink_metadata(&path).map_err(|e| e.to_string())?;
            if metadata.file_type().is_symlink() || !metadata.file_type().is_file() {
                return Err("导出目录包含符号链接或非普通文件。".into());
            }
            if metadata.len() > MAX_ENTRY {
                return Err("导出超过大小限制。".into());
            }
            if total
                .checked_add(metadata.len())
                .is_none_or(|size| size > MAX_TOTAL)
            {
                return Err("导出超过大小限制。".into());
            }
            writer
                .start_file(format!("overrides/{relative}"), options)
                .map_err(|e| e.to_string())?;
            let mut source = File::open(path).map_err(|e| e.to_string())?;
            let mut copied = 0_u64;
            let mut buffer = [0_u8; 32 * 1024];
            loop {
                let read = source.read(&mut buffer).map_err(|e| e.to_string())?;
                if read == 0 {
                    break;
                }
                copied = copied
                    .checked_add(read as u64)
                    .filter(|size| *size <= MAX_ENTRY)
                    .ok_or("导出文件实际大小超过限制。")?;
                total = total
                    .checked_add(read as u64)
                    .filter(|size| *size <= MAX_TOTAL)
                    .ok_or("导出超过大小限制。")?;
                writer
                    .write_all(&buffer[..read])
                    .map_err(|e| e.to_string())?;
            }
            if copied != metadata.len() {
                return Err("导出文件在读取期间发生变化。".into());
            }
        }
        writer.finish().map_err(|e| e.to_string())?;
        Ok(target.clone())
    })();
    if let Err(error) = &result
        && let Err(cleanup) = fs::remove_file(&target)
    {
        return Err(format!("{error}；清理未完成导出失败：{cleanup}"));
    }
    result
}

fn collect_export_files(
    root: &Path,
    dir: &Path,
    files: &mut Vec<(PathBuf, String)>,
    paths: &mut PathRegistry,
    depth: u8,
) -> Result<(), String> {
    let metadata = match fs::symlink_metadata(dir) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.to_string()),
    };
    if metadata.file_type().is_symlink() || !metadata.file_type().is_dir() {
        return Err("导出目录包含符号链接或非普通目录。".into());
    }
    if depth > 16 {
        return Err("导出目录层级过深。".into());
    }
    for entry in fs::read_dir(dir).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let path = entry.path();
        let kind = entry.file_type().map_err(|e| e.to_string())?;
        if kind.is_symlink() {
            return Err("导出目录包含符号链接。".into());
        }
        if kind.is_dir() {
            collect_export_files(root, &path, files, paths, depth + 1)?;
        } else if kind.is_file() {
            let relative = path
                .strip_prefix(root)
                .map_err(|e| e.to_string())?
                .to_str()
                .ok_or("导出文件名不是有效的 UTF-8。")?
                .to_owned();
            #[cfg(windows)]
            let relative = relative.replace('\\', "/");
            safe_pack_path(&relative)?;
            paths.insert(&relative, false)?;
            files.push((path, relative));
            if files.len() >= MAX_ARCHIVE_ENTRIES {
                return Err("导出文件数量过多。".into());
            }
        } else {
            return Err("导出目录包含不支持的文件类型。".into());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::InstanceState;
    use download_engine::DownloadSettings;
    use std::io::Cursor;

    fn base_index() -> PackIndex {
        PackIndex {
            format_version: 1,
            game: "minecraft".into(),
            version_id: "1.0.0".into(),
            name: "test pack".into(),
            files: vec![],
            dependencies: std::collections::HashMap::from([("minecraft".into(), "1.21.4".into())]),
        }
    }

    fn write_pack(entries: &[(&str, &[u8])]) -> PathBuf {
        let path = std::env::temp_dir().join(format!("{}.mrpack", uuid::Uuid::new_v4()));
        let mut writer = ZipWriter::new(File::create(&path).unwrap());
        let options = SimpleFileOptions::default();
        let index = serde_json::to_vec(&base_index()).unwrap();
        writer.start_file("modrinth.index.json", options).unwrap();
        writer.write_all(&index).unwrap();
        for (name, contents) in entries {
            writer.start_file(*name, options).unwrap();
            writer.write_all(contents).unwrap();
        }
        writer.finish().unwrap();
        path
    }

    fn test_instance(root: &Path, version: &str) -> InstanceRecord {
        InstanceRecord {
            id: "test".into(),
            name: "test".into(),
            game_version: version.into(),
            loader: "vanilla".into(),
            instance_dir: root.to_path_buf(),
            game_dir: root.join("game"),
            isolated: true,
            state: InstanceState::NotInstalled,
            created_at: "".into(),
            last_launched_at: None,
        }
    }

    fn test_engine() -> DownloadEngine {
        DownloadEngine::new(DownloadSettings::default()).unwrap()
    }

    #[test]
    fn path_rules_are_cross_platform() {
        for value in [
            "../evil",
            "mods/../../a.jar",
            "C:\\evil",
            "mods/a.jar:stream",
            "mods/CON.txt",
            "mods/a/",
            "mods/a\\b.jar",
        ] {
            assert!(safe_pack_path(value).is_err(), "{value}");
        }
        assert!(safe_pack_path("mods/a.jar").is_ok());
    }

    #[test]
    fn path_registry_rejects_case_and_parent_child_collisions() {
        let mut paths = PathRegistry::default();
        paths.insert("mods/Foo.jar", false).unwrap();
        assert!(paths.insert("mods/foo.jar", false).is_err());
        assert!(paths.insert("mods/Foo.jar/child", false).is_err());
        let mut paths = PathRegistry::default();
        paths.insert("config/folder/child", false).unwrap();
        assert!(paths.insert("config/folder", false).is_err());
    }

    #[test]
    fn bounded_stream_checks_actual_bytes_and_declared_length() {
        let mut total = 0;
        let mut output = Vec::new();
        assert!(copy_bounded(&mut Cursor::new(b"12345"), &mut output, 5, 4, &mut total).is_err());
        assert!(copy_bounded(&mut Cursor::new(b"123"), &mut Vec::new(), 4, 8, &mut 0).is_err());
        let mut total = 0;
        assert_eq!(
            copy_bounded(&mut Cursor::new(b"1234"), &mut Vec::new(), 4, 4, &mut total).unwrap(),
            4
        );
    }

    #[test]
    fn actual_stream_budget_checks_total_and_dishonest_declarations() {
        let mut output = Vec::new();
        let mut total = MAX_TOTAL - 2;
        assert!(copy_bounded(&mut Cursor::new(b"123"), &mut output, 3, 10, &mut total).is_err());
        assert!(output.is_empty());
        let mut output = Vec::new();
        assert!(copy_bounded(&mut Cursor::new(b"12345"), &mut output, 1, 10, &mut 0).is_err());
        assert!(output.is_empty());
    }

    #[test]
    fn forged_zip_size_does_not_bypass_stream_limits() {
        let path = write_pack(&[("overrides/config/test.txt", &[b'x'; 1024])]);
        let mut bytes = fs::read(&path).unwrap();
        let positions: Vec<_> = bytes
            .windows(4)
            .enumerate()
            .filter_map(|(index, bytes)| (bytes == b"PK\x01\x02").then_some(index))
            .collect();
        assert_eq!(positions.len(), 2);
        bytes[positions[1] + 24..positions[1] + 28].copy_from_slice(&1_u32.to_le_bytes());
        fs::write(&path, bytes).unwrap();
        assert!(preview_pack(&path).is_err());
        fs::remove_file(path).unwrap();
    }

    #[tokio::test]
    async fn changed_archive_data_rolls_back_partial_extraction() {
        use std::io::{Seek, SeekFrom};
        let path = write_pack(&[("overrides/config/test.txt", b"original data")]);
        let prepared = prepare_pack_ready(&path).unwrap();
        let offset = {
            let mut archive = ZipArchive::new(File::open(&path).unwrap()).unwrap();
            archive
                .by_name("overrides/config/test.txt")
                .unwrap()
                .data_start()
        };
        let mut archive_file = OpenOptions::new().write(true).open(&path).unwrap();
        archive_file.seek(SeekFrom::Start(offset)).unwrap();
        archive_file.write_all(b"BAD DATA").unwrap();
        drop(archive_file);
        let root = std::env::temp_dir().join(uuid::Uuid::new_v4().to_string());
        let target = test_instance(&root, "1.21.4");
        assert!(
            import_prepared_pack(prepared, &target, &test_engine(), None)
                .await
                .is_err()
        );
        assert!(!target.game_dir.exists());
        assert!(!root.exists());
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn rejects_directory_case_aliases_and_directory_file_collisions() {
        for (first, second) in [
            ("overrides/config/Foo/", "client-overrides/config/foo/a"),
            ("overrides/config/a/", "client-overrides/config/a"),
        ] {
            let path = write_pack(&[(second, b"data")]);
            let file = OpenOptions::new()
                .read(true)
                .write(true)
                .open(&path)
                .unwrap();
            let mut writer = ZipWriter::new_append(file).unwrap();
            writer
                .add_directory(first, SimpleFileOptions::default())
                .unwrap();
            writer.finish().unwrap();
            assert!(preview_pack(&path).is_err(), "{first}, {second}");
            fs::remove_file(path).unwrap();
        }
    }

    #[test]
    fn rejects_index_override_collisions_even_for_skipped_client_files() {
        let path = write_pack(&[("overrides/config/shared", b"override")]);
        let mut index = base_index();
        index.files.push(PackFile {
            path: "config/shared".into(),
            hashes: Default::default(),
            downloads: vec![],
            file_size: 0,
            env: std::collections::HashMap::from([("client".into(), "optional".into())]),
        });
        let mut prepared = prepare_pack_ready(&path).unwrap();
        let (_, size, destinations) = validate_index(&index).unwrap();
        assert!(scan_archive(&mut prepared.archive, size, &destinations, None).is_err());
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn rejects_duplicate_identical_zip_names_before_index_lookup() {
        let path = write_pack(&[
            ("overrides/config/repeated.txt", b"first"),
            ("overrides/config/repeated.jar", b"second"),
        ]);
        let mut bytes = fs::read(&path).unwrap();
        let source = b"overrides/config/repeated.jar";
        let target = b"overrides/config/repeated.txt";
        let offsets: Vec<_> = bytes
            .windows(source.len())
            .enumerate()
            .filter_map(|(offset, part)| (part == source).then_some(offset))
            .collect();
        assert_eq!(offsets.len(), 2);
        for offset in offsets {
            bytes[offset..offset + source.len()].copy_from_slice(target);
        }
        fs::write(&path, bytes).unwrap();
        assert!(preview_pack(&path).is_err());
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn rejects_duplicate_case_alias_and_override_collisions() {
        let path = write_pack(&[
            ("overrides/mods/Example.jar", b"one"),
            ("client-overrides/MODS/example.jar", b"two"),
        ]);
        assert!(preview_pack(&path).is_err());
        fs::remove_file(path).unwrap();

        let path = write_pack(&[
            ("overrides/config/a", b"one"),
            ("overrides/config/a/child", b"two"),
        ]);
        assert!(preview_pack(&path).is_err());
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn rejects_traversal_and_symlink_entries_before_import() {
        let path = write_pack(&[("overrides/../escape", b"unsafe")]);
        assert!(preview_pack(&path).is_err());
        fs::remove_file(path).unwrap();

        let path = std::env::temp_dir().join(format!("{}.mrpack", uuid::Uuid::new_v4()));
        let mut writer = ZipWriter::new(File::create(&path).unwrap());
        writer
            .start_file("modrinth.index.json", SimpleFileOptions::default())
            .unwrap();
        writer
            .write_all(&serde_json::to_vec(&base_index()).unwrap())
            .unwrap();
        writer
            .add_symlink(
                "overrides/mods/link.jar",
                "target",
                SimpleFileOptions::default(),
            )
            .unwrap();
        writer.finish().unwrap();
        assert!(preview_pack(&path).is_err());
        fs::remove_file(path).unwrap();
    }

    #[tokio::test]
    async fn client_override_replaces_generic_exact_path_but_not_a_download() {
        let path = write_pack(&[
            ("overrides/config/options.txt", b"generic"),
            ("client-overrides/config/options.txt", b"client"),
        ]);
        let prepared = prepare_pack_ready(&path).unwrap();
        assert_eq!(prepared.preview.override_files, 1);
        let root = std::env::temp_dir().join(uuid::Uuid::new_v4().to_string());
        let instance = test_instance(&root, "1.21.4");
        let preview = import_prepared_pack(prepared, &instance, &test_engine(), None)
            .await
            .unwrap();
        assert_eq!(preview.override_files, 1);
        assert_eq!(
            fs::read(root.join("game/config/options.txt")).unwrap(),
            b"client"
        );
        fs::remove_dir_all(root).unwrap();
        fs::remove_file(path).unwrap();
    }

    #[tokio::test]
    async fn import_rechecks_the_instance_version_and_leaves_no_staging_files() {
        let path = write_pack(&[]);
        let root = std::env::temp_dir().join(uuid::Uuid::new_v4().to_string());
        let instance = test_instance(&root, "1.20.1");
        let prepared = prepare_pack_ready(&path).unwrap();
        let result = import_prepared_pack(prepared, &instance, &test_engine(), None).await;
        assert!(result.is_err());
        assert!(!root.exists());
        fs::remove_file(path).unwrap();
    }

    #[tokio::test]
    async fn import_uses_prepared_archive_and_commits_atomically() {
        let path = write_pack(&[("overrides/config/options.txt", b"from archive")]);
        let prepared = prepare_pack_ready(&path).unwrap();
        let root = std::env::temp_dir().join(uuid::Uuid::new_v4().to_string());
        let instance = test_instance(&root, "1.21.4");
        fs::remove_file(&path).unwrap();
        let preview = import_prepared_pack(prepared, &instance, &test_engine(), None)
            .await
            .unwrap();
        assert_eq!(preview.override_files, 1);
        assert_eq!(
            fs::read(root.join("game/config/options.txt")).unwrap(),
            b"from archive"
        );
        assert!(root.join("modrinth.index.json").is_file());
        assert_eq!(fs::read_dir(&root).unwrap().count(), 2);
        fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn failed_metadata_publish_preserves_existing_metadata_and_removes_staging() {
        let path = write_pack(&[("overrides/config/test", b"new")]);
        let root = std::env::temp_dir().join(uuid::Uuid::new_v4().to_string());
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("modrinth.index.json"), b"keep").unwrap();
        let instance = test_instance(&root, "1.21.4");
        assert!(
            import_pack(&path, &instance, &test_engine(), None)
                .await
                .is_err()
        );
        assert!(!instance.game_dir.exists());
        assert_eq!(fs::read(root.join("modrinth.index.json")).unwrap(), b"keep");
        assert_eq!(fs::read_dir(&root).unwrap().count(), 1);
        fs::remove_dir_all(root).unwrap();
        fs::remove_file(path).unwrap();
    }

    #[tokio::test]
    async fn import_does_not_replace_existing_game_directory() {
        let path = write_pack(&[]);
        let root = std::env::temp_dir().join(uuid::Uuid::new_v4().to_string());
        fs::create_dir_all(root.join("game")).unwrap();
        fs::write(root.join("game/keep.txt"), b"keep").unwrap();
        let instance = test_instance(&root, "1.21.4");
        assert!(
            import_pack(&path, &instance, &test_engine(), None)
                .await
                .is_err()
        );
        assert_eq!(fs::read(root.join("game/keep.txt")).unwrap(), b"keep");
        fs::remove_dir_all(root).unwrap();
        fs::remove_file(path).unwrap();
    }

    #[tokio::test]
    async fn exported_pack_round_trips_without_saves_or_launcher_files() {
        let root = std::env::temp_dir().join(uuid::Uuid::new_v4().to_string());
        fs::create_dir_all(root.join("game/config")).unwrap();
        fs::create_dir_all(root.join("game/saves")).unwrap();
        fs::write(root.join("game/config/options.txt"), b"options").unwrap();
        fs::write(root.join("game/saves/private.dat"), b"private").unwrap();
        let instance = test_instance(&root, "1.21.4");
        let path = export_pack(&instance, &root).unwrap();
        let preview = preview_pack(&path).unwrap();
        assert_eq!(preview.override_files, 1);
        let target = test_instance(&root.join("imported"), "1.21.4");
        import_pack(&path, &target, &test_engine(), None)
            .await
            .unwrap();
        assert_eq!(
            fs::read(target.game_dir.join("config/options.txt")).unwrap(),
            b"options"
        );
        assert!(!target.game_dir.join("saves").exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn export_rejects_symlinked_game_and_case_conflicts() {
        let root = std::env::temp_dir().join(uuid::Uuid::new_v4().to_string());
        fs::create_dir_all(root.join("game/mods")).unwrap();
        fs::write(root.join("game/mods/A.jar"), b"a").unwrap();
        fs::write(root.join("game/mods/a.jar"), b"b").unwrap();
        let instance = test_instance(&root, "1.21.4");
        assert!(export_pack(&instance, &root).is_err());
        fs::remove_dir_all(root).unwrap();
    }

    // ---------------------------------------------------------------------
    // CurseForge 格式（manifest.json）
    // ---------------------------------------------------------------------

    /// 写一个 CurseForge 格式的整合包（manifest.json + 可选覆盖文件）。
    fn write_curseforge_pack(manifest: &serde_json::Value, entries: &[(&str, &[u8])]) -> PathBuf {
        let path = std::env::temp_dir().join(format!("{}.zip", uuid::Uuid::new_v4()));
        let mut writer = ZipWriter::new(File::create(&path).unwrap());
        let options = SimpleFileOptions::default();
        writer.start_file("manifest.json", options).unwrap();
        writer
            .write_all(&serde_json::to_vec(manifest).unwrap())
            .unwrap();
        for (name, contents) in entries {
            writer.start_file(*name, options).unwrap();
            writer.write_all(contents).unwrap();
        }
        writer.finish().unwrap();
        path
    }

    fn curseforge_manifest() -> serde_json::Value {
        serde_json::json!({
            "minecraft": {
                "version": "1.20.1",
                "modLoaders": [{ "id": "fabric-0.15.11", "primary": true }]
            },
            "manifestType": "minecraftModpack",
            "manifestVersion": 1,
            "name": "CF Test Pack",
            "version": "2.0.0",
            "author": "someone",
            "files": [
                { "projectID": 10, "fileID": 1001, "required": true },
                { "projectID": 11, "fileID": 1002, "required": false }
            ]
        })
    }

    #[test]
    fn curseforge_manifest_is_detected_without_network() {
        let path = write_curseforge_pack(&curseforge_manifest(), &[]);
        match prepare_pack(&path).unwrap() {
            PackPreparation::NeedsOnlineLookup { manifest, .. } => {
                assert_eq!(manifest.minecraft.version, "1.20.1");
                assert_eq!(manifest.files.len(), 2);
                // fileID 去重后按清单顺序给出。
                assert_eq!(manifest.file_ids(), vec![1001, 1002]);
                // 预览在补全前也能给出准确信息，只有总字节数未知。
                let preview = manifest.to_preview();
                assert_eq!(preview.minecraft, "1.20.1");
                assert_eq!(preview.loader, "fabric@0.15.11");
                assert_eq!(preview.required_files, 1);
                assert_eq!(preview.optional_files, 1);
                assert_eq!(preview.total_bytes, 0);
            }
            PackPreparation::Ready(_) => panic!("CurseForge pack must not be Ready without lookup"),
        }
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn curseforge_preview_matches_prepare_result() {
        let path = write_curseforge_pack(&curseforge_manifest(), &[]);
        // preview_pack 走的是同一条解析路径，两者不能给出不同结论。
        let preview = preview_pack(&path).unwrap();
        assert_eq!(preview.name, "CF Test Pack");
        assert_eq!(preview.version, "2.0.0");
        assert_eq!(preview.minecraft, "1.20.1");
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn curseforge_file_ids_are_deduplicated() {
        let mut manifest = curseforge_manifest();
        manifest["files"] = serde_json::json!([
            { "projectID": 1, "fileID": 7, "required": true },
            { "projectID": 1, "fileID": 7, "required": true },
            { "projectID": 2, "fileID": 9, "required": true }
        ]);
        let parsed: CurseForgeManifest = serde_json::from_value(manifest).unwrap();
        assert_eq!(parsed.file_ids(), vec![7, 9]);
    }

    #[test]
    fn zip_without_any_index_is_rejected_clearly() {
        let path = std::env::temp_dir().join(format!("{}.zip", uuid::Uuid::new_v4()));
        let mut writer = ZipWriter::new(File::create(&path).unwrap());
        writer
            .start_file("readme.txt", SimpleFileOptions::default())
            .unwrap();
        writer.write_all(b"hi").unwrap();
        writer.finish().unwrap();
        let error = match prepare_pack(&path) {
            Ok(_) => panic!("expect error"),
            Err(error) => error,
        };
        // 不能误报成「这是 CurseForge 格式」——那样用户会去查一个不存在的问题。
        assert!(error.contains("不是有效的整合包"), "{error}");
        assert!(!error.contains("CurseForge 格式"), "{error}");
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn curseforge_files_are_classified_into_instance_directories() {
        // `.jar` 进 mods/，资源包/光影包按名字归到各自目录，其余跳过。
        assert_eq!(
            classify_curseforge_file("jei-1.20.1.jar").as_deref(),
            Some("mods/jei-1.20.1.jar")
        );
        assert_eq!(
            classify_curseforge_file("Faithful-32x-resourcepack.zip").as_deref(),
            Some("resourcepacks/Faithful-32x-resourcepack.zip")
        );
        assert_eq!(
            classify_curseforge_file("ComplementaryShaders.zip").as_deref(),
            Some("shaderpacks/ComplementaryShaders.zip")
        );
        // 拿不准的不硬塞进 mods/：放错位置会让游戏启动报错，比少装更难排查。
        assert_eq!(classify_curseforge_file("world-save.zip"), None);
        assert_eq!(classify_curseforge_file("notes.txt"), None);
    }

    #[test]
    fn curseforge_files_skip_restricted_and_unknown_entries() {
        use crate::curseforge::CurseForgeVersion;
        let manifest: CurseForgeManifest = serde_json::from_value(serde_json::json!({
            "minecraft": { "version": "1.20.1", "modLoaders": [] },
            "name": "n", "version": "1",
            "files": [
                { "projectID": 1, "fileID": 1, "required": true },
                { "projectID": 2, "fileID": 2, "required": true },
                { "projectID": 3, "fileID": 3, "required": true }
            ]
        }))
        .unwrap();
        let version = |id: &str, name: &str, url: Option<&str>| CurseForgeVersion {
            version_id: id.into(),
            display_name: name.into(),
            date_created: String::new(),
            file_name: name.into(),
            url: url.map(str::to_owned),
            sha1: Some("a".repeat(40)),
            size: 10,
            game_versions: vec![],
            loaders: vec![],
        };
        let details = std::collections::HashMap::from([
            (
                1_u64,
                version("1", "ok.jar", Some("https://edge.forgecdn.net/ok.jar")),
            ),
            // 作者禁止第三方分发：downloadUrl 为 null，必须跳过。
            (2_u64, version("2", "restricted.jar", None)),
        ]);
        let (files, skipped) = curseforge_files_to_pack_files(&manifest, &details);
        // 只有 1 号可用；2 号受限被跳过；3 号没查到也被跳过。
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].path, "mods/ok.jar");
        assert_eq!(skipped, 2);
        // 校验值与下载地址必须原样带上，否则下载引擎无法验证完整性。
        assert_eq!(files[0].hashes.get("sha1"), Some(&"a".repeat(40)));
        assert_eq!(
            files[0].downloads,
            vec!["https://edge.forgecdn.net/ok.jar".to_owned()]
        );
    }

    #[test]
    fn curseforge_pack_index_passes_domain_validation() {
        use crate::curseforge::CurseForgeVersion;
        let manifest: CurseForgeManifest = serde_json::from_value(serde_json::json!({
            "minecraft": { "version": "1.20.1", "modLoaders": [] },
            "name": "n", "version": "1",
            "files": [{ "projectID": 1, "fileID": 1, "required": true }]
        }))
        .unwrap();
        let details = std::collections::HashMap::from([(
            1_u64,
            CurseForgeVersion {
                version_id: "1".into(),
                display_name: "ok".into(),
                date_created: String::new(),
                file_name: "ok.jar".into(),
                url: Some("https://edge.forgecdn.net/ok.jar".into()),
                sha1: Some("a".repeat(40)),
                size: 10,
                game_versions: vec![],
                loaders: vec![],
            },
        )]);
        let (files, _) = curseforge_files_to_pack_files(&manifest, &details);
        let index = manifest.to_pack_index(files);
        // forgecdn 必须被 validate_index 接受，否则整条导入链在最后一步失败。
        assert!(validate_index(&index).is_ok());
    }
}
