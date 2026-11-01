//! 加载器适配器：Fabric / Quilt 走 profile 合并，Forge / NeoForge 走 installer processor。
//!
//! ## 为什么 Forge 需要另一条完全不同的路径
//!
//! Fabric 与 Quilt 发布的是「profile json」——可以直接和原版版本配置合并，得到一个
//! 新的 `launch.json`。不需要执行任何外部程序。
//!
//! Forge / NeoForge 发布的是「installer jar」，里面有一份 `install_profile.json`
//! 列出若干 **processor**（`installertools`、`jarsplitter`、`ForgeAutoRenamingTool`、
//! `binarypatcher` 等）。安装必须依次运行这些 Java 程序：解压 MCP 映射、下载
//! Mojang 官方映射、合并映射、切分客户端 jar、按映射重命名、应用二进制补丁。
//! 走完这些步骤才会产出 Forge 需要的那个打过补丁的客户端 jar。
//!
//! 因此这里有两套实现，共享下载引擎、库路径解析与原子发布，但执行方式不同。
use crate::{InstanceRecord, InstanceState, LauncherStore, installer::maven_path};
use download_engine::{ArtifactSpec, DownloadEngine, DownloadProgress};
use mc_metadata::{Artifact, GameVersionMetadata, Library, LibraryDownloads};
use serde::{Deserialize, Serialize};
use std::{
    path::{Path, PathBuf},
    time::Duration,
};
use tokio::sync::mpsc;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum LoaderKind {
    Fabric,
    Quilt,
    Forge,
    NeoForge,
}

impl LoaderKind {
    pub fn name(self) -> &'static str {
        match self {
            Self::Fabric => "fabric",
            Self::Quilt => "quilt",
            Self::Forge => "forge",
            Self::NeoForge => "neoforge",
        }
    }
    fn meta(self) -> &'static str {
        match self {
            Self::Fabric => "https://meta.fabricmc.net/v2/",
            Self::Quilt => "https://meta.quiltmc.org/v3/",
            // Forge 与 NeoForge 没有 meta 服务，版本列表与配置都来自 maven 的
            // promotions / 安装器 jar，见各自的 `list_*_versions` 实现。
            Self::Forge => "https://maven.minecraftforge.net/net/minecraftforge/forge/",
            Self::NeoForge => "https://maven.neoforged.net/releases/net/neoforged/neoforge/",
        }
    }
    /// 这一族加载器是否走 installer processor 安装。
    fn uses_installer(self) -> bool {
        matches!(self, Self::Forge | Self::NeoForge)
    }

    /// 从实例记录里的 `loader` 字段前缀还原出类型（`fabric` / `quilt` / …）。
    ///
    /// 实例的 `loader` 字段形如 `fabric@0.16.10`，前半段就是这里的名字。用枚举
    /// 反查而不是直接字符串比较，是为了让「有哪些加载器」这件事只有一个定义处。
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "fabric" => Some(Self::Fabric),
            "quilt" => Some(Self::Quilt),
            "forge" => Some(Self::Forge),
            "neoforge" => Some(Self::NeoForge),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LoaderVersion {
    pub version: String,
    pub stable: bool,
}

pub fn safe_version(value: &str) -> Result<(), String> {
    if value.is_empty()
        || value.len() > 128
        || !value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || ".-+_".contains(c))
        || value.contains("..")
    {
        return Err("版本标识包含不允许的字符。".into());
    }
    crate::modpacks::safe_pack_path(value)?;
    Ok(())
}

pub async fn list_loader_versions(
    kind: LoaderKind,
    game: &str,
) -> Result<Vec<LoaderVersion>, String> {
    safe_version(game)?;
    if kind.uses_installer() {
        return list_installer_versions(kind, game).await;
    }
    let client = metadata_client()?;
    let response = client
        .get(format!("{}versions/loader/{game}", kind.meta()))
        .send()
        .await
        .map_err(|_| "无法连接加载器版本服务。")?
        .error_for_status()
        .map_err(|_| "该游戏版本没有可用加载器。")?;
    let bytes = bounded_response(response, 4 * 1024 * 1024).await?;
    let values: Vec<serde_json::Value> =
        serde_json::from_slice(&bytes).map_err(|_| "加载器版本响应无效。")?;
    Ok(values
        .into_iter()
        .filter_map(|value| {
            let loader = value.get("loader")?;
            let version = loader.get("version")?.as_str()?.to_owned();
            if safe_version(&version).is_err() {
                return None;
            }
            Some(LoaderVersion {
                version,
                stable: loader
                    .get("stable")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false),
            })
        })
        .collect())
}

/// Forge / NeoForge 的版本列表。
///
/// Forge 用 `promotions_slim.json`：一个 `{ "<mc>-latest": "<forge ver>", ... }` 的
/// 映射。这正是官方推荐的「该 MC 版本该用哪个 Forge」来源，按 MC 版本过滤即可。
///
/// NeoForge 用 maven 的 `maven-metadata.xml`。它的版本号规则是
/// `<mc minor>.<mc patch>.<neoforge build>`（例如 1.21.1 → `21.1.x`），与 Forge
/// 的 `<mc>-<forge>` 完全不同，必须单独换算。
async fn list_installer_versions(
    kind: LoaderKind,
    game: &str,
) -> Result<Vec<LoaderVersion>, String> {
    match kind {
        LoaderKind::Forge => forge_versions(game).await,
        LoaderKind::NeoForge => neoforge_versions(game).await,
        _ => Err("该加载器不使用安装器。".into()),
    }
}

async fn forge_versions(game: &str) -> Result<Vec<LoaderVersion>, String> {
    let client = metadata_client()?;
    let response = client
        .get("https://files.minecraftforge.net/net/minecraftforge/forge/promotions_slim.json")
        .send()
        .await
        .map_err(|_| "无法连接 Forge 版本服务。")?
        .error_for_status()
        .map_err(|_| "Forge 版本服务拒绝了请求。")?;
    let bytes = bounded_response(response, 4 * 1024 * 1024).await?;
    let value: serde_json::Value =
        serde_json::from_slice(&bytes).map_err(|_| "Forge 版本列表格式无效。")?;
    let promos = value
        .get("promos")
        .and_then(|v| v.as_object())
        .ok_or("Forge 版本列表缺少 promos 字段。")?;
    let mut out: Vec<LoaderVersion> = Vec::new();
    for (key, version) in promos {
        // 键形如 `<mc>-latest` / `<mc>-recommended`。
        let Some((mc, channel)) = key.rsplit_once('-') else {
            continue;
        };
        if mc != game {
            continue;
        }
        let Some(version) = version.as_str() else {
            continue;
        };
        if safe_version(version).is_err() {
            continue;
        }
        // recommended 视为稳定版；同一版本可能既是 latest 又是 recommended。
        let stable = channel == "recommended";
        match out.iter_mut().find(|item| item.version == version) {
            Some(existing) => existing.stable |= stable,
            None => out.push(LoaderVersion {
                version: version.to_owned(),
                stable,
            }),
        }
    }
    // 稳定版排前面，同组内按版本号倒序（新版本优先）。
    out.sort_by(|a, b| {
        b.stable
            .cmp(&a.stable)
            .then_with(|| compare_versions(&b.version, &a.version))
    });
    if out.is_empty() {
        return Err(format!("Minecraft {game} 没有可用的 Forge 版本。"));
    }
    Ok(out)
}

async fn neoforge_versions(game: &str) -> Result<Vec<LoaderVersion>, String> {
    /*
     * NeoForge 的版本号不是 `<mc>-<loader>`，而是 `<mc 次版本>.<mc 补丁版>.<构建号>`：
     *   1.21.1 → 21.1.x
     *   1.20.4 → 20.4.x
     *   1.21   → 21.0.x
     * 1.20.1 及更早没有 NeoForge（当时是 Forge 的 fork）。
     */
    let parts: Vec<&str> = game.split('.').collect();
    if parts.len() < 2 || parts[0] != "1" {
        return Err(format!("Minecraft {game} 不支持 NeoForge。"));
    }
    let minor = parts[1];
    let patch = parts.get(2).copied().unwrap_or("0");
    // 1.20.1 是 Forge 时代，NeoForge 从 1.20.2 才开始。
    if minor == "20" && patch == "1" {
        return Err("Minecraft 1.20.1 没有 NeoForge；请改用 Forge。".into());
    }
    let prefix = format!("{minor}.{patch}.");

    let client = metadata_client()?;
    let response = client
        .get("https://maven.neoforged.net/releases/net/neoforged/neoforge/maven-metadata.xml")
        .send()
        .await
        .map_err(|_| "无法连接 NeoForge 版本服务。")?
        .error_for_status()
        .map_err(|_| "NeoForge 版本服务拒绝了请求。")?;
    let bytes = bounded_response(response, 8 * 1024 * 1024).await?;
    let text = std::str::from_utf8(&bytes).map_err(|_| "NeoForge 版本列表不是有效文本。")?;
    let mut versions: Vec<String> = Vec::new();
    // maven-metadata.xml 很小且结构固定，抓 <version> 标签即可，不必引入 XML 解析器。
    let mut rest = text;
    while let Some(start) = rest.find("<version>") {
        let after = &rest[start + "<version>".len()..];
        let Some(end) = after.find("</version>") else {
            break;
        };
        let value = after[..end].trim();
        if value.starts_with(&prefix) && safe_version(value).is_ok() {
            versions.push(value.to_owned());
        }
        rest = &after[end + "</version>".len()..];
    }
    if versions.is_empty() {
        return Err(format!("Minecraft {game} 没有可用的 NeoForge 版本。"));
    }
    versions.sort_by(|a, b| compare_versions(b, a));
    // NeoForge 不区分 latest/recommended，带 `-beta` 的视为测试版。
    Ok(versions
        .into_iter()
        .map(|version| LoaderVersion {
            stable: !version.contains("beta"),
            version,
        })
        .collect())
}

/// 按 `.` 分段做数值比较；段数不同时短的一方补 0。
///
/// 直接字符串比较会把 `0.16.9` 排在 `0.16.10` 之后（"9" > "1"），这是加载器版本
/// 列表里最容易看出来的排序错误。
fn compare_versions(a: &str, b: &str) -> std::cmp::Ordering {
    let split = |value: &str| -> Vec<u64> {
        value
            .split(['.', '-', '+'])
            .map(|part| part.parse::<u64>().unwrap_or(0))
            .collect()
    };
    let (left, right) = (split(a), split(b));
    let len = left.len().max(right.len());
    for index in 0..len {
        let l = left.get(index).copied().unwrap_or(0);
        let r = right.get(index).copied().unwrap_or(0);
        match l.cmp(&r) {
            std::cmp::Ordering::Equal => {}
            other => return other,
        }
    }
    std::cmp::Ordering::Equal
}

fn metadata_client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .user_agent("MFMCL/0.1.0")
        .https_only(true)
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|_| "无法初始化 HTTPS 客户端。".into())
}

pub async fn install_loader(
    root: &Path,
    instance: &InstanceRecord,
    store: &LauncherStore,
    engine: &DownloadEngine,
    kind: LoaderKind,
    version: &str,
    progress: Option<mpsc::UnboundedSender<DownloadProgress>>,
) -> Result<(), String> {
    install_loader_inner(
        root, instance, store, engine, kind, version, progress, false,
    )
    .await
}

/// 重装一个已记录的加载器（用于补全文件时修复损坏的加载器）。
///
/// 与 [`install_loader`] 的唯一差别是允许实例已经有同一个加载器：正常安装时那句
/// 「不会覆盖已有配置」是为了防止用户误操作把实例换掉，而修复场景下正是在修同
/// 一个加载器，拦下来就永远修不好。
///
/// 仍然会拒绝「实例已装 A、却要装 B」的替换：那不是修复，是换加载器，需要用户
/// 主动决定（目前产品上没有这条路径）。
pub async fn reinstall_loader(
    root: &Path,
    instance: &InstanceRecord,
    store: &LauncherStore,
    engine: &DownloadEngine,
    kind: LoaderKind,
    version: &str,
    progress: Option<mpsc::UnboundedSender<DownloadProgress>>,
) -> Result<(), String> {
    install_loader_inner(root, instance, store, engine, kind, version, progress, true).await
}

#[allow(clippy::too_many_arguments)]
async fn install_loader_inner(
    root: &Path,
    instance: &InstanceRecord,
    store: &LauncherStore,
    engine: &DownloadEngine,
    kind: LoaderKind,
    version: &str,
    progress: Option<mpsc::UnboundedSender<DownloadProgress>>,
    allow_same_loader: bool,
) -> Result<(), String> {
    safe_version(version)?;
    safe_version(&instance.game_version)?;
    if instance.state != InstanceState::Ready {
        return Err("请先完成原版安装。".into());
    }
    if instance.loader != "vanilla" {
        if !allow_same_loader {
            return Err("实例已安装加载器；不会覆盖已有配置。".into());
        }
        // 修复场景：只允许重装「实例记录里的那一个」。
        if instance.loader != format!("{}@{version}", kind.name()) {
            return Err(format!(
                "实例已安装 {}，不能用 {}@{version} 覆盖。",
                instance.loader,
                kind.name()
            ));
        }
    }
    if kind.uses_installer() {
        return crate::forge_install::install_with_processor(
            root,
            instance,
            store,
            engine,
            kind,
            version,
            progress,
            // 修复场景（已有同一个加载器）需要能替换 launch.json。
            allow_same_loader && instance.loader != "vanilla",
        )
        .await;
    }
    let client = metadata_client()?;
    let response = client
        .get(format!(
            "{}versions/loader/{}/{version}/profile/json",
            kind.meta(),
            instance.game_version
        ))
        .send()
        .await
        .map_err(|_| "无法获取加载器配置。")?
        .error_for_status()
        .map_err(|_| "此加载器与游戏版本组合不可用。")?;
    let bytes = bounded_response(response, 4 * 1024 * 1024).await?;
    let patch: serde_json::Value =
        serde_json::from_slice(&bytes).map_err(|_| "加载器配置格式无效。")?;
    if patch.get("processors").is_some() {
        return Err("加载器配置包含不支持的处理器；不会执行任意程序。".into());
    }
    let version_dir = instance
        .instance_dir
        .join("versions")
        .join(&instance.game_version);
    let version_dir_meta = tokio::fs::symlink_metadata(&version_dir)
        .await
        .map_err(|e| e.to_string())?;
    if version_dir_meta.file_type().is_symlink() || !version_dir_meta.file_type().is_dir() {
        return Err("游戏版本目录不能是符号链接或普通文件。".into());
    }
    let base_path = version_dir.join(format!("{}.json", instance.game_version));
    let base_meta = tokio::fs::symlink_metadata(&base_path)
        .await
        .map_err(|e| e.to_string())?;
    if base_meta.file_type().is_symlink()
        || !base_meta.file_type().is_file()
        || base_meta.len() > 16 * 1024 * 1024
    {
        return Err("原版游戏配置不是有效的普通文件。".into());
    }
    let base: GameVersionMetadata = serde_json::from_slice(
        &tokio::fs::read(base_path)
            .await
            .map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    let library_list = patch
        .get("libraries")
        .and_then(|v| v.as_array())
        .ok_or("加载器缺少依赖清单。")?;
    if library_list.len() > 10_000 {
        return Err("加载器依赖数量超过限制。".into());
    }
    let mut libraries = Vec::new();
    let mut coordinates = std::collections::HashSet::new();
    for library in library_list {
        let name = library
            .get("name")
            .and_then(|v| v.as_str())
            .ok_or("加载器依赖坐标无效。")?;
        let relative = safe_maven_path(name)?;
        if !coordinates.insert(name.to_owned()) {
            return Err("加载器依赖清单包含重复 Maven 坐标。".into());
        }
        let repository = library
            .get("url")
            .and_then(|v| v.as_str())
            .unwrap_or("https://libraries.minecraft.net/");
        let mut repository = reqwest::Url::parse(repository).map_err(|_| "依赖仓库 URL 无效。")?;
        if repository.scheme() != "https"
            || repository.query().is_some()
            || repository.fragment().is_some()
            || !matches!(
                repository.host_str(),
                Some(
                    "maven.fabricmc.net"
                        | "maven.quiltmc.org"
                        | "libraries.minecraft.net"
                        | "repo1.maven.org"
                        | "repo.maven.apache.org"
                )
            )
            || !repository.username().is_empty()
            || repository.password().is_some()
        {
            return Err("加载器配置引用了未获准的依赖仓库。".into());
        }
        if !repository.path().ends_with('/') {
            repository.set_path(&format!("{}/", repository.path()));
        }
        let url = repository
            .join(&relative)
            .map_err(|e| e.to_string())?
            .to_string();
        let sha1 = if let Some(value) = library.get("sha1") {
            value
                .as_str()
                .ok_or("加载器 SHA-1 校验值无效。")?
                .to_owned()
        } else {
            let response = client
                .get(format!("{url}.sha1"))
                .send()
                .await
                .map_err(|_| "无法取得加载器依赖校验值。")?
                .error_for_status()
                .map_err(|_| "加载器依赖没有可用 SHA-1 校验值。")?;
            let bytes = bounded_response(response, 4096).await?;
            let text = std::str::from_utf8(&bytes).map_err(|_| "校验值不是有效文本。")?;
            text.split_whitespace().next().unwrap_or("").to_owned()
        };
        if sha1.len() != 40 || !sha1.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err("加载器 SHA-1 校验值无效。".into());
        }
        let sha256 = match library.get("sha256") {
            Some(value) => {
                let hash = value.as_str().ok_or("加载器 SHA-256 校验值无效。")?;
                if hash.len() != 64 || !hash.chars().all(|c| c.is_ascii_hexdigit()) {
                    return Err("加载器 SHA-256 校验值无效。".into());
                }
                Some(hash.to_owned())
            }
            None => None,
        };
        let size = match library.get("size") {
            Some(value) => Some(
                value
                    .as_u64()
                    .filter(|size| *size <= 512 * 1024 * 1024)
                    .ok_or("加载器依赖大小无效。")?,
            ),
            None => None,
        };
        let artifact = Artifact {
            path: Some(relative.clone()),
            url: url.clone(),
            sha1: Some(sha1.clone()),
            size,
        };
        engine
            .download(
                &ArtifactSpec {
                    id: format!("loader:{name}"),
                    label: format!("加载器依赖 {name}"),
                    url,
                    mirror_url: None,
                    destination: root.join("libraries").join(&relative),
                    size,
                    sha1: Some(sha1),
                    sha256,
                },
                progress.clone(),
            )
            .await
            .map_err(|e| e.to_string())?;
        libraries.push(Library {
            name: name.to_owned(),
            downloads: LibraryDownloads {
                artifact: Some(artifact),
                ..Default::default()
            },
            ..Default::default()
        });
    }
    let merged = merge_loader_profile(base, &patch, libraries)?;
    /*
     * 发布 `launch.json`。
     *
     * 正常安装用「不覆盖」语义（`hard_link` 在目标存在时失败）；重装场景下那个
     * 文件一定已经存在，必须先删掉再发布，否则永远修不好。
     *
     * 先删再写有一个短暂窗口：若此时进程被杀，会留下「没有 launch.json」的状态。
     * 这比反过来（先写后删）安全——前者只是加载器配置丢了，后者可能把两个版本的
     * 配置混在一起。而且删掉之后紧接着的 hard_link 是原子的，窗口只有微秒级。
     */
    let replacing = allow_same_loader && instance.loader != "vanilla";
    publish_profile(
        &version_dir,
        &serde_json::to_vec_pretty(&merged).map_err(|e| e.to_string())?,
        replacing,
        || {
            store
                .set_instance_loader(&instance.id, &format!("{}@{version}", kind.name()))
                .map_err(|e| e.to_string())
        },
    )
}

fn publish_profile(
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
    if let Err(error) = result {
        let mut error = error;
        if published && let Err(rollback) = std::fs::remove_file(&target) {
            error = format!("{error}；加载器配置回滚失败：{rollback}");
        }
        if let Err(cleanup) = std::fs::remove_file(&temp) {
            error = format!("{error}；清理临时配置失败：{cleanup}");
        }
        return Err(error);
    }
    // Once committed, cleanup can be retried without reinstalling the loader.
    std::fs::remove_file(&temp).map_err(|e| format!("加载器已安装，但清理临时配置失败：{e}"))
}

async fn bounded_response(
    mut response: reqwest::Response,
    limit: usize,
) -> Result<Vec<u8>, String> {
    if response
        .content_length()
        .is_some_and(|length| length > limit as u64)
    {
        return Err("加载器响应过大。".into());
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| "加载器响应读取失败。")? {
        if bytes
            .len()
            .checked_add(chunk.len())
            .is_none_or(|size| size > limit)
        {
            return Err("加载器响应过大。".into());
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

pub(crate) fn safe_maven_path(name: &str) -> Result<String, String> {
    if name.contains(['/', '\\'])
        || name.contains("..")
        || name.matches('@').count() > 1
        || name
            .chars()
            .any(|c| !c.is_ascii_alphanumeric() && !".:-_+@".contains(c))
    {
        return Err("不安全的 Maven 坐标。".into());
    }
    let (coordinate, extension) = name.split_once('@').unwrap_or((name, "jar"));
    let parts: Vec<_> = coordinate.split(':').collect();
    if !(3..=4).contains(&parts.len())
        || parts.iter().any(|part| part.is_empty())
        || extension.is_empty()
        || !extension.chars().all(|c| c.is_ascii_alphanumeric())
    {
        return Err("Maven 坐标无效。".into());
    }
    let path = maven_path(name)
        .and_then(|p| p.to_str().map(str::to_owned))
        .ok_or("Maven 坐标无效。")?;
    #[cfg(windows)]
    let path = path.replace('\\', "/");
    crate::modpacks::safe_pack_path(&path)?;
    Ok(path)
}

/// A pending marker can survive a successful install if its removal failed.
pub fn loader_overlay_is_installed(
    instance: &InstanceRecord,
    kind: LoaderKind,
    version: &str,
) -> Result<bool, String> {
    safe_version(version)?;
    safe_version(&instance.game_version)?;
    if instance.loader != format!("{}@{version}", kind.name()) {
        return Ok(false);
    }
    let path = instance
        .instance_dir
        .join("versions")
        .join(&instance.game_version)
        .join("launch.json");
    let metadata = match std::fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error.to_string()),
    };
    if !metadata.file_type().is_file() || metadata.len() > 16 * 1024 * 1024 {
        return Err("已记录的加载器配置不是有效的普通文件。".into());
    }
    let profile: GameVersionMetadata =
        serde_json::from_slice(&std::fs::read(path).map_err(|e| e.to_string())?)
            .map_err(|_| "已记录的加载器配置格式无效。")?;
    Ok(profile.id == instance.game_version
        && profile
            .main_class
            .as_deref()
            .is_some_and(|value| !value.is_empty()))
}

/// 加载器完好性诊断的结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoaderHealth {
    /// 实例本来就没有加载器，不需要修复。
    NotInstalled,
    /// 配置与库文件都在。
    Healthy,
    /// 需要重装：`reason` 用于给用户解释发生了什么。
    Broken(String),
}

/// 检查实例上记录的加载器是否仍然可用。
///
/// 补全官方文件只覆盖 Mojang 那一侧（客户端 jar、库、资源）。加载器的文件是另一
/// 套来源——Fabric 的 loader jar 在 `maven.fabricmc.net`，Forge 的处理器产物在
/// `maven.minecraftforge.net`。这些文件同样会被误删、被磁盘错误破坏，而
/// `install_vanilla` 完全不碰它们。结果是：官方文件补全成功，游戏却仍然起不来，
/// 报的还是一个和文件缺失毫无关系的「找不到主类」。
///
/// 这里做三件事：
///   1. `launch.json` 是否存在且能解析（加载器配置本身）；
///   2. 它声明的 `mainClass` 是否非空（起不来的最常见原因）；
///   3. 它引用的**加载器侧**库文件是否都在磁盘上。
///
/// 第 3 点只检查非官方来源的库：官方库由 `install_vanilla` 校验并补全，这里重复
/// 检查既慢又会误报——加载器配置里混着大量原版库（gson、lwjgl 等），它们的
/// 校验值来自 Mojang，不该由这一层判断。
pub fn diagnose_loader(instance: &InstanceRecord, libraries_dir: &Path) -> LoaderHealth {
    if instance.loader == "vanilla" {
        return LoaderHealth::NotInstalled;
    }
    let Some((kind, version)) = instance.loader.split_once('@') else {
        return LoaderHealth::Broken(format!("加载器记录格式无效：{}", instance.loader));
    };
    let Some(kind) = LoaderKind::from_name(kind) else {
        return LoaderHealth::Broken(format!("未识别的加载器：{kind}"));
    };
    let path = instance
        .instance_dir
        .join("versions")
        .join(&instance.game_version)
        .join("launch.json");
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return LoaderHealth::Broken("加载器配置 launch.json 不存在".to_owned());
        }
        Err(error) => return LoaderHealth::Broken(format!("无法读取加载器配置：{error}")),
    };
    let profile: GameVersionMetadata = match serde_json::from_slice(&bytes) {
        Ok(profile) => profile,
        Err(_) => return LoaderHealth::Broken("加载器配置已损坏".to_owned()),
    };
    if profile.id != instance.game_version {
        return LoaderHealth::Broken(format!(
            "加载器配置对应版本 {}，与实例 {} 不一致",
            profile.id, instance.game_version
        ));
    }
    if profile
        .main_class
        .as_deref()
        .is_none_or(|value| value.is_empty())
    {
        return LoaderHealth::Broken("加载器配置缺少入口类".to_owned());
    }
    /*
     * 只挑「加载器侧」的库来检查：group 不是 Mojang 官方的那批。
     *
     * 判定依据是 group 前缀——官方库的坐标以 `com.mojang`、`org.lwjgl`、
     * `net.java.jinput`、`net.java.jutils`、`ca.weblite`（oshi）、`com.google.code.gson`
     * 等为主，而加载器自己的产物一定是 `net.fabricmc` / `org.quiltmc` /
     * `net.minecraftforge` / `cpw.mods` / `net.neoforged` 之一。
     */
    let mut missing: Vec<String> = Vec::new();
    for library in &profile.libraries {
        if !is_loader_side_library(&library.name) {
            continue;
        }
        let Some(artifact) = &library.downloads.artifact else {
            continue;
        };
        let Some(relative) = artifact
            .path
            .as_deref()
            .map(PathBuf::from)
            .or_else(|| maven_path(&library.name))
        else {
            continue;
        };
        if !libraries_dir.join(relative).is_file() {
            missing.push(library.name.clone());
        }
    }
    if !missing.is_empty() {
        let sample: Vec<&str> = missing.iter().take(3).map(String::as_str).collect();
        return LoaderHealth::Broken(format!(
            "缺少 {} 个加载器文件（例如 {}）",
            missing.len(),
            sample.join("、")
        ));
    }
    let _ = (kind, version);
    LoaderHealth::Healthy
}

/// 判断一个 Maven 坐标是否属于「加载器自身发布的内容」。
fn is_loader_side_library(name: &str) -> bool {
    let group = name.split(':').next().unwrap_or_default();
    group.starts_with("net.fabricmc")
        || group.starts_with("org.quiltmc")
        || group.starts_with("net.minecraftforge")
        || group.starts_with("net.neoforged")
        || group.starts_with("cpw.mods")
        || group == "de.oceanlabs.mcp"
}

fn merge_loader_profile(
    mut base: GameVersionMetadata,
    patch: &serde_json::Value,
    libraries: Vec<Library>,
) -> Result<GameVersionMetadata, String> {
    if patch.get("inheritsFrom").and_then(|v| v.as_str()) != Some(base.id.as_str()) {
        return Err("加载器继承版本与实例不一致。".into());
    }
    base.main_class = Some(
        patch
            .get("mainClass")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .ok_or("加载器缺少入口类。")?
            .to_owned(),
    );
    // Replace conflicting Maven module versions instead of adding duplicate classpath entries.
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
    if let Some(args) = patch.get("arguments") {
        if base.arguments.is_none() {
            return Err("当前加载器适配器需要现代版本启动参数；此旧版本暂不支持。".into());
        }
        let extra: mc_metadata::LaunchArguments =
            serde_json::from_value(args.clone()).map_err(|_| "加载器启动参数无效。")?;
        let base_args = base.arguments.get_or_insert_with(Default::default);
        base_args.jvm.extend(extra.jvm);
        base_args.game.extend(extra.game);
    }
    Ok(base)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn publishing_is_atomic_and_rolls_back_store_failure() {
        let directory = std::env::temp_dir().join(uuid::Uuid::new_v4().to_string());
        std::fs::create_dir(&directory).unwrap();
        assert!(
            publish_profile(&directory, b"new", false, || Err("store failure".into())).is_err()
        );
        assert_eq!(std::fs::read_dir(&directory).unwrap().count(), 0);
        std::fs::write(directory.join("launch.json"), b"old").unwrap();
        // 非替换模式：目标已存在时必须失败，且旧内容保持不变。
        assert!(publish_profile(&directory, b"new", false, || Ok(())).is_err());
        assert_eq!(
            std::fs::read(directory.join("launch.json")).unwrap(),
            b"old"
        );
        assert_eq!(std::fs::read_dir(&directory).unwrap().count(), 1);
        std::fs::remove_file(directory.join("launch.json")).unwrap();
        publish_profile(&directory, b"committed", false, || Ok(())).unwrap();
        assert_eq!(
            std::fs::read(directory.join("launch.json")).unwrap(),
            b"committed"
        );
        assert_eq!(std::fs::read_dir(&directory).unwrap().count(), 1);
        std::fs::remove_dir_all(directory).unwrap();
    }

    /// 修复场景必须能覆盖已有的 `launch.json`。
    ///
    /// 正常安装拒绝覆盖是为了防止误操作换掉实例的加载器；但修复损坏的加载器时
    /// 那个文件一定存在，不覆盖就永远修不好。这条测试钉住两种模式的差异。
    #[test]
    fn replace_mode_overwrites_existing_launch_json() {
        let directory = std::env::temp_dir().join(uuid::Uuid::new_v4().to_string());
        std::fs::create_dir(&directory).unwrap();
        std::fs::write(directory.join("launch.json"), b"broken").unwrap();
        publish_profile(&directory, b"repaired", true, || Ok(())).unwrap();
        assert_eq!(
            std::fs::read(directory.join("launch.json")).unwrap(),
            b"repaired"
        );
        // 替换模式下提交失败要能回滚到「没有配置」而不是留下半份。
        assert!(publish_profile(&directory, b"next", true, || Err("store".into())).is_err());
        assert!(!directory.join("launch.json").exists());
        assert_eq!(std::fs::read_dir(&directory).unwrap().count(), 0);
        std::fs::remove_dir_all(directory).unwrap();
    }
    #[test]
    fn pending_retry_recognizes_the_committed_loader() {
        let directory = std::env::temp_dir().join(uuid::Uuid::new_v4().to_string());
        std::fs::create_dir_all(directory.join("versions/1.21.4")).unwrap();
        let instance = InstanceRecord {
            id: "test".into(),
            name: "test".into(),
            game_version: "1.21.4".into(),
            loader: "fabric@0.16.10".into(),
            instance_dir: directory.clone(),
            game_dir: directory.join("game"),
            isolated: true,
            state: InstanceState::Ready,
            created_at: String::new(),
            last_launched_at: None,
        };
        assert!(!loader_overlay_is_installed(&instance, LoaderKind::Fabric, "0.16.10").unwrap());
        std::fs::write(
            directory.join("versions/1.21.4/launch.json"),
            br#"{"id":"1.21.4","mainClass":"loader.Main","libraries":[]}"#,
        )
        .unwrap();
        assert!(loader_overlay_is_installed(&instance, LoaderKind::Fabric, "0.16.10").unwrap());
        assert!(!loader_overlay_is_installed(&instance, LoaderKind::Quilt, "0.16.10").unwrap());
        std::fs::remove_dir_all(directory).unwrap();
    }
    #[test]
    fn coordinates_cannot_escape_library_directory() {
        assert_eq!(
            safe_maven_path("net.fabricmc:fabric-loader:0.16.10").unwrap(),
            "net/fabricmc/fabric-loader/0.16.10/fabric-loader-0.16.10.jar"
        );
        for value in [
            "a:../b:c",
            "a:/b:c",
            "a:b:c/evil",
            ".:b:c",
            "a::c",
            "a:b:c@",
            "a:b:c@jar@txt",
            "a:CON:c",
        ] {
            assert!(safe_maven_path(value).is_err(), "{value}");
        }
    }
    #[test]
    fn overlay_keeps_official_id_and_rejects_wrong_base() {
        let base: GameVersionMetadata = serde_json::from_value(serde_json::json!({"id":"1.21.4","mainClass":"original","libraries":[],"arguments":{"game":[],"jvm":[]}})).unwrap();
        let patch = serde_json::json!({"inheritsFrom":"1.21.4","mainClass":"loader.Main","arguments":{"game":[],"jvm":["-Dloader=true"]}});
        let merged = merge_loader_profile(base.clone(), &patch, vec![]).unwrap();
        assert_eq!(merged.id, "1.21.4");
        assert_eq!(merged.main_class.as_deref(), Some("loader.Main"));
        assert!(
            merge_loader_profile(base, &serde_json::json!({"inheritsFrom":"bad"}), vec![]).is_err()
        );
    }

    // --- diagnose_loader ---

    fn instance_with_loader(root: &Path, loader: &str) -> InstanceRecord {
        InstanceRecord {
            id: "diag".into(),
            name: "diag".into(),
            game_version: "1.21.4".into(),
            loader: loader.into(),
            instance_dir: root.to_path_buf(),
            game_dir: root.join("game"),
            isolated: true,
            state: InstanceState::Ready,
            created_at: String::new(),
            last_launched_at: None,
        }
    }

    #[test]
    fn vanilla_instances_need_no_loader_repair() {
        let root = std::env::temp_dir().join(uuid::Uuid::new_v4().to_string());
        let instance = instance_with_loader(&root, "vanilla");
        assert_eq!(
            diagnose_loader(&instance, &root.join("libraries")),
            LoaderHealth::NotInstalled
        );
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn missing_launch_json_is_reported_as_broken() {
        let root = std::env::temp_dir().join(uuid::Uuid::new_v4().to_string());
        std::fs::create_dir_all(root.join("versions/1.21.4")).unwrap();
        let instance = instance_with_loader(&root, "fabric@0.16.10");
        match diagnose_loader(&instance, &root.join("libraries")) {
            LoaderHealth::Broken(reason) => assert!(reason.contains("不存在"), "{reason}"),
            other => panic!("expected Broken, got {other:?}"),
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn intact_loader_reports_healthy() {
        let root = std::env::temp_dir().join(uuid::Uuid::new_v4().to_string());
        std::fs::create_dir_all(root.join("versions/1.21.4")).unwrap();
        std::fs::create_dir_all(root.join("libraries/net/fabricmc/fabric-loader/0.16.10")).unwrap();
        std::fs::write(
            root.join("libraries/net/fabricmc/fabric-loader/0.16.10/fabric-loader-0.16.10.jar"),
            b"jar",
        )
        .unwrap();
        std::fs::write(
            root.join("versions/1.21.4/launch.json"),
            serde_json::to_vec(&serde_json::json!({
                "id": "1.21.4",
                "mainClass": "net.fabricmc.loader.impl.launch.knot.KnotClient",
                "libraries": [{
                    "name": "net.fabricmc:fabric-loader:0.16.10",
                    "downloads": {"artifact": {
                        "path": "net/fabricmc/fabric-loader/0.16.10/fabric-loader-0.16.10.jar",
                        "url": "https://maven.fabricmc.net/fabric-loader-0.16.10.jar",
                        "sha1": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                        "size": 3
                    }}
                }]
            }))
            .unwrap(),
        )
        .unwrap();
        let instance = instance_with_loader(&root, "fabric@0.16.10");
        assert_eq!(
            diagnose_loader(&instance, &root.join("libraries")),
            LoaderHealth::Healthy
        );
        // 删掉加载器的 jar —— 这正是「补全官方文件」不会碰的那类文件。
        std::fs::remove_file(
            root.join("libraries/net/fabricmc/fabric-loader/0.16.10/fabric-loader-0.16.10.jar"),
        )
        .unwrap();
        match diagnose_loader(&instance, &root.join("libraries")) {
            LoaderHealth::Broken(reason) => {
                assert!(reason.contains("缺少"), "{reason}");
                assert!(reason.contains("fabric-loader"), "{reason}");
            }
            other => panic!("expected Broken after removing the loader jar, got {other:?}"),
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn only_loader_side_libraries_are_checked() {
        // 官方库缺失不该由这一层判断：那是 install_vanilla 的责任，重复报错会误报。
        assert!(!is_loader_side_library("com.google.code.gson:gson:2.10"));
        assert!(!is_loader_side_library("org.lwjgl:lwjgl:3.3.3"));
        assert!(!is_loader_side_library("com.mojang:brigadier:1.0.18"));
        assert!(is_loader_side_library("net.fabricmc:fabric-loader:0.16.10"));
        assert!(is_loader_side_library("org.quiltmc:quilt-loader:0.27.0"));
        assert!(is_loader_side_library(
            "net.minecraftforge:fmlloader:1.20.1-47.4.28"
        ));
        assert!(is_loader_side_library("cpw.mods:securejarhandler:2.1.10"));
    }

    #[test]
    fn loader_kind_round_trips_through_its_name() {
        for kind in [
            LoaderKind::Fabric,
            LoaderKind::Quilt,
            LoaderKind::Forge,
            LoaderKind::NeoForge,
        ] {
            assert_eq!(LoaderKind::from_name(kind.name()), Some(kind));
        }
        assert_eq!(LoaderKind::from_name("nonexistent"), None);
    }
}
