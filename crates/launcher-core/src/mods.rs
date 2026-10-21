use crate::InstanceRecord;
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::{Read, Seek, SeekFrom},
    path::{Component, Path, PathBuf},
};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModFile {
    pub file_name: String,
    pub enabled: bool,
    pub size: u64,
}

fn checked_name(name: &str) -> Result<(), String> {
    if name.is_empty()
        || name.len() > 255
        || name.contains(['/', '\\', ':', '\0'])
        || Path::new(name)
            .components()
            .any(|c| !matches!(c, Component::Normal(_)))
        || !(name.ends_with(".jar") || name.ends_with(".jar.disabled"))
    {
        return Err("请选择实例 mods 目录内的 JAR 文件。".into());
    }
    Ok(())
}

fn mods_directory(instance: &InstanceRecord) -> Result<Option<PathBuf>, String> {
    // Check the game directory too: a normal mods directory below a symlink escapes the instance.
    let directory = instance.game_dir.join("mods");
    let directories = [&instance.game_dir, &directory];
    for directory in directories {
        match fs::symlink_metadata(directory) {
            Ok(metadata) if metadata.file_type().is_dir() => {}
            Ok(_) => return Err("游戏或 mods 目录不能是符号链接或普通文件。".into()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.to_string()),
        }
    }
    Ok(Some(instance.game_dir.join("mods")))
}

pub fn list_mods(instance: &InstanceRecord) -> Result<Vec<ModFile>, String> {
    let Some(directory) = mods_directory(instance)? else {
        return Ok(vec![]);
    };
    let mut result = Vec::new();
    let mut names = std::collections::HashSet::new();
    for entry in fs::read_dir(directory).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        let metadata = fs::symlink_metadata(entry.path()).map_err(|e| e.to_string())?;
        if checked_name(&name).is_err() || !metadata.file_type().is_file() {
            continue;
        }
        if !names.insert(name.to_lowercase()) {
            return Err("模组文件名存在大小写冲突。".into());
        }
        result.push(ModFile {
            enabled: name.ends_with(".jar"),
            file_name: name,
            size: metadata.len(),
        });
    }
    result.sort_by(|a, b| a.file_name.cmp(&b.file_name));
    Ok(result)
}

pub fn set_mod_enabled(instance: &InstanceRecord, name: &str, enabled: bool) -> Result<(), String> {
    checked_name(name)?;
    let directory = mods_directory(instance)?.ok_or("mods 目录不存在。")?;
    let source = directory.join(name);
    if !fs::symlink_metadata(&source)
        .map_err(|e| e.to_string())?
        .file_type()
        .is_file()
    {
        return Err("拒绝修改符号链接或非普通文件。".into());
    }
    let target_name = if enabled {
        name.strip_suffix(".disabled").unwrap_or(name).to_owned()
    } else if name.ends_with(".disabled") {
        name.to_owned()
    } else {
        format!("{name}.disabled")
    };
    if target_name == name {
        return Ok(());
    }
    let target = directory.join(&target_name);
    for entry in fs::read_dir(&directory).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let sibling = entry.file_name().to_string_lossy().into_owned();
        if sibling != name && sibling.to_lowercase() == target_name.to_lowercase() {
            return Err("目标文件已经存在或有大小写冲突；没有覆盖任何模组。".into());
        }
    }
    // hard_link atomically refuses an existing target, including dangling symlinks.
    // Unlike rename on Unix this cannot overwrite a file created after the check.
    fs::hard_link(&source, &target).map_err(|e| e.to_string())?;
    if let Err(error) = fs::remove_file(&source) {
        if let Err(rollback) = fs::remove_file(&target) {
            return Err(format!("{error}；回滚失败：{rollback}"));
        }
        return Err(error.to_string());
    }
    Ok(())
}

/// Severity of a static, local mod inspection finding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ModIssueSeverity {
    Warning,
    Error,
}

/// A finding concerns only the listed file names, never inferred mod names.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModIssue {
    pub severity: ModIssueSeverity,
    pub code: String,
    pub message: String,
    pub files: Vec<String>,
}

const MAX_MOD_JARS: usize = 512;
const MAX_DIRECTORY_ENTRIES: usize = 8192;
const MAX_JAR_BYTES: u64 = 256 * 1024 * 1024;
const MAX_METADATA_BYTES: u64 = 256 * 1024;
const MAX_ZIP_ENTRIES: usize = 16_384;
const MAX_CENTRAL_BYTES: u64 = 8 * 1024 * 1024;

fn finding(
    severity: ModIssueSeverity,
    code: &str,
    message: impl Into<String>,
    files: Vec<String>,
) -> ModIssue {
    ModIssue {
        severity,
        code: code.into(),
        message: message.into(),
        files,
    }
}

fn file_error(code: &str, message: impl Into<String>, name: &str) -> ModIssue {
    finding(ModIssueSeverity::Error, code, message, vec![name.into()])
}

#[derive(Clone)]
struct Dependency {
    id: String,
    versions: serde_json::Value,
}

struct InspectedMod {
    file: String,
    id: String,
    version: String,
    provides: Vec<(String, String)>,
    depends: Vec<Dependency>,
    conflicts: Vec<Dependency>,
    nested: bool,
    quilt: bool,
}

/// Inspect enabled JARs without extracting files, loading classes, or executing mods.
///
/// This is a best-effort static check, NOT a compatibility guarantee. Only root
/// Fabric schema 1 and Quilt schema 1 JSON metadata are understood. Filenames do
/// not identify mods. Fabric metadata is accepted on Quilt; Quilt-only metadata
/// is not accepted on Fabric. Unknown loaders do not produce loader guesses.
/// Dependencies on built-in game/runtime/loader IDs are omitted. Complex conflict
/// version predicates are never guessed; dependency version compatibility is not
/// evaluated. Nested JARs and unsupported metadata make absence checks incomplete.
///
/// Limits: 512 enabled JARs, 8192 directory entries, 256 MiB per JAR, 16384 ZIP
/// entries, 8 MiB central directory and 256 KiB per metadata document. ZIP64 and
/// ambiguous metadata/file name casing are rejected. Disabled files are ignored.
/// Findings (including I/O failures) are returned in deterministic order. An empty
/// result means no supported static issue was found, not that mods are compatible.
pub fn inspect_mods(instance: &InstanceRecord) -> Vec<ModIssue> {
    let mut issues = Vec::new();
    let directory = match mods_directory(instance) {
        Ok(Some(directory)) => directory,
        Ok(None) => return issues,
        Err(error) => {
            return vec![finding(
                ModIssueSeverity::Error,
                "unsafe_mods_directory",
                error,
                vec![],
            )];
        }
    };
    let entries = match fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) => {
            return vec![finding(
                ModIssueSeverity::Error,
                "inspection_io",
                error.to_string(),
                vec![],
            )];
        }
    };
    let mut candidates = Vec::new();
    for (index, entry) in entries.enumerate() {
        if index >= MAX_DIRECTORY_ENTRIES {
            return vec![finding(
                ModIssueSeverity::Error,
                "directory_limit",
                "mods 目录条目超限；未进行部分扫描。",
                vec![],
            )];
        }
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                return vec![finding(
                    ModIssueSeverity::Error,
                    "inspection_io",
                    error.to_string(),
                    vec![],
                )];
            }
        };
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        // In particular, .jar.disabled is never opened, even when it is a symlink.
        if !name.to_ascii_lowercase().ends_with(".jar") {
            continue;
        }
        candidates.push((name, entry.path()));
        if candidates.len() > MAX_MOD_JARS {
            return vec![finding(
                ModIssueSeverity::Error,
                "jar_limit",
                "启用的 JAR 数量超限；未进行部分扫描。",
                vec![],
            )];
        }
    }
    candidates.sort_by(|a, b| a.0.cmp(&b.0));
    let mut by_case = BTreeMap::<String, Vec<String>>::new();
    for (name, _) in &candidates {
        by_case
            .entry(name.to_lowercase())
            .or_default()
            .push(name.clone());
    }
    let mut ambiguous = BTreeSet::new();
    for files in by_case.values().filter(|files| files.len() > 1) {
        ambiguous.extend(files.iter().cloned());
        issues.push(finding(
            ModIssueSeverity::Error,
            "ambiguous_file_name",
            "模组文件名存在大小写歧义；这些文件不参与依赖判断。",
            files.clone(),
        ));
    }
    let loader = instance.loader.split('@').next().unwrap_or("");
    let mut complete = ambiguous.is_empty();
    let mut mods = Vec::new();
    for (name, path) in candidates {
        if ambiguous.contains(&name) {
            continue;
        }
        if checked_name(&name).is_err() {
            complete = false;
            issues.push(file_error(
                "ambiguous_file_name",
                "JAR 文件名无效或扩展名大小写不明确。",
                &name,
            ));
            continue;
        }
        match inspect_jar(&path, &name, loader) {
            Ok((metadata, mut jar_issues, understood)) => {
                complete &= understood;
                issues.append(&mut jar_issues);
                if let Some(metadata) = metadata {
                    mods.push(metadata);
                }
            }
            Err(issue) => {
                complete = false;
                issues.push(issue);
            }
        }
    }
    let mut providers = BTreeMap::<String, Vec<(usize, String)>>::new();
    for (index, metadata) in mods.iter().enumerate() {
        if incompatible_loader(metadata, loader) {
            continue;
        }
        providers
            .entry(metadata.id.clone())
            .or_default()
            .push((index, metadata.version.clone()));
        for (id, version) in &metadata.provides {
            if id != &metadata.id {
                providers
                    .entry(id.clone())
                    .or_default()
                    .push((index, version.clone()));
            }
        }
    }
    // Real IDs, unlike aliases, must be unique. Several mods may provide an alias.
    let mut ids = BTreeMap::<&str, Vec<String>>::new();
    for metadata in &mods {
        ids.entry(&metadata.id)
            .or_default()
            .push(metadata.file.clone());
    }
    for (id, files) in ids.into_iter().filter(|(_, files)| files.len() > 1) {
        issues.push(finding(
            ModIssueSeverity::Error,
            "duplicate_mod_id",
            format!("模组 ID {id} 在多个启用 JAR 中重复。"),
            files,
        ));
    }
    for metadata in &mods {
        // A known incompatible mod cannot satisfy this loader's dependencies.
        if incompatible_loader(metadata, loader) {
            issues.push(file_error(
                "incompatible_loader",
                format!(
                    "元数据声明的 {} 模组与实例加载器 {loader} 不兼容。",
                    if metadata.quilt { "Quilt" } else { "Fabric" }
                ),
                &metadata.file,
            ));
            continue;
        }
        if !matches!(loader, "fabric" | "quilt") {
            continue;
        }
        if complete {
            for dependency in &metadata.depends {
                if !builtin_id(&dependency.id) && !providers.contains_key(&dependency.id) {
                    issues.push(file_error(
                        "missing_dependency",
                        format!(
                            "模组 {} 缺少强制依赖 {}（仅检查声明的 ID，未判断版本范围）。",
                            metadata.id, dependency.id
                        ),
                        &metadata.file,
                    ));
                }
            }
        }
        for conflict in &metadata.conflicts {
            if builtin_id(&conflict.id) {
                continue;
            }
            if let Some(targets) = providers.get(&conflict.id) {
                for (index, version) in targets {
                    let target = &mods[*index];
                    if target.file != metadata.file
                        && certain_version_match(&conflict.versions, version)
                    {
                        issues.push(finding(ModIssueSeverity::Warning, "declared_conflict", format!("模组 {} 声明与 {} {} 冲突；这里只确认了静态声明，不能保证运行时兼容性。", metadata.id, conflict.id, version), vec![metadata.file.clone(), target.file.clone()]));
                    }
                }
            }
        }
    }
    if !complete {
        issues.push(finding(ModIssueSeverity::Warning, "inspection_incomplete", "部分 JAR、元数据或嵌套模组无法完整静态检查，已暂停缺少依赖判断；静态检查不能保证兼容性。", vec![]));
    }
    for issue in &mut issues {
        issue.files.sort();
        issue.files.dedup();
    }
    issues.sort_by(|a, b| (&a.code, &a.files, &a.message).cmp(&(&b.code, &b.files, &b.message)));
    issues.dedup();
    issues
}

fn incompatible_loader(metadata: &InspectedMod, loader: &str) -> bool {
    match loader {
        "fabric" => metadata.quilt,
        "vanilla" | "forge" | "neoforge" => true,
        _ => false,
    }
}

fn builtin_id(id: &str) -> bool {
    matches!(
        id,
        "minecraft"
            | "java"
            | "fabricloader"
            | "fabric-loader"
            | "quilt_loader"
            | "quilt-loader"
            | "forge"
            | "neoforge"
    )
}

// Bound allocations *before* ZipArchive parses its central directory. Reject
// ZIP64, multi-disk, prepended/trailing data and noncanonical metadata names.
// Walk raw central entries because ZipArchive collapses duplicate entry names.
fn check_zip(file: &mut fs::File, name: &str) -> Result<(), ModIssue> {
    let invalid = || file_error("invalid_metadata", "JAR ZIP 目录损坏或格式不受支持。", name);
    let limit = || {
        file_error(
            "archive_limit",
            "JAR 大小、ZIP 条目数或中央目录超限，或使用 ZIP64。",
            name,
        )
    };
    let size = file.metadata().map_err(|_| invalid())?.len();
    if size > MAX_JAR_BYTES {
        return Err(limit());
    }
    let tail_size = size.min(65_557) as usize;
    file.seek(SeekFrom::End(-(tail_size as i64)))
        .map_err(|_| invalid())?;
    let mut tail = vec![0; tail_size];
    file.read_exact(&mut tail).map_err(|_| invalid())?;
    let end = (0..tail.len().saturating_sub(21))
        .rev()
        .find(|&i| {
            tail[i..].starts_with(b"PK\x05\x06")
                && i + 22 + u16::from_le_bytes([tail[i + 20], tail[i + 21]]) as usize == tail.len()
        })
        .ok_or_else(invalid)?;
    let u16_at = |offset| u16::from_le_bytes([tail[end + offset], tail[end + offset + 1]]);
    let u32_at =
        |offset| u32::from_le_bytes(tail[end + offset..end + offset + 4].try_into().unwrap());
    let count = u16_at(10) as usize;
    let central_size = u32_at(12) as u64;
    let central_offset = u32_at(16) as u64;
    let end_offset = size - tail_size as u64 + end as u64;
    if count > MAX_ZIP_ENTRIES
        || central_size > MAX_CENTRAL_BYTES
        || (end >= 20 && tail[end - 20..].starts_with(b"PK\x06\x07"))
    {
        return Err(limit());
    }
    if u16_at(4) != 0
        || u16_at(6) != 0
        || u16_at(8) as usize != count
        || central_offset + central_size != end_offset
    {
        return Err(invalid());
    }
    file.seek(SeekFrom::Start(central_offset))
        .map_err(|_| invalid())?;
    let mut central = vec![0; central_size as usize];
    file.read_exact(&mut central).map_err(|_| invalid())?;
    let mut offset = 0;
    let mut metadata_names = BTreeSet::new();
    for _ in 0..count {
        let header = central.get(offset..offset + 46).ok_or_else(invalid)?;
        if !header.starts_with(b"PK\x01\x02") {
            return Err(invalid());
        }
        let field = |i| u16::from_le_bytes([header[i], header[i + 1]]) as usize;
        let name_length = field(28);
        let next = offset + 46 + name_length + field(30) + field(32);
        let entry_name = central
            .get(offset + 46..offset + 46 + name_length)
            .ok_or_else(invalid)?;
        for canonical in [b"fabric.mod.json".as_slice(), b"quilt.mod.json".as_slice()] {
            if entry_name.eq_ignore_ascii_case(canonical)
                && (entry_name != canonical || !metadata_names.insert(canonical.to_vec()))
            {
                return Err(file_error(
                    "ambiguous_metadata",
                    "JAR 内元数据名称重复或存在大小写歧义。",
                    name,
                ));
            }
        }
        if next > central.len() {
            return Err(invalid());
        }
        offset = next;
    }
    if offset != central.len() {
        return Err(invalid());
    }
    file.rewind().map_err(|_| invalid())?;
    Ok(())
}

fn inspect_jar(
    path: &Path,
    name: &str,
    loader: &str,
) -> Result<(Option<InspectedMod>, Vec<ModIssue>, bool), ModIssue> {
    let metadata =
        fs::symlink_metadata(path).map_err(|e| file_error("inspection_io", e.to_string(), name))?;
    if !metadata.file_type().is_file() {
        return Err(file_error(
            "unsafe_mod_file",
            "拒绝读取符号链接或非普通 JAR 文件。",
            name,
        ));
    }
    let mut file =
        fs::File::open(path).map_err(|e| file_error("inspection_io", e.to_string(), name))?;
    // Verify the opened handle still refers to the checked regular file before
    // reading bytes. This also catches replacement by a symlink during open.
    let after =
        fs::symlink_metadata(path).map_err(|e| file_error("inspection_io", e.to_string(), name))?;
    if !after.file_type().is_file() {
        return Err(file_error(
            "unsafe_mod_file",
            "JAR 在检查期间被替换，拒绝读取。",
            name,
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let opened = file
            .metadata()
            .map_err(|e| file_error("inspection_io", e.to_string(), name))?;
        if (metadata.dev(), metadata.ino()) != (opened.dev(), opened.ino())
            || (after.dev(), after.ino()) != (opened.dev(), opened.ino())
        {
            return Err(file_error(
                "unsafe_mod_file",
                "JAR 在检查期间被替换，拒绝读取。",
                name,
            ));
        }
    }
    check_zip(&mut file, name)?;
    let mut archive = zip::ZipArchive::new(file)
        .map_err(|e| file_error("invalid_metadata", e.to_string(), name))?;
    if archive.len() > MAX_ZIP_ENTRIES {
        return Err(file_error("archive_limit", "ZIP 条目数量超限。", name));
    }
    let mut fabric = None;
    let mut quilt = None;
    for (entry_name, destination, is_quilt) in [
        ("fabric.mod.json", &mut fabric, false),
        ("quilt.mod.json", &mut quilt, true),
    ] {
        let mut entry = match archive.by_name(entry_name) {
            Ok(entry) => entry,
            Err(zip::result::ZipError::FileNotFound) => continue,
            Err(error) => return Err(file_error("invalid_metadata", error.to_string(), name)),
        };
        if entry.is_symlink() || entry.is_dir() {
            return Err(file_error(
                "unsafe_metadata",
                "拒绝读取非普通元数据条目。",
                name,
            ));
        }
        if entry.size() > MAX_METADATA_BYTES {
            return Err(file_error(
                "metadata_limit",
                "模组 JSON 元数据超出 256 KiB 限制。",
                name,
            ));
        }
        let mut bytes = Vec::new();
        (&mut entry)
            .take(MAX_METADATA_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|e| file_error("invalid_metadata", e.to_string(), name))?;
        if bytes.len() as u64 > MAX_METADATA_BYTES {
            return Err(file_error(
                "metadata_limit",
                "解压后的模组 JSON 元数据超限。",
                name,
            ));
        }
        *destination = Some(parse_metadata(&bytes, name, is_quilt)?);
    }
    let selected = if loader == "quilt" {
        quilt.or(fabric)
    } else {
        fabric.or(quilt)
    };
    let Some(selected) = selected else {
        // A JAR without either supported descriptor is not identified from its name.
        return Ok((
            None,
            vec![finding(
                ModIssueSeverity::Warning,
                "unrecognized_metadata",
                "JAR 没有支持的 Fabric/Quilt 根元数据；未根据文件名猜测模组身份。",
                vec![name.into()],
            )],
            false,
        ));
    };
    let complete = !selected.nested;
    let issues = if selected.nested {
        vec![finding(
            ModIssueSeverity::Warning,
            "nested_mods_unchecked",
            "元数据包含嵌套 JAR；本地静态检查不展开嵌套模组。",
            vec![name.into()],
        )]
    } else {
        vec![]
    };
    Ok((Some(selected), issues, complete))
}

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id.bytes().all(|c| {
            c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, b'_' | b'-' | b'.' | b':')
        })
}

fn parse_metadata(bytes: &[u8], name: &str, quilt: bool) -> Result<InspectedMod, ModIssue> {
    use serde_json::Value;
    let invalid = || {
        file_error(
            "invalid_metadata",
            "模组 JSON 元数据损坏、字段类型错误或 schema 不受支持（仅支持 schema 1）。",
            name,
        )
    };
    let root: Value = serde_json::from_slice(bytes).map_err(|_| invalid())?;
    let schema = if quilt {
        "schema_version"
    } else {
        "schemaVersion"
    };
    if root.get(schema).and_then(Value::as_u64) != Some(1) {
        return Err(invalid());
    }
    let body = if quilt {
        root.get("quilt_loader").ok_or_else(invalid)?
    } else {
        &root
    };
    let id = body
        .get("id")
        .and_then(Value::as_str)
        .filter(|s| valid_id(s))
        .ok_or_else(invalid)?;
    let version = body
        .get("version")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or_else(invalid)?;
    let mut result = InspectedMod {
        file: name.into(),
        id: id.into(),
        version: version.into(),
        provides: vec![],
        depends: vec![],
        conflicts: vec![],
        nested: false,
        quilt,
    };
    if let Some(jars) = body.get("jars").or_else(|| root.get("jars")) {
        result.nested = !jars.as_array().ok_or_else(invalid)?.is_empty();
    }
    if let Some(provides) = body.get("provides") {
        for provided in provides.as_array().ok_or_else(invalid)? {
            let (id, version) = if quilt && provided.is_object() {
                (
                    provided.get("id").and_then(Value::as_str),
                    provided
                        .get("version")
                        .and_then(Value::as_str)
                        .unwrap_or(version),
                )
            } else {
                (provided.as_str(), version)
            };
            let id = id.filter(|s| valid_id(s)).ok_or_else(invalid)?;
            result.provides.push((id.into(), version.into()));
        }
    }
    for key in ["depends", "breaks", "conflicts"] {
        let Some(declarations) = body.get(key) else {
            continue;
        };
        let target = if key == "depends" {
            &mut result.depends
        } else {
            &mut result.conflicts
        };
        if quilt {
            for declaration in declarations.as_array().ok_or_else(invalid)? {
                // Quilt arrays express alternatives; `unless` is conditional. Neither
                // implies an unconditional missing dependency or certain conflict.
                if declaration.is_array() {
                    continue;
                }
                if let Some(id) = declaration.as_str() {
                    if !valid_id(id) {
                        return Err(invalid());
                    }
                    target.push(Dependency {
                        id: id.into(),
                        versions: Value::String("*".into()),
                    });
                    continue;
                }
                let declaration = declaration.as_object().ok_or_else(invalid)?;
                let id = declaration
                    .get("id")
                    .and_then(Value::as_str)
                    .filter(|s| valid_id(s))
                    .ok_or_else(invalid)?;
                if let Some(optional) = declaration.get("optional") {
                    if optional.as_bool().ok_or_else(invalid)? {
                        continue;
                    }
                }
                if declaration.contains_key("unless") {
                    continue;
                }
                let versions = declaration
                    .get("versions")
                    .cloned()
                    .unwrap_or(Value::String("*".into()));
                target.push(Dependency {
                    id: id.into(),
                    versions,
                });
            }
        } else {
            for (id, versions) in declarations.as_object().ok_or_else(invalid)? {
                if !valid_id(id)
                    || !(versions.is_string()
                        || versions.as_array().is_some_and(|items| {
                            !items.is_empty() && items.iter().all(Value::is_string)
                        }))
                {
                    return Err(invalid());
                }
                target.push(Dependency {
                    id: id.clone(),
                    versions: versions.clone(),
                });
            }
        }
    }
    Ok(result)
}

fn certain_version_match(predicate: &serde_json::Value, version: &str) -> bool {
    if let Some(alternatives) = predicate.as_array() {
        return alternatives
            .iter()
            .any(|item| certain_version_match(item, version));
    }
    let Some(predicate) = predicate.as_str() else {
        return false;
    };
    if predicate == "*" {
        return true;
    }
    // Only literal, full three-component versions can establish equality. No
    // ranges, wildcards, logical expressions, or partial versions are evaluated.
    let literal = predicate.strip_prefix('=').unwrap_or(predicate);
    let core = literal.split(['-', '+']).next().unwrap_or("");
    let components: Vec<_> = core.split('.').collect();
    literal == version
        && components.len() == 3
        && components
            .iter()
            .all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()))
}

#[cfg(test)]
#[path = "mods_inspection_tests.rs"]
mod inspection_tests;

#[cfg(test)]
mod tests {
    use super::*;
    fn instance(root: &Path) -> InstanceRecord {
        InstanceRecord {
            id: "test".into(),
            name: "test".into(),
            game_version: "1.21.4".into(),
            loader: "fabric@0.16.10".into(),
            instance_dir: root.to_owned(),
            game_dir: root.to_owned(),
            isolated: true,
            state: crate::InstanceState::Ready,
            created_at: "".into(),
        }
    }
    #[test]
    fn disallows_path_traversal() {
        for name in ["../a.jar", "x/y.jar", "C:a.jar", "evil.exe", "", "a\0.jar"] {
            assert!(checked_name(name).is_err());
        }
    }
    #[test]
    fn toggle_is_reversible_and_never_overwrites() {
        let root = std::env::temp_dir().join(uuid::Uuid::new_v4().to_string());
        fs::create_dir_all(root.join("mods")).unwrap();
        fs::write(root.join("mods/a.jar"), b"test").unwrap();
        let instance = instance(&root);
        set_mod_enabled(&instance, "a.jar", false).unwrap();
        assert!(!list_mods(&instance).unwrap()[0].enabled);
        set_mod_enabled(&instance, "a.jar.disabled", true).unwrap();
        assert!(list_mods(&instance).unwrap()[0].enabled);
        fs::write(root.join("mods/a.jar.disabled"), b"other").unwrap();
        assert!(set_mod_enabled(&instance, "a.jar", false).is_err());
        fs::remove_dir_all(root).unwrap();
    }
    #[cfg(unix)]
    #[test]
    fn symlinks_including_dangling_targets_are_never_followed() {
        use std::os::unix::fs::symlink;
        let root = std::env::temp_dir().join(uuid::Uuid::new_v4().to_string());
        fs::create_dir(&root).unwrap();
        let instance = instance(&root);
        symlink(root.join("missing"), root.join("mods")).unwrap();
        assert!(list_mods(&instance).is_err());
        assert!(set_mod_enabled(&instance, "a.jar", false).is_err());
        fs::remove_file(root.join("mods")).unwrap();
        fs::create_dir(root.join("mods")).unwrap();
        fs::write(root.join("mods/a.jar"), b"keep").unwrap();
        symlink(root.join("missing"), root.join("mods/a.jar.disabled")).unwrap();
        assert!(set_mod_enabled(&instance, "a.jar", false).is_err());
        assert_eq!(fs::read(root.join("mods/a.jar")).unwrap(), b"keep");
        assert!(set_mod_enabled(&instance, "a.jar.disabled", true).is_err());
        assert_eq!(list_mods(&instance).unwrap().len(), 1);
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn case_alias_target_is_not_overwritten() {
        let root = std::env::temp_dir().join(uuid::Uuid::new_v4().to_string());
        fs::create_dir_all(root.join("mods")).unwrap();
        fs::write(root.join("mods/A.jar"), b"one").unwrap();
        fs::write(root.join("mods/a.jar.disabled"), b"two").unwrap();
        assert!(set_mod_enabled(&instance(&root), "A.jar", false).is_err());
        fs::remove_dir_all(root).unwrap();
    }
}
