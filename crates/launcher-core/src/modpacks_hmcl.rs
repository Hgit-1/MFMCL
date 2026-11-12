//! HMCL's unversioned native format: `modpack.json` is Modpack metadata, while
//! `minecraft/pack.json` is a resolved Minecraft Version, NOT launcher settings.
//! Verified with HMCL-dev/HMCL HMCLModpackExportTask at 880b8cb07234,
//! HMCLModpackProvider/InstallTask and LibraryAnalyzer at v3.16.4, and
//! VersionSetting.Serializer at 30acc53ee864 (legacy `hmclversion.cfg`).
//! Also accepts a single-version directory/ZIP layout and v3.16 GameSettings
//! `overrideProperties`. Unknown metadata/settings fields are discarded; explicit
//! formatVersion values, unknown component patches and unsafe custom args fail.
//! Version mainClass, arguments, downloads and libraries are detection input only:
//! the normal Minecraft/loader installer must rebuild all executable metadata.
use super::*;
use crate::{InstanceOverrides, MemoryMode};
use serde_json::{Map, Value};
use std::collections::HashMap;

#[derive(Clone)]
pub(super) struct Layout {
    game: String,
    version_root: Option<String>,
    metadata: BTreeSet<String>,
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Manifest {
    #[serde(default)]
    name: String,
    #[serde(default)]
    version: String,
    game_version: Option<String>,
    format_version: Option<Value>,
}

fn json(archive: &mut ZipArchive<File>, name: &str) -> Result<Value, String> {
    let value: Value = serde_json::from_slice(&read_archive_bytes(archive, name)?)
        .map_err(|_| "HMCL JSON 元数据格式无效。")?;
    if !value.is_object() {
        return Err("HMCL JSON 元数据必须为对象。".into());
    }
    Ok(value)
}

fn version_path(name: &str) -> Option<(&str, &str)> {
    let (game, suffix) = if let Some((game, suffix)) = name.rsplit_once("/versions/") {
        (game, suffix)
    } else {
        ("", name.strip_prefix("versions/")?)
    };
    let (id, file) = suffix.split_once('/')?;
    (file == format!("{id}.json")).then_some((game, id))
}

pub(super) fn locate_version_root(
    archive: &mut ZipArchive<File>,
) -> Result<(String, String), String> {
    let mut roots = BTreeSet::new();
    let mut names = BTreeSet::new();
    for i in 0..archive.len() {
        let entry = archive.by_index(i).map_err(|e| e.to_string())?;
        if entry.is_dir() {
            continue;
        }
        let name = entry.name().map_err(|_| "HMCL 文件名无法解码。")?;
        names.insert(name.to_string());
        if let Some((game, _)) = version_path(&name) {
            roots.insert(if game.is_empty() {
                String::new()
            } else {
                format!("{game}/")
            });
        } else if name == "pack.json" || name.ends_with("/pack.json") {
            let root = name.strip_suffix("pack.json").unwrap();
            // A mod's config/pack.json is data, not a launcher manifest.
            if !root
                .split('/')
                .any(|part| matches!(part, "config" | "saves" | "resourcepacks" | "mods"))
            {
                roots.insert(root.to_owned());
            }
        }
    }
    if roots.is_empty() {
        for (_, game, _) in loose_versions(archive, "", &names)? {
            roots.insert(prefix(&game));
        }
    }
    if roots.len() != 1 {
        return Err(if roots.is_empty() {
            "这不是有效的整合包：未找到支持的索引或 HMCL 版本元数据。".into()
        } else {
            "HMCL 压缩包包含多个游戏根目录，无法安全选择。".into()
        });
    }
    Ok((roots.into_iter().next().unwrap(), "hmcl-version".into()))
}

pub(super) fn read_index(
    archive: &mut ZipArchive<File>,
    root: &str,
    native: bool,
) -> Result<(PackIndex, Layout, Option<InstanceOverrides>), String> {
    let manifest: Manifest = if native {
        serde_json::from_value(json(archive, &format!("{root}modpack.json"))?)
            .map_err(|_| "HMCL modpack.json 格式无效。")?
    } else {
        Manifest::default()
    };
    if manifest.format_version.is_some() {
        return Err("仅支持官方无 formatVersion 的 HMCL 原生格式。".into());
    }
    let mut names = BTreeSet::new();
    let mut candidates = Vec::new();
    for i in 0..archive.len() {
        let entry = archive.by_index(i).map_err(|e| e.to_string())?;
        let name = entry.name().map_err(|_| "HMCL 文件名无法解码。")?;
        let Some(name) = name.strip_prefix(root) else {
            continue;
        };
        names.insert(name.trim_end_matches('/').to_owned());
        if entry.is_dir() {
            continue;
        }
        if let Some((game, id)) = version_path(name) {
            candidates.push((
                name.to_owned(),
                game.to_owned(),
                Some(format!("{}versions/{id}", prefix(game))),
            ));
        } else {
            for game in if native {
                vec!["minecraft", ".minecraft", "game_root", ""]
            } else {
                vec![""]
            } {
                if name == format!("{}pack.json", prefix(game)) {
                    candidates.push((name.to_owned(), game.to_owned(), None));
                }
            }
        }
    }
    if candidates.is_empty() {
        candidates = loose_versions(archive, root, &names)?;
    }
    if candidates.len() != 1 {
        return Err("HMCL 必须且只能包含一个 pack.json、版本 JSON 或 versions/<版本>/<版本>.json；多版本或缺失元数据无法安全转换。".into());
    }
    let (version_file, game, version_root) = candidates.remove(0);
    let game_prefix = prefix(&game);
    let version_ids: BTreeSet<_> = names
        .iter()
        .filter_map(|name| {
            name.strip_prefix(&format!("{game_prefix}versions/"))
                .map(|rest| rest.split('/').next().unwrap_or(""))
                .filter(|id| !id.is_empty())
        })
        .collect();
    if version_ids.len() > 1 {
        return Err("HMCL 包含多个版本目录，无法确定要导入的版本。".into());
    }
    let metadata = json(archive, &format!("{root}{version_file}"))?;
    let mut versions = BTreeSet::new();
    if let Some(version) = manifest.game_version.as_deref() {
        add_game_version(&mut versions, version)?;
    }
    for key in ["jar", "inheritsFrom"] {
        if let Some(version) = string(&metadata, key)? {
            add_game_version(&mut versions, version)?;
        }
    }
    let mut dependencies = HashMap::new();
    detect_components(&metadata, &mut dependencies, &mut versions, 0)?;
    if dependencies.is_empty()
        && string(&metadata, "mainClass")?
            .is_some_and(|main| main != "net.minecraft.client.main.Main")
    {
        return Err(
            "HMCL 启动入口不是可识别的原版或受支持加载器；不会执行自定义 mainClass。".into(),
        );
    }
    if versions.is_empty() {
        if let Some(id) = string(&metadata, "id")? {
            add_game_version_from_id(&mut versions, id)?;
        }
    }
    if versions.len() != 1 {
        return Err("HMCL Minecraft 版本缺失或元数据相互冲突。".into());
    }
    let minecraft = versions.into_iter().next().unwrap();
    dependencies.insert("minecraft".into(), minecraft);
    let mut metadata_files = BTreeSet::from([version_file.clone()]);
    if version_file.rsplit('/').next() != Some("pack.json") {
        if let Some(stem) = version_file.strip_suffix(".json") {
            metadata_files.insert(format!("{stem}.jar"));
        }
    }
    let mut layout = Layout {
        game,
        version_root,
        metadata: metadata_files,
    };
    let setting_root = layout
        .version_root
        .as_deref()
        .unwrap_or(layout.game.as_str());
    let setting_prefix = prefix(setting_root);
    let setting_files = [
        format!("{setting_prefix}hmclversion.cfg"),
        format!("{setting_prefix}.hmcl/config/instance-game-settings.json"),
    ];
    let present: Vec<_> = setting_files
        .into_iter()
        .filter(|name| names.contains(name))
        .collect();
    if present.len() > 1 {
        return Err("HMCL 同时包含新旧实例配置，无法确定有效的启动配置。".into());
    }
    let overrides = if let Some(file) = present.first() {
        let settings = json(archive, &format!("{root}{file}"))?;
        layout.metadata.insert(file.clone());
        Some(settings_to_overrides(&settings)?)
    } else {
        None
    };
    let name = if manifest.name.trim().is_empty() {
        root.trim_end_matches('/')
            .rsplit('/')
            .next()
            .filter(|name| !name.is_empty())
            .unwrap_or("HMCL instance")
            .to_owned()
    } else {
        manifest.name
    };
    Ok((
        PackIndex {
            format_version: 1,
            game: "minecraft".into(),
            version_id: manifest.version,
            name,
            files: vec![],
            dependencies,
        },
        layout,
        overrides.filter(|value| value != &InstanceOverrides::default()),
    ))
}

fn loose_versions(
    archive: &mut ZipArchive<File>,
    root: &str,
    names: &BTreeSet<String>,
) -> Result<Vec<(String, String, Option<String>)>, String> {
    let setting_roots: BTreeSet<_> = names
        .iter()
        .filter_map(|name| {
            name.strip_suffix("hmclversion.cfg")
                .or_else(|| name.strip_suffix(".hmcl/config/instance-game-settings.json"))
                .filter(|prefix| prefix.is_empty() || prefix.ends_with('/'))
                .map(|prefix| prefix.trim_end_matches('/'))
        })
        .collect();
    let mut candidates = Vec::new();
    for game in setting_roots {
        for name in names {
            let Some(stem) = name
                .strip_prefix(&prefix(game))
                .and_then(|name| name.strip_suffix(".json"))
            else {
                continue;
            };
            if stem.contains('/')
                || matches!(
                    stem,
                    "pack" | "modpack" | "accounts" | "launcher_accounts" | "launcher_profiles"
                )
            {
                continue;
            }
            let value = json(archive, &format!("{root}{name}"))?;
            if value.get("id").and_then(Value::as_str) == Some(stem)
                && ["mainClass", "inheritsFrom", "patches"]
                    .iter()
                    .any(|field| value.get(field).is_some())
            {
                candidates.push((name.clone(), game.to_owned(), None));
            }
        }
    }
    Ok(candidates)
}

fn prefix(path: &str) -> String {
    if path.is_empty() {
        String::new()
    } else {
        format!("{path}/")
    }
}

fn string<'a>(value: &'a Value, key: &str) -> Result<Option<&'a str>, String> {
    match value.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(text)) => Ok(Some(text)),
        _ => Err(format!("HMCL 字段 {key} 必须为字符串。")),
    }
}

fn add_game_version(versions: &mut BTreeSet<String>, version: &str) -> Result<(), String> {
    safe_version(version)?;
    versions.insert(version.to_owned());
    Ok(())
}

fn add_game_version_from_id(versions: &mut BTreeSet<String>, version: &str) -> Result<(), String> {
    safe_version(version)?;
    // A free-form instance ID is weaker evidence than jar/gameVersion/game patches.
    let release = version
        .split('.')
        .all(|part| !part.is_empty() && part.bytes().all(|c| c.is_ascii_digit()))
        && version.contains('.');
    let snapshot = version.len() == 6
        && version.as_bytes()[2] == b'w'
        && version[..2].bytes().all(|c| c.is_ascii_digit())
        && version[3..5].bytes().all(|c| c.is_ascii_digit())
        && version.as_bytes()[5].is_ascii_lowercase();
    let prerelease = version.split_once('-').is_some_and(|(base, suffix)| {
        let numeric_suffix = ["pre", "rc", "snapshot"]
            .iter()
            .find_map(|prefix| suffix.strip_prefix(*prefix))
            .map(|suffix| suffix.strip_prefix('-').unwrap_or(suffix));
        base.contains('.')
            && base
                .split('.')
                .all(|part| !part.is_empty() && part.bytes().all(|c| c.is_ascii_digit()))
            && numeric_suffix.is_some_and(|suffix| {
                !suffix.is_empty() && suffix.bytes().all(|c| c.is_ascii_digit())
            })
    });
    if !release && !snapshot && !prerelease {
        return Err(
            "HMCL 游戏版本不是可确定的正式版、快照或 pre/rc 版本；不会猜测自定义版本 ID。".into(),
        );
    }
    versions.insert(version.to_owned());
    Ok(())
}

fn component(deps: &mut HashMap<String, String>, key: &str, version: &str) -> Result<(), String> {
    safe_version(version)?;
    if deps.get(key).is_some_and(|prior| prior != version) {
        return Err(format!("HMCL 的 {key} 组件声明了冲突版本。"));
    }
    deps.insert(key.to_owned(), version.to_owned());
    Ok(())
}

fn detect_components(
    value: &Value,
    deps: &mut HashMap<String, String>,
    versions: &mut BTreeSet<String>,
    depth: usize,
) -> Result<(), String> {
    if value.get("formatVersion").is_some_and(|v| !v.is_null()) {
        return Err("未知 HMCL 版本元数据格式版本。".into());
    }
    if depth > 8 {
        return Err("HMCL 版本补丁层级过深。".into());
    }
    let patches = value
        .get("patches")
        .filter(|v| !v.is_null())
        .map(|v| v.as_array().ok_or("HMCL patches 必须为数组。"))
        .transpose()?;
    if let Some(patches) = patches {
        if patches.len() > 32 {
            return Err("HMCL 版本补丁数量过多。".into());
        }
        let mut seen = BTreeSet::new();
        for patch in patches {
            let id = string(patch, "id")?.ok_or("HMCL 版本补丁缺少 ID。")?;
            if !seen.insert(id) {
                return Err("HMCL 版本补丁 ID 重复。".into());
            }
            let version = string(patch, "version")?.ok_or("HMCL 版本补丁缺少版本。")?;
            match id {
                "game" => add_game_version(versions, version)?,
                "fabric" => component(deps, "fabric-loader", version)?,
                "legacyfabric" => component(deps, "legacyfabric-loader", version)?,
                "quilt" => component(deps, "quilt-loader", version)?,
                "forge" | "neoforge" | "cleanroom" | "liteloader" => component(deps, id, version)?,
                _ => {
                    return Err(format!(
                        "暂不支持 HMCL 版本补丁 {id}；不会丢失组件后假装成功。"
                    ));
                }
            }
            detect_components(patch, deps, versions, depth + 1)?;
        }
    }
    let libraries = value
        .get("libraries")
        .filter(|v| !v.is_null())
        .map(|v| v.as_array().ok_or("HMCL libraries 必须为数组。"))
        .transpose()?;
    if let Some(libraries) = libraries {
        if libraries.len() > 4096 {
            return Err("HMCL 运行库数量超过限制。".into());
        }
        let legacy = libraries.iter().any(|library| {
            library
                .get("name")
                .and_then(Value::as_str)
                .is_some_and(|name| name.starts_with("net.legacyfabric:"))
        });
        let neo = libraries.iter().any(|library| {
            library
                .get("name")
                .and_then(Value::as_str)
                .is_some_and(|name| name.starts_with("net.neoforged:"))
        }) || deps.contains_key("neoforge");
        for library in libraries {
            let name = string(library, "name")?.ok_or("HMCL 运行库缺少名称。")?;
            let parts: Vec<_> = name.split(':').collect();
            if !(3..=4).contains(&parts.len()) {
                return Err("HMCL 运行库坐标无效。".into());
            }
            for part in &parts {
                safe_version(part)?;
            }
            let version = parts[2];
            match (parts[0], parts[1]) {
                ("net.fabricmc", "fabric-loader") => component(
                    deps,
                    if legacy {
                        "legacyfabric-loader"
                    } else {
                        "fabric-loader"
                    },
                    version,
                )?,
                ("org.quiltmc", "quilt-loader") => component(deps, "quilt-loader", version)?,
                ("net.minecraftforge", "forge" | "fmlloader") if !neo => {
                    component(deps, "forge", strip_game_prefix(version))?
                }
                ("net.neoforged", "neoforge") => component(deps, "neoforge", version)?,
                ("net.neoforged", "forge") => {
                    component(deps, "neoforge", strip_game_prefix(version))?
                }
                ("com.cleanroommc", "cleanroom") => component(deps, "cleanroom", version)?,
                ("com.mumfrey", "liteloader") => component(deps, "liteloader", version)?,
                ("optifine" | "net.optifine", _) => {
                    return Err(
                        "HMCL 内置 OptiFine 版本补丁尚不能安全重建，请使用 mods 目录的版本。"
                            .into(),
                    );
                }
                _ => {}
            }
        }
    }
    if let Some(arguments) = value
        .get("arguments")
        .and_then(|v| v.get("game"))
        .and_then(Value::as_array)
    {
        for pair in arguments.windows(2) {
            if matches!(
                pair[0].as_str(),
                Some("--fml.neoForgeVersion" | "--fml.forgeVersion")
            ) {
                let version = pair[1].as_str().ok_or("HMCL FML 版本参数无效。")?;
                let neo = pair[0] == "--fml.neoForgeVersion"
                    || deps.contains_key("neoforge")
                    || value
                        .get("libraries")
                        .and_then(Value::as_array)
                        .is_some_and(|libs| {
                            libs.iter().any(|lib| {
                                lib.get("name").and_then(Value::as_str).is_some_and(|name| {
                                    name.starts_with("net.neoforged.fancymodloader:")
                                })
                            })
                        });
                component(deps, if neo { "neoforge" } else { "forge" }, version)?;
            }
        }
    }
    if !deps.contains_key("neoforge")
        && libraries.is_some_and(|libs| {
            libs.iter().any(|lib| {
                lib.get("name")
                    .and_then(Value::as_str)
                    .is_some_and(|name| name.starts_with("net.neoforged.fancymodloader:"))
            })
        })
    {
        return Err(
            "HMCL NeoForge 元数据缺少可确定的 NeoForge 版本；不会使用 FML 版本代替。".into(),
        );
    }
    Ok(())
}

fn strip_game_prefix(version: &str) -> &str {
    let parts: Vec<_> = version.split('-').collect();
    let numeric =
        |part: &str| !part.is_empty() && part.bytes().all(|c| c.is_ascii_digit() || c == b'.');
    if (2..=3).contains(&parts.len()) && parts.iter().all(|part| numeric(part)) {
        parts[1]
    } else {
        version
    }
}

pub(super) fn settings_to_overrides(value: &Value) -> Result<InstanceOverrides, String> {
    let fields = value.as_object().ok_or("HMCL 启动配置必须为对象。")?;
    if fields.contains_key("formatVersion") {
        return Err("未知 HMCL 启动配置格式版本。".into());
    }
    if boolean(fields, "usesGlobal")? == Some(true) {
        return Ok(InstanceOverrides::default());
    }
    let selected = fields
        .get("overrideProperties")
        .map(|value| {
            let list = value
                .as_array()
                .ok_or("HMCL overrideProperties 必须为数组。")?;
            list.iter()
                .map(|value| value.as_str().ok_or("HMCL 覆盖配置名必须为字符串。"))
                .collect::<Result<BTreeSet<_>, _>>()
        })
        .transpose()?;
    let active = |key: &str| selected.as_ref().is_none_or(|keys| keys.contains(key));
    let mut out = InstanceOverrides::default();
    if active("autoMemory") && boolean(fields, "autoMemory")? == Some(true) {
        out.memory_mode = Some(MemoryMode::Auto);
    } else if active("maxMemory") {
        if let Some(memory) = number(fields, "maxMemory")? {
            set_memory(&mut out, memory)?;
        }
    }
    if active("fullscreen") {
        out.fullscreen = boolean(fields, "fullscreen")?;
    }
    if active("windowType") {
        if let Some(mode) = fields.get("windowType") {
            out.fullscreen = match mode.as_str() {
                Some("WINDOWED") => Some(false),
                Some("FULLSCREEN") => Some(true),
                _ => return Err("HMCL 窗口模式暂不支持转换。".into()),
            };
        }
    }
    if active("width") && active("height") {
        out.window_width = number(fields, "width")?;
        out.window_height = number(fields, "height")?;
        if out.window_width == Some(0) && out.window_height == Some(0) {
            out.window_width = None;
            out.window_height = None;
        }
    } else if active("width") != active("height")
        && (fields.contains_key("width") || fields.contains_key("height"))
    {
        return Err("HMCL 必须同时覆盖窗口宽度和高度。".into());
    }
    if active("showLogs") {
        out.show_game_log = boolean(fields, "showLogs")?;
    }
    let mut jvm_parts = Vec::new();
    if active("minMemory") {
        if let Some(minimum) = number(fields, "minMemory")?.filter(|value| *value > 0) {
            if minimum > 262_144 || out.memory_mb.is_some_and(|maximum| minimum > maximum) {
                return Err("整合包最小堆大于最大堆或超过限制。".into());
            }
            jvm_parts.push(format!("-Xms{minimum}M"));
        }
    }
    let jvm_key = if fields.contains_key("jvmOptions") {
        "jvmOptions"
    } else {
        "javaArgs"
    };
    if active(jvm_key) {
        if let Some(text) = string(value, jvm_key)? {
            jvm_parts.push(text.to_owned());
        }
    }
    if !jvm_parts.is_empty() {
        apply_jvm_args(&mut out, &jvm_parts.join(" "))?;
    }
    let game_key = if fields.contains_key("gameArguments") {
        "gameArguments"
    } else {
        "minecraftArgs"
    };
    if active(game_key) {
        if let Some(text) = string(value, game_key)? {
            apply_game_args(&mut out, text)?;
        }
    }
    // Local Java paths, gameDir, tokens, environment, wrapper and hook commands
    // deliberately never enter InstanceOverrides, even when marked as overrides.
    crate::settings::validate_instance_overrides(&out)
        .map_err(|error| format!("HMCL 启动配置无法转换：{error}"))?;
    Ok(out)
}

fn boolean(fields: &Map<String, Value>, key: &str) -> Result<Option<bool>, String> {
    match fields.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Bool(value)) => Ok(Some(*value)),
        _ => Err(format!("HMCL 字段 {key} 必须为布尔值。")),
    }
}

fn number(fields: &Map<String, Value>, key: &str) -> Result<Option<u32>, String> {
    let Some(value) = fields.get(key).filter(|v| !v.is_null()) else {
        return Ok(None);
    };
    let number = value
        .as_u64()
        .or_else(|| value.as_str().and_then(|v| v.parse::<u64>().ok()))
        .or_else(|| {
            value
                .as_f64()
                .filter(|v| v.is_finite() && *v >= 0.0 && v.fract() == 0.0 && *v <= u32::MAX as f64)
                .map(|v| v as u64)
        });
    number
        .and_then(|v| u32::try_from(v).ok())
        .map(Some)
        .ok_or_else(|| format!("HMCL 数值字段 {key} 无效。"))
}

fn set_memory(out: &mut InstanceOverrides, memory: u32) -> Result<(), String> {
    if !(512..=262_144).contains(&memory) {
        return Err("整合包内存必须为 512–262144 MB。".into());
    }
    if out.memory_mode == Some(MemoryMode::Custom) && out.memory_mb != Some(memory) {
        return Err("整合包内存配置与 JVM 最大堆参数冲突。".into());
    }
    out.memory_mode = Some(MemoryMode::Custom);
    out.memory_mb = Some(memory);
    Ok(())
}

fn tokens(text: &str) -> Result<Vec<String>, String> {
    if text.len() > 4096 || text.chars().any(char::is_control) {
        return Err("整合包自定义参数过长或包含控制字符。".into());
    }
    let tokens = shlex::split(text).ok_or("整合包自定义参数引号不匹配。")?;
    if tokens.len() > 128 {
        return Err("整合包自定义参数数量过多。".into());
    }
    Ok(tokens)
}

pub(super) fn apply_jvm_args(out: &mut InstanceOverrides, text: &str) -> Result<(), String> {
    let mut accepted = Vec::new();
    let mut minimum_heap = None;
    for token in tokens(text)? {
        if let Some(heap) = token.strip_prefix("-Xms") {
            let bytes = heap_size(heap).ok_or("整合包 JVM 最小堆参数无效。")?;
            if minimum_heap.is_some_and(|prior| prior != bytes) {
                return Err("整合包最小堆参数冲突。".into());
            }
            if minimum_heap.replace(bytes).is_some() {
                continue;
            }
        }
        if let Some(heap) = token.strip_prefix("-Xmx") {
            let bytes = heap_size(heap).ok_or("整合包 JVM 最大堆参数无效。")?;
            if bytes % (1024 * 1024) != 0 {
                return Err("整合包最大堆必须为整数 MB。".into());
            }
            set_memory(
                out,
                u32::try_from(bytes / (1024 * 1024)).map_err(|_| "整合包最大堆过大。")?,
            )?;
            continue;
        }
        let numeric_vm = token
            .strip_prefix("-XX:")
            .and_then(|s| s.split_once('='))
            .is_some_and(|(key, value)| {
                [
                    "G1NewSizePercent",
                    "G1ReservePercent",
                    "MaxGCPauseMillis",
                    "G1HeapRegionSize",
                    "InitiatingHeapOccupancyPercent",
                    "ParallelGCThreads",
                    "ConcGCThreads",
                    "MaxTenuringThreshold",
                    "SurvivorRatio",
                    "G1MixedGCCountTarget",
                    "G1HeapWastePercent",
                    "G1MixedGCLiveThresholdPercent",
                    "G1RSetUpdatingPauseTimePercent",
                    "MaxDirectMemorySize",
                    "ReservedCodeCacheSize",
                    "SoftRefLRUPolicyMSPerMB",
                    "MaxMetaspaceSize",
                ]
                .contains(&key)
                    && heap_size(value).is_some()
            });
        let boolean_vm = token
            .strip_prefix("-XX:")
            .and_then(|s| s.strip_prefix('+').or_else(|| s.strip_prefix('-')))
            .is_some_and(|key| {
                [
                    "UnlockExperimentalVMOptions",
                    "UnlockDiagnosticVMOptions",
                    "DisableExplicitGC",
                    "AlwaysPreTouch",
                    "ParallelRefProcEnabled",
                    "UseStringDeduplication",
                    "PerfDisableSharedMem",
                    "UseNUMA",
                    "OmitStackTraceInFastThrow",
                ]
                .contains(&key)
            });
        let heap = token
            .strip_prefix("-Xms")
            .or_else(|| token.strip_prefix("-Xss"))
            .is_some_and(|v| heap_size(v).is_some());
        let property = token
            .strip_prefix("-D")
            .and_then(|s| s.split_once('='))
            .is_some_and(|(key, value)| match key {
                "file.encoding" | "sun.stdout.encoding" | "sun.stderr.encoding" => {
                    matches!(value, "UTF-8" | "UTF8")
                }
                "user.language" | "user.country" => {
                    !value.is_empty()
                        && value.len() <= 16
                        && value
                            .bytes()
                            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-'))
                }
                "forge.logging.console.level" => matches!(
                    value,
                    "debug" | "info" | "warn" | "error" | "fatal" | "off" | "trace"
                ),
                _ => false,
            });
        if !numeric_vm && !boolean_vm && !heap && !property {
            return Err("整合包包含不支持或不安全的 JVM 参数；禁止 agent、classpath、参数文件与任意系统属性。".into());
        }
        accepted.push(token);
    }
    if out.memory_mb.is_some_and(|maximum| {
        minimum_heap.is_some_and(|minimum| minimum > u64::from(maximum) * 1024 * 1024)
    }) {
        return Err("整合包最小堆大于最大堆。".into());
    }
    out.extra_jvm_args = Some(accepted.join(" "));
    Ok(())
}

fn heap_size(text: &str) -> Option<u64> {
    let (number, multiplier) = match text.as_bytes().last()?.to_ascii_lowercase() {
        b'k' => (&text[..text.len() - 1], 1024),
        b'm' => (&text[..text.len() - 1], 1024 * 1024),
        b'g' => (&text[..text.len() - 1], 1024 * 1024 * 1024),
        _ => (text, 1),
    };
    if number.is_empty() || !number.bytes().all(|c| c.is_ascii_digit()) {
        return None;
    }
    number
        .parse::<u64>()
        .ok()?
        .checked_mul(multiplier)
        .filter(|v| *v > 0 && *v <= 262_144 * 1024 * 1024)
}

pub(super) fn apply_game_args(out: &mut InstanceOverrides, text: &str) -> Result<(), String> {
    let args = tokens(text)?;
    let mut accepted = Vec::new();
    let mut i = 0;
    while i < args.len() {
        let flag = &args[i];
        match flag.as_str() {
            "--fullscreen" => out.fullscreen = Some(true),
            "--demo" | "--disableMultiplayer" | "--disableChat" => accepted.push(flag.clone()),
            "--width" | "--height" | "--server" | "--port" => {
                i += 1;
                let value = args.get(i).ok_or("整合包游戏参数缺少值。")?;
                match flag.as_str() {
                    "--width" => {
                        out.window_width = Some(value.parse().map_err(|_| "窗口宽度无效。")?)
                    }
                    "--height" => {
                        out.window_height = Some(value.parse().map_err(|_| "窗口高度无效。")?)
                    }
                    "--port" => {
                        if value.parse::<u16>().ok().filter(|v| *v > 0).is_none() {
                            return Err("整合包服务器端口无效。".into());
                        }
                        accepted.extend([flag.clone(), value.clone()]);
                    }
                    _ => {
                        if value.is_empty()
                            || value.starts_with('-')
                            || value.len() > 253
                            || !value
                                .bytes()
                                .all(|c| c.is_ascii_alphanumeric() || b".-:[]_".contains(&c))
                        {
                            return Err("整合包服务器地址无效。".into());
                        }
                        accepted.extend([flag.clone(), value.clone()]);
                    }
                }
            }
            _ => {
                return Err(
                    "整合包包含不支持或不安全的游戏参数；不会导入账户、Token、外部路径或参数文件。"
                        .into(),
                );
            }
        }
        i += 1;
    }
    out.game_args = Some(accepted.join(" "));
    Ok(())
}

pub(super) fn entry_kind(
    name: &str,
    is_dir: bool,
    layout: &Layout,
) -> Result<ArchiveEntryKind, String> {
    let logical = name.trim_end_matches('/');
    if layout.metadata.contains(logical)
        || (!is_dir && name == "modpack.json")
        || harmless_document(name, is_dir)
    {
        return Ok(ArchiveEntryKind::Index);
    }
    let game_prefix = prefix(&layout.game);
    if is_dir && logical == layout.game {
        return Ok(ArchiveEntryKind::Ignored);
    }
    let relative = name
        .strip_prefix(&game_prefix)
        .ok_or_else(|| format!("HMCL 包含游戏根目录之外的条目：{name}"))?;
    if let Some(version_root) = layout.version_root.as_ref() {
        if is_dir && (logical == *version_root || version_root.starts_with(&format!("{logical}/")))
        {
            return Ok(ArchiveEntryKind::Ignored);
        }
        if let Some(relative) = name.strip_prefix(&format!("{version_root}/")) {
            let id = version_root.rsplit('/').next().unwrap();
            if !is_dir && relative == format!("{id}.jar") {
                return Ok(ArchiveEntryKind::Ignored);
            }
            return game_entry(relative, is_dir);
        }
    }
    if relative.starts_with("versions/") && layout.version_root.is_some() {
        return Err("HMCL 包含选定版本之外的文件。".into());
    }
    game_entry(relative, is_dir)
}

fn game_entry(relative: &str, is_dir: bool) -> Result<ArchiveEntryKind, String> {
    let relative = relative.trim_end_matches('/');
    safe_pack_path(relative)?;
    let allowed = eligible_file(relative) || relative.starts_with("screenshots/");
    if is_dir
        && ([
            "mods",
            "config",
            "resourcepacks",
            "shaderpacks",
            "defaultconfigs",
            "kubejs",
            "saves",
            "screenshots",
        ]
        .contains(&relative)
            || allowed)
    {
        return Ok(ArchiveEntryKind::OverrideDirectory(relative.into()));
    }
    if !is_dir && allowed {
        return Ok(ArchiveEntryKind::Override {
            relative: relative.into(),
            client: false,
        });
    }
    // These files may contain credentials or executable launch instructions. They
    // are still streamed and budgeted by scan_archive, but are never extracted.
    if [
        ".hmcl",
        "assets",
        "libraries",
        "logs",
        "crash-reports",
        "natives",
    ]
    .iter()
    .any(|folder| relative == *folder || relative.starts_with(&format!("{folder}/")))
        || (!is_dir
            && matches!(
                relative,
                "hmclversion.cfg"
                    | "hmcl.json"
                    | "accounts.json"
                    | "launcher_accounts.json"
                    | "launcher_profiles.json"
                    | "usercache.json"
                    | "usernamecache.json"
            ))
    {
        return Ok(ArchiveEntryKind::Ignored);
    }
    Err(format!("HMCL 游戏文件 {relative} 不在安全导入范围内。"))
}
