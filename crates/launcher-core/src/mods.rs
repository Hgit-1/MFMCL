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
const MAX_NESTED_DEPTH: usize = 8;
const MAX_TOTAL_JARS: usize = 1024;
const MAX_TOTAL_ENTRIES: usize = 65_536;
const MAX_EXPANDED_BYTES: u64 = 256 * 1024 * 1024;

#[derive(Default)]
struct InspectionBudget {
    jars: usize,
    entries: usize,
    expanded: u64,
    jarjar_ranges: BTreeMap<String, Vec<serde_json::Value>>,
    jarjar_candidates: BTreeMap<String, Vec<(String, String)>>,
}

fn safe_nested_path(path: &str) -> bool {
    !path.is_empty()
        && path.len() <= 1024
        && path.ends_with(".jar")
        && !path.contains(['\\', ':', '\0'])
        && !path.starts_with('/')
        && path
            .split('/')
            .all(|part| !part.is_empty() && part != "." && part != "..")
}

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

#[path = "mods_forge.rs"]
mod forge_metadata;

#[path = "mods_solver.rs"]
mod nested_solver;
#[path = "mods_versions.rs"]
mod versions;
use nested_solver::provider_map;
use versions::Match;

#[derive(Clone)]
enum Dependency {
    Mod {
        id: String,
        versions: serde_json::Value,
    },
    Any(Vec<Dependency>),
    Conditional {
        dependency: Box<Dependency>,
        unless: Box<Dependency>,
    },
    Unknown,
}

impl Dependency {
    fn new(id: impl Into<String>, versions: serde_json::Value) -> Self {
        Self::Mod {
            id: id.into(),
            versions,
        }
    }
    fn description(&self) -> String {
        match self {
            Self::Mod { id, versions } => format!("{id} {versions}"),
            Self::Any(items) => items
                .iter()
                .map(Self::description)
                .collect::<Vec<_>>()
                .join(" 或 "),
            Self::Conditional { dependency, unless } => format!(
                "{}（除非 {}）",
                dependency.description(),
                unless.description()
            ),
            Self::Unknown => "不受支持的依赖表达式".into(),
        }
    }
}

#[derive(Clone)]
struct InspectedMod {
    file: String,
    id: String,
    version: String,
    provides: Vec<(String, String)>,
    depends: Vec<Dependency>,
    conflicts: Vec<Dependency>,
    nested: Vec<String>,
    embedded: bool,
    artifact: Option<(String, String)>,
    containers: Vec<(String, String)>,
    quilt: bool,
    forge: Option<&'static str>,
}

/// Inspect enabled JARs without extracting files, loading classes, or executing mods.
///
/// Best-effort static inspection, never a runtime compatibility guarantee.
/// Supports Fabric comparator AND, JSON-array/string `||` OR, wildcards, loader
/// caret/tilde and prereleases; Quilt alternatives / any / all / unless; Forge
/// numeric Maven ranges, unions, open bounds and common qualifiers. Unsupported
/// grammar/version forms produce `unknown_version_constraint`, never compatibility.
/// Built-in game/runtime/loader dependencies are omitted (runtime not supplied).
///
/// Fabric/Quilt `jars` and Forge/NeoForge JarJar declarations are recursively read
/// in memory. Nested Fabric candidates are optional and root IDs take precedence;
/// JarJar coordinates and artifact versions are separate from mod IDs/versions.
/// Limits: 512 root JARs, 8192 directory entries, 256 MiB/JAR, 16384 ZIP entries
/// and 8 MiB central directory/JAR, 256 KiB/metadata, nesting depth 8, 1024 total
/// JARs, 65536 total entries, 256 MiB cumulative decompressed reads, 4096 nested
/// search states / 32768 candidate checks. ZIP64, symlinks and unsafe/ambiguous
/// declared paths are rejected.
/// Unreadable nests suspend absence/version-error conclusions, not other findings.
/// Multi-level JarJar constraints are conservatively intersected across discovered
/// containers; conditional packaging beyond these declarations is not simulated.
/// Disabled JARs are ignored. Findings are deterministic; no classes are loaded.
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
    let loader = match instance.loader.split('@').next().unwrap_or("") {
        "legacyfabric" => "fabric",
        "cleanroom" => "forge",
        "optifine" => "vanilla",
        other => other,
    };
    let mut complete = ambiguous.is_empty();
    let mut mods = Vec::new();
    let mut budget = InspectionBudget::default();
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
        match inspect_jar(&path, &name, loader, &mut budget) {
            Ok((metadata, mut jar_issues, understood)) => {
                complete &= understood;
                issues.append(&mut jar_issues);
                mods.extend(metadata);
            }
            Err(issue) => {
                complete = false;
                issues.push(issue);
            }
        }
    }
    let (selected, resolved) = nested_solver::select(mods, loader, &budget, &mut issues);
    let mods = selected;
    complete &= resolved;
    let providers = provider_map(
        mods.iter()
            .enumerate()
            .filter(|(_, m)| !incompatible_loader(m, loader))
            .map(|(i, _)| i),
        &mods,
    );
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
                    metadata
                        .forge
                        .unwrap_or(if metadata.quilt { "Quilt" } else { "Fabric" })
                ),
                &metadata.file,
            ));
            continue;
        }
        if !matches!(loader, "fabric" | "quilt" | "forge" | "neoforge") {
            continue;
        }
        for dependency in &metadata.depends {
            let result = dependency_match(dependency, &providers, metadata.forge.is_some(), false);
            let targets = dependency_targets(dependency, &providers);
            match result {
                Match::Yes => {}
                Match::Unknown => issues.push(finding(
                    ModIssueSeverity::Warning,
                    "unknown_version_constraint",
                    format!(
                        "模组 {} 的依赖 {} 含有不支持的约束或无法比较的版本；未假定兼容。",
                        metadata.id,
                        dependency.description()
                    ),
                    vec![metadata.file.clone()],
                )),
                Match::No if !targets.is_empty() && complete => {
                    let mut files = vec![metadata.file.clone()];
                    files.extend(targets.iter().map(|i| mods[*i].file.clone()));
                    let actual = targets
                        .iter()
                        .map(|i| format!("{} {}", mods[*i].id, mods[*i].version))
                        .collect::<Vec<_>>()
                        .join("、");
                    issues.push(finding(
                        ModIssueSeverity::Error,
                        "dependency_version_mismatch",
                        format!(
                            "模组 {} 需要 {}，已发现版本不满足：{actual}。",
                            metadata.id,
                            dependency.description()
                        ),
                        files,
                    ));
                }
                Match::No if complete => issues.push(file_error(
                    "missing_dependency",
                    format!(
                        "模组 {} 缺少强制依赖 {}。",
                        metadata.id,
                        dependency.description()
                    ),
                    &metadata.file,
                )),
                Match::No => {}
            }
        }
        for conflict in &metadata.conflicts {
            let targets = dependency_targets(conflict, &providers);
            if dependency_match(conflict, &providers, metadata.forge.is_some(), true)
                == Match::Unknown
            {
                issues.push(finding(
                    ModIssueSeverity::Warning,
                    "unknown_version_constraint",
                    format!(
                        "模组 {} 的冲突约束 {} 无法确定；未假定兼容。",
                        metadata.id,
                        conflict.description()
                    ),
                    vec![metadata.file.clone()],
                ));
                continue;
            }
            // Test each provider independently, preserving aliases and alternatives.
            for index in targets {
                let target = &mods[index];
                if target.file == metadata.file {
                    continue;
                }
                let one = provider_map(std::iter::once(index), &mods);
                if conflict_match(conflict, &providers, &one, metadata.forge.is_some())
                    == Match::Yes
                {
                    issues.push(finding(
                        ModIssueSeverity::Warning,
                        "declared_conflict",
                        format!(
                            "模组 {} 声明与 {} {} 冲突（{}）；这是静态声明，不是运行时保证。",
                            metadata.id,
                            target.id,
                            target.version,
                            conflict.description()
                        ),
                        vec![metadata.file.clone(), target.file.clone()],
                    ));
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
    if let Some(kind) = metadata.forge {
        return matches!(
            loader,
            "vanilla" | "fabric" | "quilt" | "forge" | "neoforge"
        ) && loader != kind;
    }
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
fn check_zip<R: Read + Seek>(file: &mut R, name: &str) -> Result<usize, ModIssue> {
    let invalid = || file_error("invalid_metadata", "JAR ZIP 目录损坏或格式不受支持。", name);
    let limit = || {
        file_error(
            "archive_limit",
            "JAR 大小、ZIP 条目数或中央目录超限，或使用 ZIP64。",
            name,
        )
    };
    let size = file.seek(SeekFrom::End(0)).map_err(|_| invalid())?;
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
        for canonical in [
            b"fabric.mod.json".as_slice(),
            b"quilt.mod.json".as_slice(),
            b"META-INF/mods.toml".as_slice(),
            b"META-INF/neoforge.mods.toml".as_slice(),
            b"META-INF/jarjar/metadata.json".as_slice(),
            b"META-INF/MANIFEST.MF".as_slice(),
        ] {
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
        if entry_name.len() >= 4 && entry_name[entry_name.len() - 4..].eq_ignore_ascii_case(b".jar")
        {
            let key: Vec<_> = entry_name.iter().map(u8::to_ascii_lowercase).collect();
            if !metadata_names.insert(key) {
                return Err(file_error(
                    "ambiguous_metadata",
                    "嵌套 JAR 条目存在重复或大小写歧义。",
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
    Ok(count)
}

fn inspect_jar(
    path: &Path,
    name: &str,
    loader: &str,
    budget: &mut InspectionBudget,
) -> Result<(Vec<InspectedMod>, Vec<ModIssue>, bool), ModIssue> {
    let metadata =
        fs::symlink_metadata(path).map_err(|e| file_error("inspection_io", e.to_string(), name))?;
    if !metadata.file_type().is_file() {
        return Err(file_error(
            "unsafe_mod_file",
            "拒绝读取符号链接或非普通 JAR 文件。",
            name,
        ));
    }
    let file =
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
    inspect_archive(file, name, loader, 0, budget)
}

fn read_entry<R: Read + Seek>(
    archive: &mut zip::ZipArchive<R>,
    path: &str,
    name: &str,
    limit: u64,
    budget: &mut InspectionBudget,
) -> Result<Option<Vec<u8>>, ModIssue> {
    let mut entry = match archive.by_name(path) {
        Ok(entry) => entry,
        Err(zip::result::ZipError::FileNotFound) => return Ok(None),
        Err(e) => return Err(file_error("invalid_metadata", e.to_string(), name)),
    };
    if entry.is_symlink() || entry.is_dir() {
        return Err(file_error(
            "unsafe_metadata",
            "拒绝读取非普通归档条目。",
            name,
        ));
    }
    if entry.size() > limit || budget.expanded.saturating_add(entry.size()) > MAX_EXPANDED_BYTES {
        return Err(file_error(
            if limit == MAX_METADATA_BYTES {
                "metadata_limit"
            } else {
                "archive_limit"
            },
            "解压后条目大小或累计字节超限。",
            name,
        ));
    }
    let mut bytes = Vec::new();
    (&mut entry)
        .take(limit.min(MAX_EXPANDED_BYTES - budget.expanded) + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| file_error("invalid_metadata", e.to_string(), name))?;
    budget.expanded = budget.expanded.saturating_add(bytes.len() as u64);
    if bytes.len() as u64 > limit || budget.expanded > MAX_EXPANDED_BYTES {
        return Err(file_error("archive_limit", "解压后累计字节超限。", name));
    }
    Ok(Some(bytes))
}

fn inspect_archive<R: Read + Seek>(
    mut file: R,
    name: &str,
    loader: &str,
    depth: usize,
    budget: &mut InspectionBudget,
) -> Result<(Vec<InspectedMod>, Vec<ModIssue>, bool), ModIssue> {
    if depth > MAX_NESTED_DEPTH || budget.jars >= MAX_TOTAL_JARS {
        return Err(file_error(
            "archive_limit",
            "嵌套 JAR 深度或累计数量超限。",
            name,
        ));
    }
    budget.jars += 1;
    budget.entries += check_zip(&mut file, name)?;
    if budget.entries > MAX_TOTAL_ENTRIES {
        return Err(file_error("archive_limit", "累计 ZIP 条目超限。", name));
    }
    let mut archive = zip::ZipArchive::new(file)
        .map_err(|e| file_error("invalid_metadata", e.to_string(), name))?;
    let mut fabric = None;
    let mut quilt = None;
    // Only parse the descriptor the loader will prefer; an unrelated descriptor
    // must not poison an otherwise valid dual-loader archive.
    let json_order = if loader == "quilt" {
        [("quilt.mod.json", true), ("fabric.mod.json", false)]
    } else {
        [("fabric.mod.json", false), ("quilt.mod.json", true)]
    };
    let mut forge_mods = if matches!(loader, "forge" | "neoforge") {
        forge_metadata::read(&mut archive, name, loader, budget)?
    } else {
        None
    };
    let forge_selected = matches!(loader, "forge" | "neoforge") && forge_mods.is_some();
    if !forge_selected {
        for (path, is_quilt) in json_order {
            if let Some(bytes) = read_entry(&mut archive, path, name, MAX_METADATA_BYTES, budget)? {
                let parsed = parse_metadata(&bytes, name, is_quilt)?;
                if is_quilt {
                    quilt = Some(parsed);
                } else {
                    fabric = Some(parsed);
                }
                break;
            }
        }
    }
    if fabric.is_none()
        && quilt.is_none()
        && forge_mods.is_none()
        && !matches!(loader, "forge" | "neoforge")
    {
        forge_mods = forge_metadata::read(&mut archive, name, loader, budget)?;
    }
    let mut mods = if forge_selected {
        forge_mods.take().unwrap()
    } else if let Some(item) = quilt.or(fabric) {
        vec![item]
    } else {
        forge_mods.unwrap_or_default()
    };
    let recognized = !mods.is_empty();
    let mut paths: Vec<(String, Option<(String, String)>)> = Vec::new();
    for metadata in &mods {
        if !incompatible_loader(metadata, loader)
            && (matches!(loader, "fabric" | "quilt") || metadata.forge.is_some())
        {
            paths.extend(metadata.nested.iter().cloned().map(|path| (path, None)));
        }
    }
    let mut jarjar = false;
    if matches!(loader, "forge" | "neoforge") {
        if let Some(bytes) = read_entry(
            &mut archive,
            "META-INF/jarjar/metadata.json",
            name,
            MAX_METADATA_BYTES,
            budget,
        )? {
            jarjar = true;
            let root: serde_json::Value = serde_json::from_slice(&bytes)
                .map_err(|_| file_error("invalid_metadata", "JarJar 元数据 JSON 无效。", name))?;
            let jars = root
                .get("jars")
                .and_then(serde_json::Value::as_array)
                .filter(|jars| jars.len() <= MAX_TOTAL_JARS)
                .ok_or_else(|| {
                    file_error("invalid_metadata", "JarJar jars 字段无效或超限。", name)
                })?;
            for jar in jars {
                let field = |pointer| {
                    jar.pointer(pointer)
                        .and_then(serde_json::Value::as_str)
                        .filter(|s| !s.is_empty())
                        .ok_or_else(|| {
                            file_error(
                                "invalid_metadata",
                                "JarJar 条目缺少标识、路径或版本。",
                                name,
                            )
                        })
                };
                let path = field("/path")?;
                if !safe_nested_path(path) {
                    return Err(file_error(
                        "unsafe_nested_path",
                        "JarJar 路径不安全。",
                        name,
                    ));
                }
                let key = format!(
                    "{}:{}",
                    field("/identifier/group")?,
                    field("/identifier/artifact")?
                );
                let version = field("/version/artifactVersion")?.to_owned();
                let range = serde_json::Value::String(field("/version/range")?.into());
                budget
                    .jarjar_ranges
                    .entry(key.clone())
                    .or_default()
                    .push(range);
                paths.push((path.into(), Some((key, version))));
            }
        }
    }
    paths.sort();
    paths.dedup();
    let mut issues = Vec::new();
    let mut complete =
        recognized || (depth > 0 && matches!(loader, "forge" | "neoforge")) || jarjar;
    if !complete {
        issues.push(finding(
            ModIssueSeverity::Warning,
            "unrecognized_metadata",
            "JAR 没有支持的模组元数据。",
            vec![name.into()],
        ));
    }
    for (path, artifact) in paths {
        let child_name = format!("{name}!/{path}");
        let result = (|| {
            if depth >= MAX_NESTED_DEPTH {
                return Err(file_error(
                    "archive_limit",
                    "嵌套 JAR 深度超限。",
                    &child_name,
                ));
            }
            let bytes = read_entry(&mut archive, &path, &child_name, MAX_JAR_BYTES, budget)?
                .ok_or_else(|| {
                    file_error("missing_nested_jar", "声明的嵌套 JAR 不存在。", &child_name)
                })?;
            inspect_archive(
                std::io::Cursor::new(bytes),
                &child_name,
                loader,
                depth + 1,
                budget,
            )
        })();
        match result {
            Ok((mut children, mut child_issues, understood)) => {
                complete &= understood;
                if understood {
                    if let Some((key, version)) = &artifact {
                        budget
                            .jarjar_candidates
                            .entry(key.clone())
                            .or_default()
                            .push((version.clone(), child_name.clone()));
                    }
                }
                for child in &mut children {
                    child.embedded = true;
                    if let Some((key, _)) = &artifact {
                        child.containers.push((key.clone(), child_name.clone()));
                        if child.file == child_name {
                            child.artifact = artifact.clone();
                        }
                    }
                }
                mods.append(&mut children);
                issues.append(&mut child_issues);
            }
            Err(issue) => {
                complete = false;
                issues.push(issue);
            }
        }
    }
    Ok((mods, issues, complete))
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
        nested: vec![],
        embedded: false,
        artifact: None,
        containers: vec![],
        quilt,
        forge: None,
    };
    if let Some(jars) = body.get("jars").or_else(|| root.get("jars")) {
        for jar in jars.as_array().ok_or_else(invalid)? {
            let path = if quilt {
                jar.as_str()
            } else {
                jar.get("file").and_then(Value::as_str)
            }
            .ok_or_else(invalid)?;
            if !safe_nested_path(path) {
                return Err(file_error(
                    "unsafe_nested_path",
                    "嵌套 JAR 路径不安全。",
                    name,
                ));
            }
            result.nested.push(path.into());
        }
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
                if let Some(dependency) = quilt_dependency(declaration, 0).map_err(|_| invalid())? {
                    target.push(dependency);
                }
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
                target.push(Dependency::new(id.clone(), versions.clone()));
            }
        }
    }
    Ok(result)
}

fn quilt_dependency(value: &serde_json::Value, depth: usize) -> Result<Option<Dependency>, ()> {
    use serde_json::Value;
    if depth >= 16 {
        return Ok(Some(Dependency::Unknown));
    }
    if let Some(id) = value.as_str() {
        if !valid_id(id) {
            return Err(());
        }
        return Ok(Some(Dependency::new(id, Value::String("*".into()))));
    }
    if let Some(items) = value.as_array() {
        if items.is_empty() || items.len() > 128 {
            return Err(());
        }
        let mut alternatives = Vec::new();
        for item in items {
            if let Some(item) = quilt_dependency(item, depth + 1)? {
                alternatives.push(item);
            }
        }
        return Ok(Some(Dependency::Any(alternatives)));
    }
    let map = value.as_object().ok_or(())?;
    let id = map
        .get("id")
        .and_then(Value::as_str)
        .filter(|id| valid_id(id))
        .ok_or(())?;
    if map
        .get("optional")
        .map(|v| v.as_bool().ok_or(()))
        .transpose()?
        .unwrap_or(false)
    {
        return Ok(None);
    }
    let dependency = Dependency::new(
        id,
        map.get("versions")
            .cloned()
            .unwrap_or(Value::String("*".into())),
    );
    if let Some(unless) = map.get("unless") {
        let unless = quilt_dependency(unless, depth + 1)?.unwrap_or(Dependency::Unknown);
        Ok(Some(Dependency::Conditional {
            dependency: Box::new(dependency),
            unless: Box::new(unless),
        }))
    } else {
        Ok(Some(dependency))
    }
}

fn dependency_match(
    dependency: &Dependency,
    providers: &BTreeMap<String, Vec<(usize, String)>>,
    forge: bool,
    conflict: bool,
) -> Match {
    match dependency {
        Dependency::Mod {
            id,
            versions: predicate,
        } => {
            if builtin_id(id) {
                return if conflict { Match::No } else { Match::Yes };
            }
            providers
                .get(id)
                .map(|targets| {
                    Match::any(
                        targets
                            .iter()
                            .map(|(_, version)| versions::evaluate(predicate, version, forge)),
                    )
                })
                .unwrap_or_else(|| {
                    if versions::evaluate(predicate, "0", forge) == Match::Unknown {
                        Match::Unknown
                    } else {
                        Match::No
                    }
                })
        }
        Dependency::Any(items) => Match::any(
            items
                .iter()
                .map(|d| dependency_match(d, providers, forge, conflict)),
        ),
        Dependency::Conditional { dependency, unless } => {
            match dependency_match(unless, providers, forge, false) {
                Match::Yes => {
                    if conflict {
                        Match::No
                    } else {
                        Match::Yes
                    }
                }
                Match::No => dependency_match(dependency, providers, forge, conflict),
                Match::Unknown => Match::Unknown,
            }
        }
        Dependency::Unknown => Match::Unknown,
    }
}

fn conflict_match(
    dependency: &Dependency,
    environment: &BTreeMap<String, Vec<(usize, String)>>,
    targets: &BTreeMap<String, Vec<(usize, String)>>,
    forge: bool,
) -> Match {
    match dependency {
        Dependency::Any(items) => Match::any(
            items
                .iter()
                .map(|item| conflict_match(item, environment, targets, forge)),
        ),
        Dependency::Conditional { dependency, unless } => {
            match dependency_match(unless, environment, forge, false) {
                Match::Yes => Match::No,
                Match::No => conflict_match(dependency, environment, targets, forge),
                Match::Unknown => Match::Unknown,
            }
        }
        _ => dependency_match(dependency, targets, forge, true),
    }
}

fn dependency_targets(
    dependency: &Dependency,
    providers: &BTreeMap<String, Vec<(usize, String)>>,
) -> Vec<usize> {
    match dependency {
        Dependency::Mod { id, .. } => providers
            .get(id)
            .into_iter()
            .flatten()
            .map(|(i, _)| *i)
            .collect(),
        Dependency::Any(items) => items
            .iter()
            .flat_map(|d| dependency_targets(d, providers))
            .collect(),
        Dependency::Conditional { dependency, .. } => dependency_targets(dependency, providers),
        Dependency::Unknown => vec![],
    }
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
            last_launched_at: None,
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
