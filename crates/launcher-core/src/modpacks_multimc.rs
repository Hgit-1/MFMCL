//! Component-based MultiMC conversion and bounded snapshots shared by local pack formats.
//! Custom patches are not silently discarded. Standard safe instance preferences
//! follow HMCL v3.16.4 `MultiMCInstanceConfiguration`; commands/Java paths are ignored.
//! Format references: MultiMC/Launcher `PackProfile.cpp`, `MinecraftInstance::gameRoot`,
//! HMCL `MultiMCComponents.java` (including the NeoForge and LiteLoader component UIDs).
use super::*;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Components {
    format_version: u32,
    components: Vec<Component>,
}

#[derive(Deserialize)]
struct Component {
    uid: String,
    #[serde(default)]
    version: String,
    #[serde(default)]
    disabled: bool,
}

pub(super) struct Snapshot(PathBuf);
impl Snapshot {
    pub(super) fn archive_path(&self) -> PathBuf {
        self.0.join("instance.zip")
    }
}
impl Drop for Snapshot {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

pub(super) fn snapshot_directory(root: &Path) -> Result<Snapshot, String> {
    if root.join("mmc-pack.json").exists() {
        for name in ["mmc-pack.json", "instance.cfg"] {
            let meta = fs::symlink_metadata(root.join(name)).map_err(
                |_| "请选择包含 mmc-pack.json 与 instance.cfg 的标准 MultiMC 实例目录。",
            )?;
            if !meta.file_type().is_file() || meta.len() > MAX_INDEX {
                return Err("MultiMC 索引必须为有限大小的普通文件。".into());
            }
        }
    } else if ![
        "modpack.json",
        "pack.json",
        "versions",
        "hmclversion.cfg",
        ".hmcl/config/instance-game-settings.json",
    ]
    .iter()
    .any(|name| root.join(name).exists())
    {
        return Err("请选择标准 MultiMC 实例或包含 HMCL 版本元数据的目录。".into());
    }
    let directory =
        std::env::temp_dir().join(format!("mfmcl-instance-snapshot-{}", uuid::Uuid::new_v4()));
    fs::create_dir(&directory).map_err(|e| e.to_string())?;
    let snapshot = Snapshot(directory);
    let mut writer = ZipWriter::new(
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(snapshot.archive_path())
            .map_err(|e| e.to_string())?,
    );
    snapshot_tree(
        root,
        "",
        &mut writer,
        &mut PathRegistry::default(),
        &mut 0,
        &mut 0,
        0,
    )?;
    writer.finish().map_err(|e| e.to_string())?;
    Ok(snapshot)
}

fn snapshot_tree(
    root: &Path,
    relative: &str,
    writer: &mut ZipWriter<File>,
    paths: &mut PathRegistry,
    count: &mut usize,
    total: &mut u64,
    depth: usize,
) -> Result<(), String> {
    if depth > 32 {
        return Err("已有实例目录层级过深。".into());
    }
    for item in fs::read_dir(root).map_err(|e| e.to_string())? {
        let item = item.map_err(|e| e.to_string())?;
        let name = item
            .file_name()
            .into_string()
            .map_err(|_| "已有实例文件名必须为 Unicode。")?;
        let name = if relative.is_empty() {
            name
        } else {
            format!("{relative}/{name}")
        };
        safe_pack_path(&name)?;
        *count += 1;
        if *count > MAX_ARCHIVE_ENTRIES {
            return Err("已有实例文件数量超过限制。".into());
        }
        let meta = fs::symlink_metadata(item.path()).map_err(|e| e.to_string())?;
        if meta.file_type().is_symlink() || (!meta.is_dir() && !meta.is_file()) {
            return Err("已有实例包含符号链接或非普通文件。".into());
        }
        paths.insert(&name, meta.is_dir())?;
        if meta.is_dir() {
            writer
                .add_directory(format!("{name}/"), SimpleFileOptions::default())
                .map_err(|e| e.to_string())?;
            snapshot_tree(&item.path(), &name, writer, paths, count, total, depth + 1)?;
        } else {
            if meta.len() > MAX_ENTRY {
                return Err("已有实例单个文件超过限制。".into());
            }
            writer
                .start_file(
                    &name,
                    SimpleFileOptions::default()
                        .compression_method(zip::CompressionMethod::Deflated),
                )
                .map_err(|e| e.to_string())?;
            let mut source = File::open(item.path()).map_err(|e| e.to_string())?;
            copy_bounded(&mut source, writer, meta.len(), MAX_ENTRY, total)?;
        }
    }
    Ok(())
}

pub(super) fn read_index(
    archive: &mut ZipArchive<File>,
    root: &str,
) -> Result<(PackIndex, String, Option<crate::InstanceOverrides>), String> {
    let bytes = read_archive_bytes(archive, &format!("{root}mmc-pack.json"))?;
    let manifest: Components =
        serde_json::from_slice(&bytes).map_err(|_| "MultiMC 组件索引格式无效。")?;
    if manifest.format_version != 1 || manifest.components.len() > 32 {
        return Err("仅支持 MultiMC 组件格式版本 1。".into());
    }
    let cfg = read_archive_bytes(archive, &format!("{root}instance.cfg"))?;
    let cfg = std::str::from_utf8(&cfg).map_err(|_| "MultiMC instance.cfg 必须使用 UTF-8。")?;
    let mut name = None;
    let mut properties = std::collections::BTreeMap::new();
    for line in cfg.lines() {
        let line = line.trim();
        if line.starts_with(['#', ';', '[']) || line.is_empty() {
            continue;
        }
        if let Some((key, value)) = line.split_once('=') {
            let (key, value) = (key.trim(), value.trim());
            if properties.insert(key, value).is_some() {
                return Err("MultiMC instance.cfg 包含重复配置项。".into());
            }
            match key {
                "name" => name = Some(value.to_owned()),
                "InstanceType" if value != "OneSix" => {
                    return Err("仅支持现代 MultiMC OneSix 实例。".into());
                }
                "OverrideMCLaunchMethod" if value.eq_ignore_ascii_case("true") => {
                    return Err("MultiMC 自定义启动方式无法安全转换。".into());
                }
                _ => {}
            }
        }
    }
    let overrides = read_overrides(&properties)?;
    let name = name.filter(|name| !name.is_empty()).unwrap_or_else(|| {
        root.trim_end_matches('/')
            .rsplit('/')
            .next()
            .filter(|part| !part.is_empty())
            .unwrap_or("MultiMC instance")
            .to_owned()
    });
    let legacy_fabric = manifest
        .components
        .iter()
        .any(|component| !component.disabled && component.uid == "net.legacyfabric.intermediary");
    let mut dependencies = std::collections::HashMap::new();
    let mut seen = BTreeSet::new();
    for component in manifest.components {
        if !seen.insert(component.uid.clone()) {
            return Err("MultiMC 组件重复。".into());
        }
        if component.disabled {
            continue;
        }
        let key = match component.uid.as_str() {
            "net.minecraft" => "minecraft",
            "net.minecraftforge" => "forge",
            "net.neoforged" => "neoforge",
            "net.fabricmc.fabric-loader" if legacy_fabric => "legacyfabric-loader",
            "net.fabricmc.fabric-loader" => "fabric-loader",
            "org.quiltmc.quilt-loader" => "quilt-loader",
            "com.mumfrey.liteloader" => "liteloader",
            "com.cleanroommc.cleanroom" => "cleanroom",
            "net.legacyfabric.fabric-loader" => "legacyfabric-loader",
            "org.lwjgl"
            | "org.lwjgl3"
            | "net.fabricmc.intermediary"
            | "org.quiltmc.hashed"
            | "net.legacyfabric.intermediary" => {
                continue;
            }
            other => {
                return Err(format!(
                    "不支持 MultiMC 自定义组件 {other}；不会创建不完整实例。"
                ));
            }
        };
        safe_version(&component.version)?;
        if dependencies
            .insert(key.to_owned(), component.version)
            .is_some()
        {
            return Err("MultiMC 组件映射到重复的加载器。".into());
        }
    }
    let mut roots = BTreeSet::new();
    for i in 0..archive.len() {
        let entry = archive.by_index(i).map_err(|e| e.to_string())?;
        let name = entry.name().map_err(|_| "MultiMC 文件名无法解码。")?;
        if let Some(name) = name.strip_prefix(root) {
            for candidate in ["minecraft", ".minecraft"] {
                if name.trim_end_matches('/') == candidate
                    || name.starts_with(&format!("{candidate}/"))
                {
                    roots.insert(candidate.to_owned());
                }
            }
        }
    }
    if roots.len() > 1 {
        return Err("MultiMC 同时包含 minecraft 与 .minecraft，无法确定游戏目录。".into());
    }
    let game = roots
        .into_iter()
        .next()
        .unwrap_or_else(|| ".minecraft".into());
    Ok((
        PackIndex {
            format_version: 1,
            game: "minecraft".into(),
            version_id: "imported".into(),
            name,
            files: Vec::new(),
            dependencies,
        },
        game,
        overrides,
    ))
}

fn read_overrides(
    properties: &std::collections::BTreeMap<&str, &str>,
) -> Result<Option<crate::InstanceOverrides>, String> {
    let flag = |key: &str| -> Result<bool, String> {
        match properties.get(key) {
            None => Ok(false),
            Some(value) if value.eq_ignore_ascii_case("true") => Ok(true),
            Some(value) if value.eq_ignore_ascii_case("false") => Ok(false),
            _ => Err(format!("MultiMC 布尔配置 {key} 无效。")),
        }
    };
    let mut fields = serde_json::Map::new();
    if flag("OverrideMemory")? {
        for (source, target) in [("MaxMemAlloc", "maxMemory"), ("MinMemAlloc", "minMemory")] {
            if let Some(value) = properties.get(source) {
                fields.insert(target.into(), serde_json::Value::String((*value).into()));
            }
        }
    }
    if flag("OverrideWindow")? {
        fields.insert("fullscreen".into(), flag("LaunchMaximized")?.into());
        for (source, target) in [
            ("MinecraftWinWidth", "width"),
            ("MinecraftWinHeight", "height"),
        ] {
            if let Some(value) = properties.get(source) {
                fields.insert(target.into(), serde_json::Value::String((*value).into()));
            }
        }
    }
    if flag("OverrideJavaArgs")? {
        fields.insert(
            "javaArgs".into(),
            properties.get("JvmArgs").copied().unwrap_or("").into(),
        );
    }
    if flag("OverrideConsole")? {
        fields.insert("showLogs".into(), flag("ShowConsole")?.into());
    }
    // Commands and local Java locations are intentionally absent from this map.
    let overrides = hmcl::settings_to_overrides(&serde_json::Value::Object(fields))?;
    Ok((overrides != crate::InstanceOverrides::default()).then_some(overrides))
}

pub(super) fn entry_kind(name: &str, is_dir: bool, game: &str) -> Result<ArchiveEntryKind, String> {
    if !is_dir && matches!(name, "mmc-pack.json" | "instance.cfg")
        || harmless_document(name, is_dir)
    {
        return Ok(ArchiveEntryKind::Index);
    }
    if is_dir && name.trim_end_matches('/') == game {
        return Ok(ArchiveEntryKind::Ignored);
    }
    if name.starts_with("patches/")
        || name.starts_with("jarmods/")
        || matches!(name.trim_end_matches('/'), "patches" | "jarmods")
    {
        return Err("MultiMC 自定义补丁或 JAR 修改尚不支持转换。".into());
    }
    let relative = name
        .strip_prefix(&format!("{game}/"))
        .ok_or_else(|| format!("MultiMC 包含不支持的实例条目：{name}"))?;
    let relative = relative.trim_end_matches('/');
    safe_pack_path(relative)?;
    let allowed = eligible_file(relative)
        || matches!(relative, "options.txt" | "optionsof.txt" | "servers.dat")
        || relative.starts_with("saves/")
        || relative.starts_with("screenshots/");
    if is_dir {
        if [
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
            || allowed
        {
            return Ok(ArchiveEntryKind::OverrideDirectory(relative.to_owned()));
        }
    } else if allowed {
        return Ok(ArchiveEntryKind::Override {
            relative: relative.to_owned(),
            client: false,
        });
    }
    // Regenerable runtime/cache data is never imported as trusted launch metadata.
    if [
        "logs",
        "crash-reports",
        "assets",
        "libraries",
        "versions",
        "natives",
    ]
    .iter()
    .any(|folder| relative == *folder || relative.starts_with(&format!("{folder}/")))
    {
        return Ok(ArchiveEntryKind::Ignored);
    }
    Err(format!("MultiMC 游戏文件 {relative} 不在安全导入范围内。"))
}
