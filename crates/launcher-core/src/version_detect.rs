//! 实例版本识别。
//!
//! ## 为什么需要它
//!
//! `InstanceRecord.game_version` 是创建时用户手填的一个字段。它只是一个记录，
//! 不是「事实」——用户在文件管理器里换了 jar、把外部整合包拷进来、或旧记录写错，
//! 这个字段就与实际文件脱节，而后续所有操作（选 Java、筛模组、装加载器）都以它
//! 为依据。
//!
//! PCL2 的做法是**从磁盘读出来**：扫 `versions/` 下的 JSON，看 `id`、
//! `inheritsFrom` 与库的坐标。这里按同样的思路实现，把「记录」降级为提示，
//! 让磁盘上的实际内容说话。
//!
//! ## 识别顺序
//!
//! 1. **版本 JSON 的 `id`** —— 最权威。Forge/NeoForge 的 `launch.json` 里
//!    `id` 是 `<mc>-forge-<ver>` 这种，从中能解析出 MC 版本。
//! 2. **`inheritsFrom`** —— 加载器配置几乎都指向基础版本（原版 JSON 的 id）。
//! 3. **`libraries` 里的客户端坐标** —— `net.minecraft:client:1.20.1` 那种。
//! 4. **版本目录名 / 实例目录名** —— 兜底，只能做模式匹配。
//!
//! 任一来源给出结果就停：越靠前越可信，后面的只在前面的信息缺失时才用。

use crate::InstanceRecord;
use serde::Deserialize;
use std::path::{Path, PathBuf};

/// 识别结果的可信度。
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum VersionConfidence {
    /// 来自版本 JSON 或 `inheritsFrom`，可直接采用。
    Exact,
    /// 来自库坐标或目录名匹配，通常正确但需要用户确认。
    Inferred,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VersionDetection {
    /// 识别出的 Minecraft 版本；`None` 表示完全没找到线索。
    pub version: Option<String>,
    /// 识别出的加载器（`forge` / `neoforge` / `fabric` / `quilt`），没有则为空。
    pub loader: Option<String>,
    /// 加载器版本（若能识别）。
    pub loader_version: Option<String>,
    pub confidence: Option<VersionConfidence>,
    /// 说明这一结论来自哪里，供界面显示（用户据此判断是否该手动修正）。
    pub source: String,
    /// 实例记录里存的值，与识别结果不同时界面需要提示「记录不一致」。
    pub recorded: String,
}

impl VersionDetection {
    /// 识别结果与记录不一致，且识别结果可信到足以提示用户。
    pub fn disagrees_with_record(&self) -> bool {
        match (&self.version, self.confidence) {
            (Some(found), Some(VersionConfidence::Exact)) => found != &self.recorded,
            _ => false,
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawProfile {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    inherits_from: Option<String>,
    #[serde(default)]
    libraries: Vec<RawLibrary>,
}

#[derive(Deserialize)]
struct RawLibrary {
    #[serde(default)]
    name: String,
}

/// 从 `net.minecraft:client:1.20.1` 这类坐标里取出 MC 版本。
///
/// 只认 `net.minecraft` 组的 `client` / `server` 产物：其它库的第三段是它们自己的
/// 版本号，误读会把 `asm:9.6` 当成 Minecraft 9.6。
fn version_from_libraries(libraries: &[RawLibrary]) -> Option<String> {
    for library in libraries {
        let parts: Vec<&str> = library.name.split(':').collect();
        if parts.len() < 3 {
            continue;
        }
        if parts[0] != "net.minecraft" || (parts[1] != "client" && parts[1] != "server") {
            continue;
        }
        let candidate = parts[2].split('-').next()?;
        if looks_like_mc_version(candidate) {
            return Some(candidate.to_owned());
        }
    }
    None
}

/// 粗略判断一个字符串是否像 Minecraft 版本号。
///
/// 形如 `1.20.1`、`1.21`、`26.3`、`1.20.1-pre1`、`24w14a`。不追求完备——
/// 它只用来在候选值里挑一个，误判的代价是「没识别出来」而不是「识别错了」，
/// 因为最终会与 `inheritsFrom` 等更强的来源交叉。
fn looks_like_mc_version(text: &str) -> bool {
    if text.is_empty() || text.len() > 32 {
        return false;
    }
    let first = text.chars().next().unwrap_or(' ');
    if !first.is_ascii_digit() {
        return false;
    }
    // 至少含一个点（`1.20`）或符合快照格式（`24w14a`）。
    text.contains('.') || (text.len() >= 6 && text.contains('w'))
}

/// 从 Forge/NeoForge 的版本 id 里拆出 MC 版本与加载器版本。
///
/// Forge 的 id 形如 `1.20.1-forge-47.4.28`；NeoForge 是 `neoforge-21.1.265`，
/// 它的 MC 版本要从加载器版本反推（`21.1` → `1.21.1`）。
fn parse_loader_from_id(id: &str) -> (Option<String>, Option<String>, Option<String>) {
    if let Some((mc, rest)) = id.split_once("-forge-") {
        if looks_like_mc_version(mc) {
            return (
                Some(mc.to_owned()),
                Some("forge".to_owned()),
                Some(rest.to_owned()),
            );
        }
    }
    if let Some(rest) = id.strip_prefix("neoforge-") {
        return (
            neoforge_mc_version(rest),
            Some("neoforge".to_owned()),
            Some(rest.to_owned()),
        );
    }
    if let Some((mc, rest)) = id.split_once("-fabric") {
        if looks_like_mc_version(mc) {
            return (
                Some(mc.to_owned()),
                Some("fabric".to_owned()),
                Some(rest.trim_start_matches('-').to_owned()),
            );
        }
    }
    if let Some((mc, rest)) = id.split_once("-quilt") {
        if looks_like_mc_version(mc) {
            return (
                Some(mc.to_owned()),
                Some("quilt".to_owned()),
                Some(rest.trim_start_matches('-').to_owned()),
            );
        }
    }
    (None, None, None)
}

/// 从 NeoForge 的版本号反推 MC 版本：`21.1.265` → `1.21.1`，`20.4.100` → `1.20.4`。
fn neoforge_mc_version(loader: &str) -> Option<String> {
    let mut parts = loader.split('.');
    let minor = parts.next()?;
    let patch = parts.next()?;
    if !minor.chars().all(|c| c.is_ascii_digit()) || !patch.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    Some(if patch == "0" {
        format!("1.{minor}")
    } else {
        format!("1.{minor}.{patch}")
    })
}

/// 读一个版本 JSON 并识别版本。
fn detect_in_profile(path: &Path) -> Option<(String, Option<VersionConfidence>, String)> {
    let bytes = std::fs::read(path).ok()?;
    let profile: RawProfile = serde_json::from_slice(&bytes).ok()?;

    // 1) inheritsFrom 最可信：加载器配置几乎都指向基础版本的原版 id。
    if let Some(parent) = &profile.inherits_from
        && looks_like_mc_version(parent)
    {
        return Some((
            parent.clone(),
            Some(VersionConfidence::Exact),
            "inheritsFrom".to_owned(),
        ));
    }
    // 2) id 本身可能就是 MC 版本（原版 JSON）。
    if let Some(id) = &profile.id {
        if looks_like_mc_version(id) && !id.contains('-') {
            return Some((
                id.clone(),
                Some(VersionConfidence::Exact),
                "version id".to_owned(),
            ));
        }
        let (mc, _, _) = parse_loader_from_id(id);
        if let Some(mc) = mc {
            return Some((mc, Some(VersionConfidence::Exact), "loader id".to_owned()));
        }
    }
    // 3) 库坐标。
    if let Some(mc) = version_from_libraries(&profile.libraries) {
        return Some((
            mc,
            Some(VersionConfidence::Inferred),
            "libraries".to_owned(),
        ));
    }
    None
}

/// 扫一个目录下所有 `*.json`，返回第一个能识别出 MC 版本的。
fn detect_in_directory(directory: &Path) -> Option<(String, Option<VersionConfidence>, String)> {
    let entries = std::fs::read_dir(directory).ok()?;
    let mut profiles: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().and_then(|e| e.to_str()) == Some("json"))
        .collect();
    // `launch.json` 优先：那是启动器自己写的加载器配置，比原版 JSON 更贴近实际。
    profiles.sort_by_key(|path| {
        if path.file_name().and_then(|n| n.to_str()) == Some("launch.json") {
            0
        } else {
            1
        }
    });
    for path in profiles {
        if let Some(found) = detect_in_profile(&path) {
            return Some(found);
        }
    }
    None
}

/// 从实例名里猜版本，例如 `1.21.1 neoforge 21.1.265`、`Minecraft 1.20.1`。
///
/// 只作为最后的兜底：名字是用户可改的自由文本，可信度最低。
fn detect_in_name(name: &str) -> Option<String> {
    for token in name.split([' ', '_', '-']) {
        let trimmed = token.trim();
        if looks_like_mc_version(trimmed) {
            return Some(trimmed.to_owned());
        }
    }
    None
}

/// 识别一个实例的 Minecraft 版本与加载器。
///
/// 不修改任何东西：调用方决定要不要把结果写回记录。这样「识别」与「修正」可以
/// 分开——界面可以先展示差异让用户确认，而不是静默改掉他的数据。
pub fn detect_instance_version(instance: &InstanceRecord) -> VersionDetection {
    let mut result = VersionDetection {
        version: None,
        loader: None,
        loader_version: None,
        confidence: None,
        source: String::new(),
        recorded: instance.game_version.clone(),
    };

    // 1) 版本目录里的 JSON。
    let version_dir = instance.version_dir();
    if let Some((found, confidence, source)) = detect_in_directory(&version_dir) {
        result.version = Some(found);
        result.confidence = confidence;
        result.source = source;
    }
    // 2) 版本根目录下的任何 JSON（布局与预期不同时的兜底）。
    if result.version.is_none() {
        if let Some((found, confidence, source)) = detect_in_directory(&instance.version_root()) {
            result.version = Some(found);
            result.confidence = confidence;
            result.source = format!("{source} (version root)");
        }
    }
    // 3) 从版本 JSON 的 id 里顺带取加载器信息。
    if let Ok(bytes) = std::fs::read(version_dir.join("launch.json"))
        && let Ok(profile) = serde_json::from_slice::<RawProfile>(&bytes)
        && let Some(id) = &profile.id
    {
        let (_, loader, loader_version) = parse_loader_from_id(id);
        result.loader = loader;
        result.loader_version = loader_version;
    }
    // 4) 实例记录里的加载器字段（`neoforge@21.1.265`）。
    if result.loader.is_none()
        && let Some((name, version)) = instance.loader.split_once('@')
        && name != "vanilla"
    {
        result.loader = Some(name.to_owned());
        result.loader_version = Some(version.to_owned());
    }
    // 5) 名字兜底。
    if result.version.is_none()
        && let Some(found) = detect_in_name(&instance.name)
    {
        result.version = Some(found);
        result.confidence = Some(VersionConfidence::Inferred);
        result.source = "instance name".to_owned();
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::InstanceState;

    fn write_profile(dir: &Path, name: &str, body: &str) {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(dir.join(name), body).unwrap();
    }

    fn instance(name: &str, recorded: &str, loader: &str, root: &Path) -> InstanceRecord {
        InstanceRecord {
            id: "i".into(),
            name: name.into(),
            game_version: recorded.into(),
            loader: loader.into(),
            instance_dir: root.join("instances").join("1.21.1 neoforge 21.1.265"),
            game_dir: root.join("shared"),
            isolated: false,
            state: InstanceState::Ready,
            created_at: "2026-01-01T00:00:00Z".into(),
            last_launched_at: None,
        }
    }

    #[test]
    fn reads_the_version_from_inherits_from() {
        let root = std::env::temp_dir().join(format!("mfmcl-detect-{}", uuid::Uuid::new_v4()));
        let record = instance("x", "WRONG", "neoforge@21.1.265", &root);
        write_profile(
            &record.version_dir(),
            "launch.json",
            r#"{"id":"1.21.1-neoforge-21.1.265","inheritsFrom":"1.21.1"}"#,
        );

        let found = detect_instance_version(&record);
        assert_eq!(found.version.as_deref(), Some("1.21.1"));
        assert_eq!(found.confidence, Some(VersionConfidence::Exact));
        assert!(
            found.disagrees_with_record(),
            "recorded WRONG should be flagged"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn parses_forge_and_neoforge_ids() {
        let (mc, loader, ver) = parse_loader_from_id("1.20.1-forge-47.4.28");
        assert_eq!(mc.as_deref(), Some("1.20.1"));
        assert_eq!(loader.as_deref(), Some("forge"));
        assert_eq!(ver.as_deref(), Some("47.4.28"));

        let (mc, loader, ver) = parse_loader_from_id("neoforge-21.1.265");
        assert_eq!(mc.as_deref(), Some("1.21.1"), "21.1.x maps to 1.21.1");
        assert_eq!(loader.as_deref(), Some("neoforge"));
        assert_eq!(ver.as_deref(), Some("21.1.265"));
    }

    #[test]
    fn neoforge_version_with_zero_patch_has_no_patch_part() {
        assert_eq!(neoforge_mc_version("21.0.167").as_deref(), Some("1.21"));
    }

    #[test]
    fn falls_back_to_library_coordinates() {
        let root = std::env::temp_dir().join(format!("mfmcl-detect-{}", uuid::Uuid::new_v4()));
        let record = instance("x", "wrong", "vanilla", &root);
        write_profile(
            &record.version_dir(),
            "1.20.1.json",
            r#"{"id":"custom-name","libraries":[{"name":"net.minecraft:client:1.20.1"},{"name":"org.ow2.asm:asm:9.6"}]}"#,
        );

        let found = detect_instance_version(&record);
        assert_eq!(found.version.as_deref(), Some("1.20.1"));
        assert_eq!(found.confidence, Some(VersionConfidence::Inferred));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn library_lookup_ignores_other_projects_versions() {
        // `asm:9.6` 的第三段不是 MC 版本，不能误读。
        let libs = vec![RawLibrary {
            name: "org.ow2.asm:asm:9.6".into(),
        }];
        assert_eq!(version_from_libraries(&libs), None);
    }

    #[test]
    fn name_is_only_a_last_resort() {
        let root = std::env::temp_dir().join(format!("mfmcl-detect-{}", uuid::Uuid::new_v4()));
        let record = instance("1.19.2 quilt test", "unknown", "vanilla", &root);
        let found = detect_instance_version(&record);
        assert_eq!(found.version.as_deref(), Some("1.19.2"));
        assert_eq!(found.confidence, Some(VersionConfidence::Inferred));
    }

    /*
     * 隔离实例的共享根就是实例目录，所以安装完成后 `instance_dir/versions/` 一定
     * 存在，里面装的是新布局的 `versions/<实例目录名>/<mc 版本>/`。
     * 只看 `versions/` 是否存在会把它误判成旧布局，`version_dir()` 于是少一层，
     * 启动和 Java 选择都会报「系统找不到指定的路径」。
     */
    #[test]
    fn nested_layout_is_not_mistaken_for_the_legacy_one() {
        let root = std::env::temp_dir().join(format!("mfmcl-detect-{}", uuid::Uuid::new_v4()));
        let mut record = instance("1.20.1 forge 1.20.1-47.4.29", "1.20.1", "vanilla", &root);
        record.isolated = true;
        record.instance_dir = root.join("instances").join("1.20.1 forge 1.20.1-47.4.29");
        record.game_dir = record.instance_dir.join("game");
        let nested = record
            .instance_dir
            .join("versions")
            .join("1.20.1 forge 1.20.1-47.4.29")
            .join("1.20.1");
        write_profile(&nested, "1.20.1.json", r#"{"id":"1.20.1"}"#);

        assert_eq!(record.version_dir(), nested);
        assert!(record.version_dir().join("1.20.1.json").is_file());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn legacy_layout_is_still_read_from_the_instance_directory() {
        let root = std::env::temp_dir().join(format!("mfmcl-detect-{}", uuid::Uuid::new_v4()));
        let mut record = instance("legacy", "1.20.1", "vanilla", &root);
        record.isolated = true;
        record.instance_dir = root.join("instances").join("legacy");
        record.game_dir = record.instance_dir.join("game");
        let legacy = record.instance_dir.join("versions").join("1.20.1");
        write_profile(&legacy, "1.20.1.json", r#"{"id":"1.20.1"}"#);

        assert_eq!(record.version_dir(), legacy);
        let _ = std::fs::remove_dir_all(root);
    }
}
