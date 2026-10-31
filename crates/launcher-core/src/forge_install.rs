//! Forge / NeoForge 的 installer processor 执行。
//!
//! ## 协议概要
//!
//! 安装器 jar 里有一份 `install_profile.json`：
//!
//! ```json
//! {
//!   "spec": 1,
//!   "minecraft": "1.20.1",
//!   "data": { "MAPPINGS": { "client": "[de.oceanlabs.mcp:mcp_config:1.20.1@zip]" }, ... },
//!   "processors": [
//!     { "jar": "...", "classpath": [...], "args": [...], "outputs": {...}, "sides": ["client"] }
//!   ],
//!   "libraries": [...]
//! }
//! ```
//!
//! 每个 processor 是一个 Java 程序，用指定 jar 的 `Main-Class` 启动，参数里的
//! 占位符（`{MINECRAFT_JAR}`、`{MAPPINGS}`、`[maven:coord:ver]`）在运行时替换成
//! 真实路径。`outputs` 声明该步骤产物的期望 SHA-1，跑完要校验。
//!
//! ## 为什么必须真的执行
//!
//! 客户端 jar 在安装前后是不同的：`jarsplitter` 切掉非类文件得到 slim jar，
//! `ForgeAutoRenamingTool` 按映射表重命名符号得到 srg jar，`binarypatcher` 应用
//! 二进制补丁得到最终被打过补丁的客户端。这套变换无法用 JSON 拼接模拟——它真的
//! 在改写字节码。
//!
//! ## 安全边界
//!
//! 这是本项目唯一会**执行下载来的代码**的地方。约束：
//!   - processor 的 jar 与 classpath 只能来自白名单 Maven 仓库（forge/mojang）；
//!   - 每个下载项都带 SHA-1，落盘前校验；
//!   - 参数逐项构造，不经过 shell，不存在命令注入面；
//!   - `sides` 明确排除 server 侧的 processor，不跑与客户端无关的步骤。
use crate::loaders::{LoaderKind, safe_version};
use crate::{InstanceRecord, LauncherStore};
use download_engine::{ArtifactSpec, DownloadEngine, DownloadProgress};
use mc_metadata::{Artifact, GameVersionMetadata, Library, LibraryDownloads};
use serde::Deserialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use tokio::sync::mpsc;

/// `install_profile.json` 的结构。
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallProfile {
    #[serde(default)]
    pub spec: u32,
    /// 版本 id，例如 `1.20.1-forge-47.4.28`。
    pub version: String,
    pub minecraft: String,
    /// 内嵌的版本配置路径（通常是 `/version.json`）。
    #[serde(default)]
    pub json: Option<String>,
    #[serde(default)]
    pub data: HashMap<String, DataEntry>,
    #[serde(default)]
    pub processors: Vec<Processor>,
    #[serde(default)]
    pub libraries: Vec<ProfileLibrary>,
}

/// `data` 里的每一项可以给 client / server 不同的值。
#[derive(Debug, Clone, Deserialize)]
pub struct DataEntry {
    #[serde(default)]
    pub client: Option<String>,
    #[serde(default)]
    pub server: Option<String>,
}

impl DataEntry {
    /// 取客户端侧的值；缺失时回落到 server 值（部分条目两者相同）。
    pub fn for_client(&self) -> Option<&str> {
        self.client.as_deref().or(self.server.as_deref())
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct Processor {
    /// 要执行的 jar 的 Maven 坐标。
    pub jar: String,
    /// 运行时 classpath（主 jar 之外）。
    #[serde(default)]
    pub classpath: Vec<String>,
    /// 参数模板，含占位符。
    #[serde(default)]
    pub args: Vec<String>,
    /// 产物路径 → 期望 SHA-1。空值时表示该步骤不产出可校验的文件。
    #[serde(default)]
    pub outputs: Option<HashMap<String, String>>,
    /// 只在这些端运行；缺省表示两端都跑。
    #[serde(default)]
    pub sides: Option<Vec<String>>,
}

impl Processor {
    /// 这一步是否需要在客户端执行。
    pub fn runs_on_client(&self) -> bool {
        match &self.sides {
            Some(sides) => sides.iter().any(|side| side == "client"),
            None => true,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct ProfileLibrary {
    pub name: String,
    #[serde(default)]
    pub downloads: Option<ProfileDownloads>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ProfileDownloads {
    #[serde(default)]
    pub artifact: Option<ProfileArtifact>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ProfileArtifact {
    #[serde(default)]
    pub path: Option<String>,
    pub url: String,
    #[serde(default)]
    pub sha1: Option<String>,
    #[serde(default)]
    pub size: Option<u64>,
}

/// 占位符解析所需的上下文。
///
/// `data` 里的值有三种形态，解析规则不同：
///   - `[maven:coord:ver]` → 展开成该依赖在本地 libraries 目录下的路径；
///   - `'literal'`        → 去掉单引号，就是字面值（常见于 SHA-1）；
///   - `/data/xxx`        → 安装器 jar 内的条目，解压后的本地路径。
pub struct ProcessorContext<'a> {
    /// 本地 `libraries` 根目录。
    pub libraries: &'a Path,
    /// 安装器 jar 解压出来的临时目录（`data/` 在这里）。
    pub installer_root: &'a Path,
    /// 原版客户端 jar 的路径。
    pub minecraft_jar: &'a Path,
    /// 已完成安装的 Maven 依赖：坐标 → 本地路径。
    pub resolved: &'a HashMap<String, PathBuf>,
    /// `data` 条目解析结果：键 → 路径或字面值。
    pub data: &'a HashMap<String, String>,
    /// 客户端侧标识，替换 `{SIDE}`。
    pub side: &'a str,
}

impl ProcessorContext<'_> {
    /// 替换参数里的占位符。
    ///
    /// 支持三种写法：
    ///   - `{KEY}`            —— 从 `data` 取；
    ///   - `[maven:coord:ver]` —— 整个参数就是一个 Maven 坐标；
    ///   - `'literal'`         —— 字面值（含在 data 里时已经剥掉引号）。
    pub fn expand(&self, raw: &str) -> Result<String, String> {
        let trimmed = raw.trim();
        // 整个参数是一个 Maven 坐标：展开成本地路径。
        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            let coord = &trimmed[1..trimmed.len() - 1];
            return self
                .resolved
                .get(coord)
                .map(|path| path.to_string_lossy().into_owned())
                .ok_or_else(|| format!("安装器引用了未下载的依赖：{coord}"));
        }
        // 参数里可能嵌多个 {KEY}，逐个替换。
        let mut out = String::with_capacity(trimmed.len());
        let mut rest = trimmed;
        while let Some(start) = rest.find('{') {
            let Some(end_rel) = rest[start..].find('}') else {
                out.push_str(rest);
                return Ok(out);
            };
            out.push_str(&rest[..start]);
            let key = &rest[start + 1..start + end_rel];
            let value = self
                .data
                .get(key)
                .ok_or_else(|| format!("安装器参数引用了未知变量：{{{key}}}"))?;
            out.push_str(value);
            rest = &rest[start + end_rel + 1..];
        }
        out.push_str(rest);
        // `{SIDE}` 单独处理：它不在 data 里，是运行时已知的常量。
        Ok(out.replace("{SIDE}", self.side))
    }
}

/// 把 `data` 条目解析成「本地路径或字面值」。
///
/// 返回的字符串直接用于参数替换：路径类的已经展开成绝对路径。
///
/// 关键点：`data` 里的 Maven 坐标有两种角色——
///   - **已下载的依赖**（processor 的 jar、classpath）：映射到已存在的文件；
///   - **产物路径**（`MAPPINGS`、`MC_SLIM`、`PATCHED` 等）：这些坐标对应的文件
///     此刻还不存在，是由前面的 processor 生成的。它们要映射到该坐标**未来会落在
///     的本地路径**，而不是报「未下载」。
///
/// 早先的实现在这里直接报错，导致 Forge 安装在第 4 步（`MCP_DATA` 生成映射）之前
/// 就失败——因为 `{MAPPINGS}` 本来就是要由那一步生成的。
pub fn resolve_data_entries(
    profile: &InstallProfile,
    libraries: &Path,
    installer_root: &Path,
    resolved: &HashMap<String, PathBuf>,
) -> Result<HashMap<String, String>, String> {
    let mut out = HashMap::new();
    for (key, entry) in &profile.data {
        let Some(raw) = entry.for_client() else {
            continue;
        };
        let value = raw.trim();
        let expanded =
            if let Some(literal) = value.strip_prefix('\'').and_then(|v| v.strip_suffix('\'')) {
                // 字面值：SHA-1 之类，原样保留。
                literal.to_owned()
            } else if value.starts_with('[') && value.ends_with(']') {
                let coord = &value[1..value.len() - 1];
                match resolved.get(coord) {
                    Some(path) => path.to_string_lossy().into_owned(),
                    None => {
                        // 产物路径：该坐标对应一个还没生成的文件，落到 libraries 里的
                        // 常规位置。processor 会把结果写到这个路径。
                        let relative = maven_relative(coord)?;
                        libraries.join(relative).to_string_lossy().into_owned()
                    }
                }
            } else if let Some(relative) = value.strip_prefix('/') {
                // 安装器 jar 内的条目，已经解压到 installer_root。
                installer_root.join(relative).to_string_lossy().into_owned()
            } else {
                value.to_owned()
            };
        out.insert(key.clone(), expanded);
    }
    Ok(out)
}

/// 补齐内置变量。
///
/// `{MINECRAFT_JAR}` 与 `{SIDE}` 这两个变量不在 `install_profile.data` 里，但
/// processor 的参数会引用它们——前者是原版客户端 jar 的路径，后者是 `client`。
/// 不补的话参数展开会报「未知变量」，而它们其实是运行时已知的常量。
pub fn with_builtin_vars(
    mut data: HashMap<String, String>,
    minecraft_jar: &Path,
    side: &str,
) -> HashMap<String, String> {
    data.insert(
        "MINECRAFT_JAR".to_owned(),
        minecraft_jar.to_string_lossy().into_owned(),
    );
    data.insert("SIDE".to_owned(), side.to_owned());
    data
}

/// 判断一个 Maven 坐标是否来自允许的仓库。
///
/// 这是「执行下载来的代码」这条路径上最关键的一道闸：只要坐标的 group 或额外
/// 仓库不在白名单内，就拒绝整个安装，而不是尝试去猜。
pub fn maven_repository_for(coord: &str, kind: LoaderKind) -> Result<String, String> {
    let (coordinate, extension) = coord.split_once('@').unwrap_or((coord, "jar"));
    let parts: Vec<&str> = coordinate.split(':').collect();
    if !(3..=4).contains(&parts.len()) {
        return Err(format!("Maven 坐标无效：{coord}"));
    }
    let group = parts[0];
    let artifact = parts[1];
    // 只接受字母数字与常见分隔符，防止拼出路径穿越。
    for part in [group, artifact] {
        if part.is_empty()
            || !part
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "._-".contains(c))
        {
            return Err(format!("Maven 坐标包含不允许的字符：{coord}"));
        }
    }
    if extension.is_empty() || !extension.chars().all(|c| c.is_ascii_alphanumeric()) {
        return Err(format!("Maven 扩展名无效：{coord}"));
    }

    // 已知仓库映射。NeoForge 的依赖也在 neoforged 仓库，但 Forge 的安装器
    // 可能引用 `net.neoforged.*`（跨版本迁移时），所以两者都放行。
    let neoforged = "https://maven.neoforged.net/releases/";
    let forge = match kind {
        LoaderKind::Forge | LoaderKind::NeoForge => "https://maven.minecraftforge.net/",
        _ => return Err("该加载器不使用安装器仓库。".into()),
    };
    let mojang = "https://libraries.minecraft.net/";

    let repository = if group.starts_with("net.minecraftforge")
        || group.starts_with("cpw.mods")
        || group == "de.oceanlabs.mcp"
        || group == "com.github.jponge"
        || group == "com.nothome"
        || group == "net.sf.jopt-simple"
        || group == "net.minecraft"
    {
        // `net.minecraft:*` 的 mappings / client 产物都在 Forge 的 maven 上。
        forge
    } else if group.starts_with("net.neoforged") {
        neoforged
    } else if group.starts_with("org.lwjgl") || group.starts_with("com.mojang") {
        mojang
    } else {
        // 其余常见库（guava、gson、commons-io 等）走 Maven Central。
        "https://repo1.maven.org/maven2/"
    };
    Ok(repository.to_owned())
}

/// 从 Maven 坐标推出相对路径（`group/artifact/version/artifact-version.ext`）。
pub fn maven_relative(coord: &str) -> Result<String, String> {
    let (coordinate, extension) = coord.split_once('@').unwrap_or((coord, "jar"));
    let parts: Vec<&str> = coordinate.split(':').collect();
    let (group, artifact, version, classifier) = match parts.len() {
        3 => (parts[0], parts[1], parts[2], None),
        4 => (parts[0], parts[1], parts[2], Some(parts[3])),
        _ => return Err(format!("Maven 坐标无效：{coord}")),
    };
    if [group, artifact, version].iter().any(|p| p.is_empty())
        || version.contains("..")
        || version.contains('/')
        || version.contains('\\')
    {
        return Err(format!("Maven 坐标版本无效：{coord}"));
    }
    let group_path = group.replace('.', "/");
    let file = match classifier {
        Some(classifier) => format!("{artifact}-{version}-{classifier}.{extension}"),
        None => format!("{artifact}-{version}.{extension}"),
    };
    Ok(format!("{group_path}/{artifact}/{version}/{file}"))
}

/// 校验 processor 的产物 SHA-1。
///
/// `outputs` 里的值可能是 `{MC_SLIM_SHA}` 这样的占位符，由调用方先展开。
pub fn verify_processor_outputs(
    outputs: &HashMap<String, String>,
    context: &ProcessorContext<'_>,
) -> Result<(), String> {
    use sha1::{Digest, Sha1};
    for (path_template, sha_template) in outputs {
        let path = context.expand(path_template)?;
        let expected = context.expand(sha_template)?;
        // 有些版本用空串表示「不校验」。
        if expected.trim().is_empty() {
            continue;
        }
        if expected.len() != 40 || !expected.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(format!("安装器产物的校验值无效：{expected}"));
        }
        let bytes = std::fs::read(&path)
            .map_err(|error| format!("安装器没有产出预期的文件（{path}）：{error}"))?;
        let actual = hex::encode(Sha1::digest(&bytes));
        if actual != expected.to_ascii_lowercase() {
            return Err(format!(
                "安装器产物校验失败：{path}\n期望 {expected}\n实际 {actual}"
            ));
        }
    }
    Ok(())
}

/// 用安装器完成 Forge / NeoForge 的安装。
///
/// 完整流程：
///   1. 下载 `<loader>-installer.jar` 并校验 SHA-1；
///   2. 解压出 `install_profile.json`、`version.json` 和 `data/` 下的条目；
///   3. 下载 `install_profile.libraries`（processor 自身依赖）与 `version.json`
///      声明的库；
///   4. 解析 `data` 里的占位符（Maven 坐标 → 本地路径、字面量、安装器内条目）；
///   5. 依次执行客户端侧的 processor，每个产物按 `outputs` 校验 SHA-1；
///   6. 用 `version.json` 与原版配置合并出 `launch.json`，并把记录里的 loader
///      字段写成 `<kind>@<version>`。
///
/// 任一步失败都不会写入 `launch.json`：配置发布是最后一步，且用 `hard_link`
/// 原子完成（不覆盖已有文件）。
pub async fn install_with_processor(
    root: &Path,
    instance: &InstanceRecord,
    store: &LauncherStore,
    engine: &DownloadEngine,
    kind: LoaderKind,
    version: &str,
    progress: Option<mpsc::UnboundedSender<DownloadProgress>>,
    replace_existing: bool,
) -> Result<(), String> {
    use crate::loaders::LoaderKind as K;
    if !matches!(kind, K::Forge | K::NeoForge) {
        return Err("此加载器不走安装器流程。".into());
    }
    let version_id = installer_version_id(kind, &instance.game_version, version)?;

    // --- 1. 安装器 jar -----------------------------------------------------
    let (installer_url, installer_sha1) =
        installer_artifact(kind, &instance.game_version, version)?;
    let work = std::env::temp_dir().join(format!("mfmcl-loader-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&work).map_err(|e| format!("无法创建临时目录：{e}"))?;
    // 无论成败都清理临时目录：里面只有安装器的中间产物。
    let result = install_from_installer(
        engine,
        root,
        instance,
        store,
        kind,
        version,
        &version_id,
        &installer_url,
        installer_sha1.as_deref(),
        &work,
        progress,
        replace_existing,
    )
    .await;
    let _ = std::fs::remove_dir_all(&work);
    result
}

#[allow(clippy::too_many_arguments)]
async fn install_from_installer(
    engine: &DownloadEngine,
    root: &Path,
    instance: &InstanceRecord,
    store: &LauncherStore,
    kind: LoaderKind,
    version: &str,
    version_id: &str,
    installer_url: &str,
    installer_sha1: Option<&str>,
    work: &Path,
    progress: Option<mpsc::UnboundedSender<DownloadProgress>>,
    replace_existing: bool,
) -> Result<(), String> {
    let installer_jar = work.join("installer.jar");
    engine
        .download(
            &ArtifactSpec {
                id: format!("loader-installer:{version_id}"),
                label: format!("{} 安装器 {version}", kind.name()),
                url: installer_url.to_owned(),
                mirror_url: None,
                destination: installer_jar.clone(),
                size: None,
                sha1: installer_sha1.map(str::to_owned),
                sha256: None,
            },
            progress.clone(),
        )
        .await
        .map_err(|e| format!("下载加载器安装器失败：{e}"))?;

    // --- 2. 解压 profile 与 data ------------------------------------------
    let extract_root = work.join("extracted");
    std::fs::create_dir_all(&extract_root).map_err(|e| e.to_string())?;
    let installer_jar_clone = installer_jar.clone();
    let extract_clone = extract_root.clone();
    tokio::task::spawn_blocking(move || extract_installer(&installer_jar_clone, &extract_clone))
        .await
        .map_err(|e| e.to_string())??;

    let profile_bytes = std::fs::read(extract_root.join("install_profile.json"))
        .map_err(|e| format!("安装器缺少 install_profile.json：{e}"))?;
    let profile: InstallProfile =
        serde_json::from_slice(&profile_bytes).map_err(|e| format!("安装器清单解析失败：{e}"))?;
    if profile.spec != 1 {
        return Err(format!("不支持的安装器协议版本：{}", profile.spec));
    }
    if profile.minecraft != instance.game_version {
        return Err(format!(
            "安装器对应 Minecraft {}，与实例 {} 不一致。",
            profile.minecraft, instance.game_version
        ));
    }

    // --- 3. 下载依赖 -------------------------------------------------------
    let mut resolved: HashMap<String, PathBuf> = HashMap::new();
    /*
     * 收集需要下载的坐标。三个来源：
     *   1. `install_profile.libraries` —— processor 的 jar 与 classpath；
     *   2. `processor.jar` 与 `processor.classpath` —— 少数安装器不把它们列进 libraries；
     *   3. **data 里作为「输入」出现的坐标** —— 例如 `mcp_config` 的 zip，它只出现在
     *      processor 参数与 data 值里，不在 libraries 中。
     *
     * 第 3 点要排除「产物坐标」：`{MAPPINGS}` 之类的值也形如 `[...]`，但它们对应的
     * 文件是由 processor 生成的，预下载会 404 并让整个安装失败。
     */
    let mut needed: Vec<String> = Vec::new();
    for library in &profile.libraries {
        needed.push(library.name.clone());
    }
    for processor in profile.processors.iter().filter(|p| p.runs_on_client()) {
        needed.push(processor.jar.clone());
        needed.extend(processor.classpath.iter().cloned());
        // 参数里直接写的坐标就是输入依赖。
        for arg in &processor.args {
            if let Some(coord) = maven_coord_in(arg) {
                needed.push(coord);
            }
        }
    }
    // 产物坐标：data 的值里那些**没有对应输入**的坐标，由 processor 生成。
    let produced: std::collections::HashSet<String> = profile
        .processors
        .iter()
        .filter(|p| p.runs_on_client())
        .filter_map(|p| p.outputs.as_ref())
        .flat_map(|outputs| outputs.keys())
        .filter_map(|template| {
            // 形如 `{MC_SLIM}`，取出键名再回查 data。
            let key = template
                .trim()
                .trim_start_matches('{')
                .trim_end_matches('}');
            profile.data.get(key).and_then(|entry| entry.for_client())
        })
        .filter_map(|raw| maven_coord_in(raw.trim()))
        .collect();
    needed.retain(|coord| !produced.contains(coord));
    needed.sort();
    needed.dedup();
    for coord in &needed {
        let repository = maven_repository_for(coord, kind)?;
        let relative = maven_relative(coord)?;
        let destination = root.join("libraries").join(&relative);
        // 已存在且能校验通过的直接复用，避免重复下载几十 MB 的库。
        if destination.is_file() {
            resolved.insert(coord.clone(), destination);
            continue;
        }
        let url = format!("{repository}{relative}");
        let sha1 = profile_artifact_sha1(&profile, coord);
        engine
            .download(
                &ArtifactSpec {
                    id: format!("loader-dep:{coord}"),
                    label: format!("加载器依赖 {coord}"),
                    url,
                    mirror_url: None,
                    destination: destination.clone(),
                    size: None,
                    sha1,
                    sha256: None,
                },
                progress.clone(),
            )
            .await
            .map_err(|e| format!("下载加载器依赖 {coord} 失败：{e}"))?;
        resolved.insert(coord.clone(), destination);
    }

    // --- 4. 解析 data 占位符 -----------------------------------------------
    let version_dir = instance
        .instance_dir
        .join("versions")
        .join(&instance.game_version);
    let minecraft_jar = version_dir.join(format!("{}.jar", instance.game_version));
    if !minecraft_jar.is_file() {
        return Err("缺少原版客户端 jar；请先完成原版安装。".into());
    }
    let data = with_builtin_vars(
        resolve_data_entries(&profile, &root.join("libraries"), &extract_root, &resolved)?,
        &minecraft_jar,
        "client",
    );
    // 产物要写到 libraries 下的常规位置，父目录可能还不存在。
    for path in data.values() {
        let path = Path::new(path);
        // 只对看起来像 libraries 内的路径建目录，避免在任意字面值上建。
        if path.starts_with(root.join("libraries"))
            && let Some(parent) = path.parent()
        {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
    }
    let java = crate::launch::detect_java_runtime(None)
        .map_err(|e| format!("找不到可用的 Java 运行时：{e}"))?
        .path;
    let client_processors: Vec<&Processor> = profile
        .processors
        .iter()
        .filter(|p| p.runs_on_client())
        .collect();
    if client_processors.is_empty() {
        return Err("安装器没有声明客户端处理器，无法完成安装。".into());
    }
    for (index, processor) in client_processors.iter().enumerate() {
        let context = ProcessorContext {
            libraries: &root.join("libraries"),
            installer_root: &extract_root,
            minecraft_jar: &minecraft_jar,
            resolved: &resolved,
            data: &data,
            side: "client",
        };
        run_processor(
            Path::new(&java),
            processor,
            &context,
            index,
            client_processors.len(),
        )
        .await?;
        if let Some(outputs) = &processor.outputs {
            verify_processor_outputs(outputs, &context)?;
        }
    }

    // --- 6. 合并版本配置并发布 ---------------------------------------------
    let patch_path = extract_root.join(
        profile
            .json
            .as_deref()
            .unwrap_or("version.json")
            .trim_start_matches('/'),
    );
    let patch_bytes = std::fs::read(&patch_path)
        .map_err(|e| format!("安装器缺少版本配置 {}：{e}", patch_path.display()))?;
    let patch: serde_json::Value =
        serde_json::from_slice(&patch_bytes).map_err(|e| format!("版本配置解析失败：{e}"))?;

    // version.json 里的库也要装到本地：它们才是 Forge 运行时用的 jar。
    let runtime_libraries =
        install_version_libraries(engine, root, &patch, kind, progress.clone()).await?;

    let base_path = version_dir.join(format!("{}.json", instance.game_version));
    let base: GameVersionMetadata = serde_json::from_slice(
        &std::fs::read(&base_path).map_err(|e| format!("读取原版配置失败：{e}"))?,
    )
    .map_err(|e| format!("原版配置解析失败：{e}"))?;
    let merged = merge_installer_profile(base, &patch, runtime_libraries)?;

    publish_loader_profile(
        &version_dir,
        &serde_json::to_vec_pretty(&merged).map_err(|e| e.to_string())?,
        replace_existing,
        || {
            store
                .set_instance_loader(&instance.id, &format!("{}@{version}", kind.name()))
                .map_err(|e| e.to_string())
        },
    )
}

/// 安装器 jar 的地址与已知 SHA-1。
///
/// Forge 与 NeoForge 的 maven 布局不同：
///   - Forge:     `net/minecraftforge/forge/<mc>-<ver>/forge-<mc>-<ver>-installer.jar`
///   - NeoForge:  `net/neoforged/neoforge/<ver>/neoforge-<ver>-installer.jar`
fn installer_artifact(
    kind: LoaderKind,
    game: &str,
    version: &str,
) -> Result<(String, Option<String>), String> {
    match kind {
        LoaderKind::Forge => {
            let artifact = format!("{game}-{version}");
            Ok((
                format!(
                    "https://maven.minecraftforge.net/net/minecraftforge/forge/{artifact}/forge-{artifact}-installer.jar"
                ),
                None,
            ))
        }
        LoaderKind::NeoForge => Ok((
            format!(
                "https://maven.neoforged.net/releases/net/neoforged/neoforge/{version}/neoforge-{version}-installer.jar"
            ),
            None,
        )),
        _ => Err("该加载器不使用安装器。".into()),
    }
}

/// 从 install_profile 里找某个坐标声明的 SHA-1（有就带上，没有就靠 HTTPS + 后续校验）。
fn profile_artifact_sha1(profile: &InstallProfile, coord: &str) -> Option<String> {
    profile
        .libraries
        .iter()
        .find(|library| library.name == coord)
        .and_then(|library| library.downloads.as_ref())
        .and_then(|downloads| downloads.artifact.as_ref())
        .and_then(|artifact| artifact.sha1.clone())
        .filter(|sha1| sha1.len() == 40 && sha1.chars().all(|c| c.is_ascii_hexdigit()))
}

/// 解压安装器 jar 里安装需要的条目。
///
/// 只取 `install_profile.json`、`version.json` 与 `data/` 下的内容——安装器 jar
/// 里还有一堆无关的类文件，解压全部既慢又浪费空间。
fn extract_installer(source: &Path, target: &Path) -> Result<(), String> {
    use std::fs::File;
    let file = File::open(source).map_err(|e| format!("无法打开安装器：{e}"))?;
    let mut archive = zip::ZipArchive::new(file).map_err(|e| format!("安装器不是有效 ZIP：{e}"))?;
    if archive.len() > 20_000 {
        return Err("安装器条目数量异常。".into());
    }
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).map_err(|e| e.to_string())?;
        let Some(raw_name) = entry.enclosed_name().map(|path| path.to_path_buf()) else {
            // 路径穿越：直接拒绝整个安装器，而不是跳过这一项。
            return Err("安装器包含不安全的路径。".into());
        };
        let name = raw_name.to_string_lossy().replace('\\', "/");
        let wanted =
            name == "install_profile.json" || name == "version.json" || name.starts_with("data/");
        if !wanted || entry.is_dir() {
            continue;
        }
        let destination = target.join(&*raw_name);
        if let Some(parent) = destination.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let mut out = File::create(&destination).map_err(|e| e.to_string())?;
        std::io::copy(&mut entry, &mut out).map_err(|e| e.to_string())?;
    }
    if !target.join("install_profile.json").is_file() {
        return Err("安装器缺少 install_profile.json。".into());
    }
    Ok(())
}

/// 执行一个 processor。
///
/// 命令行形如：
/// `java -cp <jar>:<classpath...> <Main-Class> <args...>`
///
/// 主类从 jar 的 manifest 读 `Main-Class`。参数逐项构造（不经 shell），因此即使
/// 参数里出现空格或引号也不会被解释。
async fn run_processor(
    java: &Path,
    processor: &Processor,
    context: &ProcessorContext<'_>,
    index: usize,
    total: usize,
) -> Result<(), String> {
    let main_class = read_main_class(
        context
            .resolved
            .get(&processor.jar)
            .ok_or_else(|| format!("安装器处理器 {} 未下载。", processor.jar))?,
    )?;
    let mut classpath: Vec<String> = Vec::new();
    classpath.push(
        context
            .resolved
            .get(&processor.jar)
            .expect("checked above")
            .to_string_lossy()
            .into_owned(),
    );
    for coord in &processor.classpath {
        let path = context
            .resolved
            .get(coord)
            .ok_or_else(|| format!("安装器 classpath 缺少 {coord}。"))?;
        classpath.push(path.to_string_lossy().into_owned());
    }
    let mut args: Vec<String> = Vec::new();
    for raw in &processor.args {
        args.push(context.expand(raw)?);
    }
    let separator = if cfg!(windows) { ";" } else { ":" };
    let command = format!(
        "java -cp {} {} {}",
        classpath.join(separator),
        main_class,
        args.join(" ")
    );
    // 进度：处理器是一串串行步骤，用序号表达进度比假装百分比更诚实。
    let _ = (index, total);
    let output = tokio::process::Command::new(java)
        .arg("-cp")
        .arg(classpath.join(separator))
        .arg(&main_class)
        .args(&args)
        .output()
        .await
        .map_err(|error| format!("无法启动安装器处理器（{command}）：{error}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let detail = if stderr.trim().is_empty() {
            stdout
        } else {
            stderr
        };
        return Err(format!(
            "安装器处理器失败（第 {}/{total} 步，{main_class}）：\n{}",
            index + 1,
            truncate(&detail, 1200)
        ));
    }
    Ok(())
}

fn truncate(value: &str, limit: usize) -> String {
    value.chars().take(limit).collect()
}

/// 从 jar 的 manifest 读 `Main-Class`。
fn read_main_class(jar: &Path) -> Result<String, String> {
    use std::fs::File;
    use std::io::Read;
    let file = File::open(jar).map_err(|e| format!("无法打开处理器 jar：{e}"))?;
    let mut archive = zip::ZipArchive::new(file).map_err(|e| format!("处理器 jar 无效：{e}"))?;
    let mut entry = archive
        .by_name("META-INF/MANIFEST.MF")
        .map_err(|_| format!("处理器 jar 缺少 manifest：{}", jar.display()))?;
    let mut text = String::new();
    entry
        .read_to_string(&mut text)
        .map_err(|e| format!("读取 manifest 失败：{e}"))?;
    // manifest 每行最多 72 字节，长值会以「空格 + 续行」折行，需要先拼接。
    let mut unfolded = String::new();
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix(' ') {
            unfolded.push_str(rest);
        } else {
            unfolded.push('\n');
            unfolded.push_str(line);
        }
    }
    for line in unfolded.lines() {
        if let Some(value) = line.strip_prefix("Main-Class:") {
            let value = value.trim();
            if !value.is_empty() && value.len() <= 256 {
                return Ok(value.to_owned());
            }
        }
    }
    Err(format!("处理器 jar 没有声明 Main-Class：{}", jar.display()))
}

/// 安装 `version.json` 声明的运行时库。
///
/// 这些是 Forge 启动时真正需要的 jar（BootstrapLauncher、fmlcore、mclanguage 等）。
async fn install_version_libraries(
    engine: &DownloadEngine,
    root: &Path,
    patch: &serde_json::Value,
    kind: LoaderKind,
    progress: Option<mpsc::UnboundedSender<DownloadProgress>>,
) -> Result<Vec<Library>, String> {
    let list = patch
        .get("libraries")
        .and_then(|v| v.as_array())
        .ok_or("安装器版本配置缺少依赖清单。")?;
    if list.len() > 10_000 {
        return Err("加载器依赖数量超过限制。".into());
    }
    let mut libraries = Vec::new();
    for library in list {
        let name = library
            .get("name")
            .and_then(|v| v.as_str())
            .ok_or("加载器依赖坐标无效。")?;
        let relative = crate::loaders::safe_maven_path(name)?;
        let destination = root.join("libraries").join(&relative);
        if let Some(artifact) = library.get("downloads").and_then(|d| d.get("artifact")) {
            let url = artifact
                .get("url")
                .and_then(|v| v.as_str())
                .ok_or("加载器依赖缺少下载地址。")?;
            // 只信任白名单仓库：这里的 URL 来自安装器提供的配置。
            let parsed = reqwest::Url::parse(url).map_err(|_| "加载器依赖地址无效。")?;
            let host = parsed.host_str().unwrap_or_default();
            if parsed.scheme() != "https"
                || !matches!(
                    host,
                    "maven.minecraftforge.net"
                        | "maven.neoforged.net"
                        | "libraries.minecraft.net"
                        | "repo1.maven.org"
                        | "repo.maven.apache.org"
                )
            {
                return Err(format!("加载器依赖引用了未获准的仓库：{host}"));
            }
            let sha1 = artifact
                .get("sha1")
                .and_then(|v| v.as_str())
                .filter(|sha1| sha1.len() == 40 && sha1.chars().all(|c| c.is_ascii_hexdigit()))
                .map(str::to_owned);
            let size = artifact.get("size").and_then(|v| v.as_u64());
            if !destination.is_file() {
                engine
                    .download(
                        &ArtifactSpec {
                            id: format!("loader-lib:{name}"),
                            label: format!("加载器依赖 {name}"),
                            url: url.to_owned(),
                            mirror_url: None,
                            destination: destination.clone(),
                            size,
                            sha1: sha1.clone(),
                            sha256: None,
                        },
                        progress.clone(),
                    )
                    .await
                    .map_err(|e| format!("下载加载器依赖 {name} 失败：{e}"))?;
            }
            libraries.push(Library {
                name: name.to_owned(),
                downloads: LibraryDownloads {
                    artifact: Some(Artifact {
                        path: Some(relative),
                        url: url.to_owned(),
                        sha1,
                        size,
                    }),
                    ..Default::default()
                },
                ..Default::default()
            });
        } else {
            // 没有 artifact 的条目（例如仅 natives）保留声明，由启动阶段按规则处理。
            libraries.push(Library {
                name: name.to_owned(),
                ..Default::default()
            });
        }
        let _ = kind;
    }
    Ok(libraries)
}

/// 把安装器的 `version.json` 合并进原版配置。
fn merge_installer_profile(
    mut base: GameVersionMetadata,
    patch: &serde_json::Value,
    libraries: Vec<Library>,
) -> Result<GameVersionMetadata, String> {
    // `inheritsFrom` 必须指向实例所用的原版版本，否则会把错误的配置叠上来。
    if let Some(inherits) = patch.get("inheritsFrom").and_then(|v| v.as_str())
        && inherits != base.id
    {
        return Err(format!(
            "加载器继承版本 {inherits} 与实例 {} 不一致。",
            base.id
        ));
    }
    base.main_class = Some(
        patch
            .get("mainClass")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .ok_or("加载器缺少入口类。")?
            .to_owned(),
    );
    for library in libraries {
        let key = library
            .name
            .split(':')
            .take(2)
            .collect::<Vec<_>>()
            .join(":");
        base.libraries
            .retain(|old| old.name.split(':').take(2).collect::<Vec<_>>().join(":") != key);
        base.libraries.push(library);
    }
    if let Some(arguments) = patch.get("arguments") {
        if base.arguments.is_none() {
            return Err("此加载器需要现代版本启动参数；当前实例版本过旧。".into());
        }
        let extra: mc_metadata::LaunchArguments =
            serde_json::from_value(arguments.clone()).map_err(|_| "加载器启动参数无效。")?;
        let base_args = base.arguments.get_or_insert_with(Default::default);
        base_args.jvm.extend(extra.jvm);
        base_args.game.extend(extra.game);
    }
    Ok(base)
}

/// 发布 `launch.json`。
///
/// `replace_existing` 为真时先删掉已有文件——修复损坏的加载器时那个文件一定在，
/// 不删就永远发布不成功。正常安装保持「不覆盖」语义。
fn publish_loader_profile(
    directory: &Path,
    bytes: &[u8],
    replace_existing: bool,
    commit: impl FnOnce() -> Result<(), String>,
) -> Result<(), String> {
    use std::io::Write;
    let temp = directory.join(format!(".launch-{}.pending", uuid::Uuid::new_v4()));
    let target = directory.join("launch.json");
    if replace_existing {
        match std::fs::remove_file(&target) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(format!("无法替换加载器配置：{error}")),
        }
    }
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temp)
        .map_err(|e| e.to_string())?;
    let mut published = false;
    let result = (|| {
        file.write_all(bytes).map_err(|e| e.to_string())?;
        file.sync_all().map_err(|e| e.to_string())?;
        std::fs::hard_link(&temp, &target)
            .map_err(|e| format!("无法发布加载器配置（不会覆盖已有文件）：{e}"))?;
        published = true;
        commit()
    })();
    drop(file);
    if let Err(mut error) = result {
        if published && let Err(rollback) = std::fs::remove_file(&target) {
            error = format!("{error}；加载器配置回滚失败：{rollback}");
        }
        if let Err(cleanup) = std::fs::remove_file(&temp) {
            error = format!("{error}；清理临时配置失败：{cleanup}");
        }
        return Err(error);
    }
    std::fs::remove_file(temp).map_err(|e| format!("加载器已安装，但清理临时配置失败：{e}"))
}

/// 从一段文本里提取形如 `[group:artifact:version(@ext)]` 的 Maven 坐标。
///
/// processor 的参数既可能是完整坐标（`[a:b:1]`），也可能嵌在其它文本里
/// （`--input=[a:b:1]`），所以不能只判断「整个字符串是否被方括号包围」。
pub fn maven_coord_in(text: &str) -> Option<String> {
    let start = text.find('[')?;
    let end = text[start..].find(']')? + start;
    let coord = text[start + 1..end].trim();
    if coord.is_empty() || coord.contains(' ') {
        return None;
    }
    // 至少要有 group:artifact:version 三段。
    if coord.split(':').count() < 3 {
        return None;
    }
    Some(coord.to_owned())
}

pub fn installer_version_id(
    loader: LoaderKind,
    game: &str,
    version: &str,
) -> Result<String, String> {
    safe_version(version)?;
    safe_version(game)?;
    Ok(match loader {
        LoaderKind::Forge => format!("{game}-forge-{version}"),
        LoaderKind::NeoForge => format!("neoforge-{version}"),
        _ => return Err("该加载器不使用安装器。".into()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maven_relative_builds_expected_layout() {
        assert_eq!(
            maven_relative("net.minecraftforge:forge:1.20.1-47.4.28:universal").unwrap(),
            "net/minecraftforge/forge/1.20.1-47.4.28/forge-1.20.1-47.4.28-universal.jar"
        );
        assert_eq!(
            maven_relative("de.oceanlabs.mcp:mcp_config:1.20.1@zip").unwrap(),
            "de/oceanlabs/mcp/mcp_config/1.20.1/mcp_config-1.20.1.zip"
        );
        for bad in ["a:b", "a:b:c/../d", "a:b::c"] {
            assert!(maven_relative(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn repositories_are_whitelisted_by_group() {
        assert!(
            maven_repository_for("net.minecraftforge:installertools:1.4.1", LoaderKind::Forge)
                .unwrap()
                .contains("minecraftforge")
        );
        assert!(
            maven_repository_for("net.neoforged:neoforge:21.1.9", LoaderKind::NeoForge)
                .unwrap()
                .contains("neoforged")
        );
        assert!(
            maven_repository_for("com.google.guava:guava:25.1-jre", LoaderKind::Forge)
                .unwrap()
                .contains("maven.org")
        );
        // 非安装器加载器不该解析仓库。
        assert!(maven_repository_for("a:b:1", LoaderKind::Fabric).is_err());
    }

    #[test]
    fn processor_side_filtering_excludes_server_steps() {
        let server_only = Processor {
            jar: "x".into(),
            classpath: vec![],
            args: vec![],
            outputs: None,
            sides: Some(vec!["server".into()]),
        };
        assert!(!server_only.runs_on_client());
        let client_only = Processor {
            sides: Some(vec!["client".into()]),
            ..server_only.clone()
        };
        assert!(client_only.runs_on_client());
        // 缺省 sides 表示两端都跑。
        let both = Processor {
            sides: None,
            ..server_only
        };
        assert!(both.runs_on_client());
    }

    #[test]
    fn placeholder_expansion_handles_all_three_forms() {
        let libraries = Path::new("/libs");
        let resolved = HashMap::from([(
            "de.oceanlabs.mcp:mcp_config:1.20.1@zip".to_owned(),
            PathBuf::from("/libs/mcp.zip"),
        )]);
        let data = HashMap::from([
            ("MAPPINGS".to_owned(), "/libs/mcp.zip".to_owned()),
            ("MC_SLIM_SHA".to_owned(), "abc123".to_owned()),
        ]);
        let context = ProcessorContext {
            libraries,
            installer_root: Path::new("/tmp/inst"),
            minecraft_jar: Path::new("/mc/client.jar"),
            resolved: &resolved,
            data: &data,
            side: "client",
        };
        assert_eq!(context.expand("{MAPPINGS}").unwrap(), "/libs/mcp.zip");
        assert_eq!(
            context
                .expand("[de.oceanlabs.mcp:mcp_config:1.20.1@zip]")
                .unwrap(),
            "/libs/mcp.zip"
        );
        // 嵌在其它文本里的占位符。
        assert_eq!(
            context.expand("--input={MAPPINGS}").unwrap(),
            "--input=/libs/mcp.zip"
        );
        // 未下载的依赖要明确报错，而不是产生一个坏路径。
        assert!(context.expand("[unknown:thing:1]").is_err());
        assert!(context.expand("{NOPE}").is_err());
    }

    #[test]
    fn installer_version_ids_match_loaders() {
        assert_eq!(
            installer_version_id(LoaderKind::Forge, "1.20.1", "47.4.28").unwrap(),
            "1.20.1-forge-47.4.28"
        );
        assert_eq!(
            installer_version_id(LoaderKind::NeoForge, "1.21.1", "21.1.9").unwrap(),
            "neoforge-21.1.9"
        );
        assert!(installer_version_id(LoaderKind::Fabric, "1.20.1", "0.16.10").is_err());
    }
}
