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
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::mpsc;

#[path = "loaders_legacy.rs"]
pub(crate) mod legacy;
#[path = "loaders_optifine.rs"]
pub mod optifine;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum LoaderKind {
    Fabric,
    Quilt,
    Forge,
    NeoForge,
    LiteLoader,
    Cleanroom,
    LegacyFabric,
    OptiFine,
}

impl LoaderKind {
    pub fn name(self) -> &'static str {
        match self {
            Self::Fabric => "fabric",
            Self::Quilt => "quilt",
            Self::Forge => "forge",
            Self::NeoForge => "neoforge",
            Self::LiteLoader => "liteloader",
            Self::Cleanroom => "cleanroom",
            Self::LegacyFabric => "legacyfabric",
            Self::OptiFine => "optifine",
        }
    }
    fn meta(self) -> &'static str {
        match self {
            Self::Fabric => "https://meta.fabricmc.net/v2/",
            Self::Quilt => "https://meta.quiltmc.org/v3/",
            Self::LegacyFabric => "https://meta.legacyfabric.net/v2/",
            Self::LiteLoader => "https://dl.liteloader.com/versions/",
            Self::Cleanroom => "https://repo.cleanroommc.com/releases/com/cleanroommc/cleanroom/",
            Self::OptiFine => "https://optifine.net/",
            // Forge 与 NeoForge 没有 meta 服务，版本列表与配置都来自 maven 的
            // promotions / 安装器 jar，见各自的 `list_*_versions` 实现。
            Self::Forge => "https://maven.minecraftforge.net/net/minecraftforge/forge/",
            Self::NeoForge => "https://maven.neoforged.net/releases/net/neoforged/neoforge/",
        }
    }
    /// 这一族加载器是否走 installer processor 安装。
    fn uses_installer(self) -> bool {
        matches!(self, Self::Forge | Self::NeoForge | Self::Cleanroom)
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
            "liteloader" => Some(Self::LiteLoader),
            "cleanroom" => Some(Self::Cleanroom),
            "legacyfabric" => Some(Self::LegacyFabric),
            "optifine" => Some(Self::OptiFine),
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

/// 为 Fabric / Quilt 的 maven 地址推导 BMCLAPI 镜像。
///
/// 与 Forge 那份同理：官方 maven 在部分网络下不可达，而镜像的路径结构与官方
/// 一致。返回 `None` 表示不需要（或没有）镜像，此时下载器会直接走原始地址。
pub(crate) fn fabric_mirror(url: &str) -> Option<String> {
    const BMCLAPI: &str = "https://bmclapi2.bangbang93.com";
    for official in [
        "https://maven.fabricmc.net/",
        "https://maven.quiltmc.org/repository/release/",
    ] {
        if let Some(rest) = url.strip_prefix(official) {
            return Some(format!("{BMCLAPI}/maven/{rest}"));
        }
    }
    None
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
    list_loader_versions_with_settings(kind, game, &crate::DownloadSettings::default()).await
}

const LOADER_CACHE_TTL_SECS: u64 = 6 * 60 * 60;
const LOADER_CACHE_MAX_AGE_SECS: u64 = 24 * 60 * 60;
const LOADER_CACHE_MAX_BYTES: u64 = 4 * 1024 * 1024;

#[derive(Debug, Serialize, Deserialize)]
struct CachedVersions {
    fetched_at: u64,
    versions: Vec<LoaderVersion>,
}

fn cache_path(
    kind: LoaderKind,
    game: &str,
    settings: &crate::DownloadSettings,
    cache_root: Option<&Path>,
) -> Option<PathBuf> {
    safe_version(game).ok()?;
    let source = match settings.source {
        mc_metadata::SourcePreference::Official => "official",
        mc_metadata::SourcePreference::Bmclapi => "bmclapi",
        mc_metadata::SourcePreference::Mixed => "mixed",
    };
    Some(
        cache_root?
            .join("loader-versions")
            .join(format!("{}-{game}-{source}.json", kind.name())),
    )
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

async fn read_cache(path: &Path) -> Option<(Vec<LoaderVersion>, bool)> {
    use tokio::io::AsyncReadExt;

    let file = tokio::fs::File::open(path).await.ok()?;
    if file.metadata().await.ok()?.len() > LOADER_CACHE_MAX_BYTES {
        return None;
    }
    let mut bytes = Vec::new();
    file.take(LOADER_CACHE_MAX_BYTES + 1)
        .read_to_end(&mut bytes)
        .await
        .ok()?;
    if bytes.len() as u64 > LOADER_CACHE_MAX_BYTES {
        return None;
    }
    let cached: CachedVersions = serde_json::from_slice(&bytes).ok()?;
    if cached.versions.is_empty()
        || cached
            .versions
            .iter()
            .any(|entry| safe_version(&entry.version).is_err())
    {
        return None;
    }
    let age = now_secs().checked_sub(cached.fetched_at)?;
    if age > LOADER_CACHE_MAX_AGE_SECS {
        return None;
    }
    Some((cached.versions, age <= LOADER_CACHE_TTL_SECS))
}

async fn write_cache(path: &Path, versions: &[LoaderVersion]) {
    use tokio::io::AsyncWriteExt;

    if versions.is_empty()
        || versions
            .iter()
            .any(|entry| safe_version(&entry.version).is_err())
    {
        return;
    }
    let payload = CachedVersions {
        fetched_at: now_secs(),
        versions: versions.to_vec(),
    };
    let Ok(bytes) = serde_json::to_vec(&payload) else {
        return;
    };
    if bytes.len() as u64 > LOADER_CACHE_MAX_BYTES {
        return;
    }
    let Some(parent) = path.parent() else {
        return;
    };
    if tokio::fs::create_dir_all(parent).await.is_err() {
        return;
    }
    // Readers must see a complete old or new document, never a partially written list.
    let temporary = path.with_extension(format!("{}.tmp", uuid::Uuid::new_v4().simple()));
    let Ok(mut file) = tokio::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)
        .await
    else {
        return;
    };
    let written = async {
        file.write_all(&bytes).await?;
        file.flush().await
    }
    .await;
    drop(file);
    if written.is_err() || tokio::fs::rename(&temporary, path).await.is_err() {
        let _ = tokio::fs::remove_file(&temporary).await;
    }
}

/// Fresh entries avoid the network; entries up to one day old return immediately
/// while a background request refreshes them. Cache failures never hide network results.
pub async fn list_loader_versions_cached(
    kind: LoaderKind,
    game: &str,
    settings: &crate::DownloadSettings,
    cache_root: Option<&Path>,
) -> Result<Vec<LoaderVersion>, String> {
    safe_version(game)?;
    let path = cache_path(kind, game, settings, cache_root);
    let game = game.to_owned();
    let settings = settings.clone();
    cached_loader_versions(path, async move {
        list_loader_versions_with_settings(kind, &game, &settings).await
    })
    .await
}

async fn cached_loader_versions(
    path: Option<PathBuf>,
    fetch: impl std::future::Future<Output = Result<Vec<LoaderVersion>, String>> + Send + 'static,
) -> Result<Vec<LoaderVersion>, String> {
    let cached = match path.as_deref() {
        Some(path) => read_cache(path).await,
        None => None,
    };
    if let Some((versions, fresh)) = cached {
        if !fresh {
            tokio::spawn(async move {
                if let (Some(path), Ok(versions)) = (path, fetch.await) {
                    write_cache(&path, &versions).await;
                }
            });
        }
        return Ok(versions);
    }
    let versions = fetch.await?;
    if let Some(path) = path {
        write_cache(&path, &versions).await;
    }
    Ok(versions)
}

pub async fn list_loader_versions_with_settings(
    kind: LoaderKind,
    game: &str,
    settings: &crate::DownloadSettings,
) -> Result<Vec<LoaderVersion>, String> {
    safe_version(game)?;
    let network = settings.to_engine_settings();
    match kind {
        LoaderKind::OptiFine => return optifine::list_versions(game, &network).await,
        LoaderKind::LiteLoader | LoaderKind::Cleanroom => {
            return legacy::list_versions(kind, game, &network).await;
        }
        _ => {}
    }
    let client = metadata_client(&network)?;
    if kind.uses_installer() {
        return list_installer_versions(kind, game, &client, &network).await;
    }
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
    client: &reqwest::Client,
    network: &download_engine::DownloadSettings,
) -> Result<Vec<LoaderVersion>, String> {
    match kind {
        LoaderKind::Forge => forge_versions(game, client).await,
        LoaderKind::NeoForge => neoforge_versions(game, client, network).await,
        _ => Err("该加载器不使用安装器。".into()),
    }
}

/// Forge 的版本列表。
///
/// ## 为什么同时读两个来源
///
/// `promotions_slim.json` 只给「该 MC 版本当前推荐的 latest 与 recommended」两条。
/// 界面照着它做，一个 1.20.1 实例就只剩两个可选版本——而该版本实际发布过 137 个
/// Forge 构建。用户想装某个特定版本（模组作者指定、或规避某个 bug）时无从选择，
/// 这正是 PCL2 等启动器提供完整列表的原因。
///
/// 所以：**maven-metadata.xml 给全量**，`promotions_slim.json` **只用来标记哪一个
/// 是稳定版**。两个来源都失败才报错；只缺 promotions 时列表仍然可用，只是没有
/// 稳定版标记。
async fn forge_versions(
    game: &str,
    client: &reqwest::Client,
) -> Result<Vec<LoaderVersion>, String> {
    let prefix = format!("{game}-");

    /*
     * 两个来源并行拉取。
     *
     * 全量版本与稳定版标记互不依赖，串行执行会让「打开对话框」白白多等一个
     * 完整往返（实测各约 2 秒）。`join!` 让它们同时发出，总耗时取两者的较大值。
     *
     * 失败处理保持各自独立：全量列表失败才算整体失败，promotions 失败只是没有
     * 稳定版徽标——把两者绑在一起会让一个可选装饰拖垮整个列表。
     */
    collect_forge_versions(
        forge_maven_versions(&prefix, client),
        forge_promoted(game, client),
    )
    .await
}

async fn collect_forge_versions(
    maven: impl std::future::Future<Output = Result<Vec<String>, String>>,
    promotions: impl std::future::Future<Output = Result<std::collections::BTreeSet<String>, String>>,
) -> Result<Vec<LoaderVersion>, String> {
    let (versions_result, promoted) = tokio::join!(maven, promotions);
    let mut versions = versions_result?;
    let promoted = promoted.unwrap_or_default();
    versions.sort_by(|a, b| compare_versions(b, a));
    versions.dedup();

    Ok(versions
        .into_iter()
        .map(|version| {
            let stable = promoted.contains(&version);
            LoaderVersion { version, stable }
        })
        .collect())
}

/// 从 maven 元数据里抓出该 MC 版本的全部 Forge 构建号。
///
/// 单独拆出来是为了能和 `forge_promoted` 并行执行：两者互不依赖，串行会让
/// 「打开对话框」白白多等一个往返。
async fn forge_maven_versions(
    prefix: &str,
    client: &reqwest::Client,
) -> Result<Vec<String>, String> {
    let response = client
        .get("https://maven.minecraftforge.net/net/minecraftforge/forge/maven-metadata.xml")
        .send()
        .await
        .map_err(|_| "无法连接 Forge 版本服务。")?
        .error_for_status()
        .map_err(|_| "Forge 版本服务拒绝了请求。")?;
    let bytes = bounded_response(response, 16 * 1024 * 1024).await?;
    let text = std::str::from_utf8(&bytes).map_err(|_| "Forge 版本列表不是有效文本。")?;
    let mut versions: Vec<String> = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find("<version>") {
        let after = &rest[start + "<version>".len()..];
        let Some(end) = after.find("</version>") else {
            break;
        };
        let value = after[..end].trim();
        if value.starts_with(prefix) && safe_version(value).is_ok() {
            versions.push(value.to_owned());
        }
        rest = &after[end + "</version>".len()..];
    }
    if versions.is_empty() {
        return Err(format!(
            "Minecraft {} 没有可用的 Forge 版本。",
            prefix.trim_end_matches('-')
        ));
    }
    Ok(versions)
}

/// 该 MC 版本被官方标为 latest / recommended 的 Forge 构建。
///
/// 取失败时返回空集合而不是错误：这只是「给某一项加个徽标」，不值得让它挡住
/// 整个版本列表。
async fn forge_promoted(
    game: &str,
    client: &reqwest::Client,
) -> Result<std::collections::BTreeSet<String>, String> {
    let response = client
        .get("https://files.minecraftforge.net/net/minecraftforge/forge/promotions_slim.json")
        .send()
        .await
        .map_err(|_| "无法读取 Forge 推荐版本。")?
        .error_for_status()
        .map_err(|_| "Forge 推荐版本服务拒绝了请求。")?;
    let bytes = bounded_response(response, 4 * 1024 * 1024).await?;
    let value: serde_json::Value =
        serde_json::from_slice(&bytes).map_err(|_| "Forge 推荐版本格式无效。")?;
    let Some(promos) = value.get("promos").and_then(|v| v.as_object()) else {
        return Ok(Default::default());
    };
    let mut out = std::collections::BTreeSet::new();
    for (key, version) in promos {
        // 键形如 `<mc>-latest` / `<mc>-recommended`。
        let Some((mc, _channel)) = key.rsplit_once('-') else {
            continue;
        };
        if mc != game {
            continue;
        }
        if let Some(version) = version.as_str() {
            /*
             * promotions 里的值是**构建号**（`47.4.28`），而 maven 条目是
             * `<mc>-<构建号>`（`1.20.1-47.4.28`）。不补前缀就永远匹配不上，
             * 表现为「列表里有 137 个版本，但没有一个被标为稳定版」。
             */
            out.insert(format!("{game}-{version}"));
        }
    }
    Ok(out)
}

/// 记录「本进程内官方 NeoForge 源已确认不可达」，避免 `Mixed` 每次都白等一轮超时。
///
/// 只在超时（而不是 4xx/5xx）时置位：4xx/5xx 说明主机可达，下次仍值得尝试。
/// 进程重启后自动重置。
static OFFICIAL_UNREACHABLE: AtomicBool = AtomicBool::new(false);

async fn neoforge_versions(
    game: &str,
    client: &reqwest::Client,
    network: &download_engine::DownloadSettings,
) -> Result<Vec<LoaderVersion>, String> {
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

    /*
     * 顺序跟着用户的下载源设置走，而不是写死「官方优先」。
     *
     * 实测（中文区常见网络）：`maven.neoforged.net` 连接会一直挂到超时，
     * 而 bmclapi 0.1 秒就能连上。写死官方优先时，每次列版本都要先白等一个
     * 完整超时（实测 21 秒）才轮到镜像，用户看到的就是「NeoForge 特别慢」。
     *
     * 用户选了镜像或混合，就说明他希望优先用镜像；只有选「官方」才保持
     * 只用官方——那条路径下不该偷偷回退到镜像，否则同一个设置在不同加载器
     * 上会有两种含义。
     */
    const OFFICIAL: &str =
        "https://maven.neoforged.net/releases/net/neoforged/neoforge/maven-metadata.xml";
    const MIRROR: &str =
        "https://bmclapi2.bangbang93.com/maven/net/neoforged/neoforge/maven-metadata.xml";

    let mirror = mirror_client(network)?;
    /*
     * 每个设置只列出真正要尝试的源，不做「重复同一个源两次」这种占位：
     * 那会让镜像模式在第一次超时后再白等一次，用户看到的仍是慢。
     *
     * `Mixed` 把镜像放前面：它通常快一个数量级，且 302 跳转链偶发要几秒。
     * 官方只在镜像彻底失败后才尝试，且各自独立计时——一个源卡住不会吃掉
     * 另一个源的时间。
     */
    let order: Vec<(&str, &reqwest::Client)> = match network.source {
        mc_metadata::SourcePreference::Official => vec![(OFFICIAL, client)],
        mc_metadata::SourcePreference::Bmclapi => vec![(MIRROR, &mirror)],
        mc_metadata::SourcePreference::Mixed => {
            /*
             * 官方源一旦在本进程里超时过，就不再为它重复付一次完整超时。
             *
             * 实测：某些网络下 `maven.neoforged.net` 完全不可达，每次都要等满
             * 超时才轮空。`Mixed` 的语义是「镜像优先、官方兜底」，而一个已经
             * 证明不可达的兜底只会让每次列版本都多等 20 秒——那正是用户感知到的
             * 「NeoForge 特别慢」。进程重启后会自动重新尝试。
             */
            if OFFICIAL_UNREACHABLE.load(Ordering::Relaxed) {
                vec![(MIRROR, &mirror)]
            } else {
                vec![(MIRROR, &mirror), (OFFICIAL, client)]
            }
        }
    };

    let mut fetched = None;
    let mut last_error = String::new();
    let mut first_source = true;
    for (url, client) in order {
        if fetched.is_some() {
            break;
        }
        /*
         * 首选源（镜像）失败时重试一次，兜底源不重试。
         *
         * BMCLAPI 会 302 到多个节点（`open-bmcl-api.lunar.cc`、`bmclapi2.hurtmc.com`
         * 等），其中部分节点在特定网络下 DNS 解析失败。实测同一地址连续请求，
         * 有的跳转到可用节点（200，1.4 秒），有的跳转到解析不了的节点——这是
         * 瞬时故障，重试一次就能换一个节点。
         *
         * 只重试一次：BMCLAPI 的限流窗口是 57 秒内 10 次（响应头
         * `X-RateLimit-Limit-short: 10`），多轮重试会把配额烧光、直接收到 429，
         * 那比慢更糟。
         *
         * ## 兜底源的窗口要短
         *
         * 首选源给满预算：它通常是能用的那个，值得等。
         *
         * 兜底源只给 5 秒。它存在的意义是「首选源真的坏了」时还能救一次，而不是
         * 让用户为一个已经证明不可达的主机付 20 秒。实测 `maven.neoforged.net`
         * 在中文区常见网络下完全不可达（3/3 次 21 秒超时），等满 20 秒再报错，
         * 用户看到的就是「点开对话框，转半分钟，然后失败」——比直接失败更糟。
         */
        let budget = if first_source {
            std::time::Duration::from_secs(u64::from(network.timeout_sec).min(20))
        } else {
            std::time::Duration::from_secs(5)
        };
        let attempts = if first_source { 2 } else { 1 };
        for attempt in 0..attempts {
            if attempt > 0 {
                // 给 DNS/连接一点恢复时间，但别让用户感知到停顿。
                tokio::time::sleep(std::time::Duration::from_millis(250)).await;
            }
            match tokio::time::timeout(budget, client.get(url).send()).await {
                Ok(Ok(response)) if response.status().is_success() => {
                    match bounded_response(response, 8 * 1024 * 1024).await {
                        Ok(bytes) => {
                            fetched = Some(bytes);
                            break;
                        }
                        Err(error) => last_error = error,
                    }
                }
                /*
                 * 4xx/5xx 不重试：429 说明配额已经用完，再打只会更糟；其余状态码
                 * 是稳定的服务端答复，重试不会改变结果。
                 */
                Ok(Ok(response)) => {
                    last_error = format!("HTTP {}", response.status());
                    break;
                }
                Ok(Err(_)) => last_error = "无法连接".to_owned(),
                Err(_) => {
                    last_error = "连接超时".to_owned();
                    if url == OFFICIAL {
                        OFFICIAL_UNREACHABLE.store(true, Ordering::Relaxed);
                    }
                    // 超时说明主机真的不可达，重试也只是再等一个完整超时。
                    break;
                }
            }
        }
        first_source = false;
    }
    let bytes = fetched
        .ok_or_else(|| format!("无法连接 NeoForge 版本服务（官方与镜像均失败：{last_error}）。"))?;
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

fn metadata_client(network: &download_engine::DownloadSettings) -> Result<reqwest::Client, String> {
    network
        .configure_metadata_client(
            reqwest::Client::builder()
                .user_agent(concat!("MFMCL/", env!("CARGO_PKG_VERSION")))
                .https_only(true)
                .redirect(reqwest::redirect::Policy::none()),
            &[
                "meta.fabricmc.net",
                "meta.quiltmc.org",
                "meta.legacyfabric.net",
                "maven.legacyfabric.net",
                "maven.minecraftforge.net",
                "files.minecraftforge.net",
                "maven.neoforged.net",
                "maven.fabricmc.net",
                "maven.quiltmc.org",
                "libraries.minecraft.net",
                "repo1.maven.org",
                "repo.maven.apache.org",
            ],
        )
        .map_err(|e| e.to_string())?
        .build()
        .map_err(|_| "无法初始化 HTTPS 客户端。".into())
}

/// 允许跟随重定向的客户端，只用于镜像。
///
/// BMCLAPI 对文件与元数据都返回 302，跳到 `*.hurtmc.com` / `mirrors.*` 上的
/// 短时签名地址。默认客户端设了 `Policy::none()`（安全考虑：不跟随任何跳转），
/// 所以镜像永远拿到 302 而不是内容。
///
/// 这里单独开一个允许有限跳转的实例，并且仍然限制在 HTTPS。跳转目标不固定：
/// bmclapi 会按负载把请求分发到 `bmclapi.hurtmc.com`、`mirrors.qlu.edu.cn` 等
/// 镜像节点，写死单一主机就会时好时坏——这正是「镜像能力差」的来源。
///
/// ## 为什么要允许几跳，而不是一跳
///
/// 实测 bmclapi 的元数据地址会经过**两级**跳转（`bmclapi2` → 某个节点 →
/// 最终地址）。只允许一跳时，第二次跳转会被当成错误，镜像永远拿不到内容——
/// 表现为「镜像配了反而全都失败」。
///
/// ## 为什么跳转预算不能太大
///
/// `bmclapi.hurtmc.com` 这类跳转目标在部分网络下 DNS 解析失败。追得太多，每次
/// 失败都要重新解析、重新连接，把整个请求预算耗在追一个根本到不了的主机上。
/// 3 跳够覆盖正常链路，又不至于在死路上绕圈。
fn mirror_client(network: &download_engine::DownloadSettings) -> Result<reqwest::Client, String> {
    network
        .configure_metadata_client(
            reqwest::Client::builder()
                .user_agent(concat!("MFMCL/", env!("CARGO_PKG_VERSION")))
                .https_only(true)
                .redirect(reqwest::redirect::Policy::custom(|attempt| {
                    if attempt.previous().len() >= 3 {
                        return attempt.error("mirror redirected too many times");
                    }
                    // 只允许 HTTPS，避免被降级到明文。
                    if attempt.url().scheme() != "https" {
                        return attempt.error("insecure mirror redirect");
                    }
                    attempt.follow()
                })),
            &["bmclapi2.bangbang93.com"],
        )
        .map_err(|e| e.to_string())?
        .build()
        .map_err(|_| "无法初始化镜像客户端。".into())
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
    if kind == LoaderKind::OptiFine {
        return optifine::install(
            root,
            instance,
            store,
            engine,
            version,
            progress,
            allow_same_loader && instance.loader != "vanilla",
        )
        .await;
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
    let client = if matches!(kind, LoaderKind::LiteLoader | LoaderKind::LegacyFabric) {
        legacy::client(engine.settings())?
    } else {
        metadata_client(engine.settings())?
    };
    let patch: serde_json::Value = if kind == LoaderKind::LiteLoader {
        legacy::lite_profile(&instance.game_version, version, &client).await?
    } else {
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
        serde_json::from_slice(&bytes).map_err(|_| "加载器配置格式无效。")?
    };
    if patch.get("processors").is_some() {
        return Err("加载器配置包含不支持的处理器；不会执行任意程序。".into());
    }
    let version_dir = instance.version_dir();
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
        if library.get("natives").is_some() {
            libraries.push(
                legacy::native_library(root, library, engine, &client, progress.clone()).await?,
            );
            continue;
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
                        | "maven.legacyfabric.net"
                        | "dl.liteloader.com"
                        | "repo.mumfrey.com"
                        | "repo.spongepowered.org"
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
        let url = if kind == LoaderKind::LiteLoader && name.starts_with("com.mumfrey:liteloader:") {
            let value = library
                .get("mfmclDownloadUrl")
                .and_then(|v| v.as_str())
                .ok_or("LiteLoader 缺少下载地址。")?;
            legacy::allowed_url(&reqwest::Url::parse(value).map_err(|e| e.to_string())?)?;
            value.to_owned()
        } else {
            repository
                .join(&relative)
                .map_err(|e| e.to_string())?
                .to_string()
        };
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
                    url: url.clone(),
                    /*
                     * Fabric / Quilt 的库同样走 BMCLAPI。
                     * `maven.fabricmc.net` 与 `maven.quiltmc.org` 在部分网络下不可达，
                     * 而 BMCLAPI 镜像了它们，路径结构一致。
                     */
                    mirror_url: fabric_mirror(&url),
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

pub(crate) fn publish_profile(
    directory: &Path,
    bytes: &[u8],
    replace_existing: bool,
    commit: impl FnOnce() -> Result<(), String>,
) -> Result<(), String> {
    use std::io::Write;
    let target = directory.join("launch.json");
    let temp = directory.join(format!(".launch-{}.pending", uuid::Uuid::new_v4()));
    let backup = directory.join(format!(".launch-{}.backup", uuid::Uuid::new_v4()));
    let old = match std::fs::symlink_metadata(&target) {
        Ok(meta) if meta.file_type().is_file() && meta.len() <= 16 * 1024 * 1024 => true,
        Ok(_) => return Err("启动配置必须是普通文件。".into()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
        Err(error) => return Err(error.to_string()),
    };
    if old && !replace_existing {
        return Err("已有启动配置；不会覆盖。".into());
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
        if old {
            std::fs::hard_link(&target, &backup).map_err(|e| e.to_string())?;
        }
        if old {
            std::fs::rename(&temp, &target).map_err(|e| e.to_string())?;
        } else {
            std::fs::hard_link(&temp, &target).map_err(|e| e.to_string())?;
        }
        published = true;
        commit()
    })();
    drop(file);
    if let Err(mut error) = result {
        if published {
            let rollback = if old {
                std::fs::rename(&backup, &target)
            } else {
                std::fs::remove_file(&target)
            };
            if let Err(rollback) = rollback {
                let _ = std::fs::remove_file(&temp);
                return Err(format!(
                    "{error}；回滚失败：{rollback}；原配置备份为 {}",
                    backup.display()
                ));
            }
        }
        if let Err(cleanup) = std::fs::remove_file(&temp) {
            if cleanup.kind() != std::io::ErrorKind::NotFound {
                error = format!("{error}；{cleanup}");
            }
        }
        let _ = std::fs::remove_file(&backup);
        return Err(error);
    }
    let _ = std::fs::remove_file(&temp);
    let _ = std::fs::remove_file(&backup);
    Ok(())
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
    let path = instance.version_dir().join("launch.json");
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
    let path = instance.version_dir().join("launch.json");
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
        || matches!(
            group,
            "net.legacyfabric" | "com.mumfrey" | "com.cleanroommc" | "top.outlands" | "optifine"
        )
        || group == "de.oceanlabs.mcp"
}

pub(crate) fn library_module(name: &str) -> String {
    let parts: Vec<_> = name.split(':').collect();
    format!(
        "{}:{}:{}",
        parts.first().unwrap_or(&""),
        parts.get(1).unwrap_or(&""),
        parts.get(3).unwrap_or(&"")
    )
}

pub(crate) fn modernize_arguments(base: &mut GameVersionMetadata) -> Result<(), String> {
    if base.arguments.is_none() {
        let game = shlex::split(base.minecraft_arguments.as_deref().unwrap_or(""))
            .ok_or("旧版本启动参数无效。")?;
        base.arguments = Some(mc_metadata::LaunchArguments {
            game: game
                .into_iter()
                .map(mc_metadata::ArgumentEntry::Literal)
                .collect(),
            jvm: vec![
                "-Djava.library.path=${natives_directory}",
                "-cp",
                "${classpath}",
            ]
            .into_iter()
            .map(|v| mc_metadata::ArgumentEntry::Literal(v.into()))
            .collect(),
        });
    }
    Ok(())
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
        let key = library_module(&library.name);
        base.libraries
            .retain(|old| library_module(&old.name) != key);
        base.libraries.push(library);
    }
    if let Some(args) = patch.get("arguments") {
        modernize_arguments(&mut base)?;
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

    mod discovery {
        use super::*;
        use std::time::Duration;

        struct CacheFixture(PathBuf);

        impl CacheFixture {
            fn new() -> Self {
                let root = std::env::temp_dir()
                    .join(format!("mfmcl-loader-cache-{}", uuid::Uuid::new_v4()));
                std::fs::create_dir_all(&root).unwrap();
                Self(root)
            }

            fn path(&self) -> PathBuf {
                cache_path(
                    LoaderKind::NeoForge,
                    "1.21.1",
                    &crate::DownloadSettings::default(),
                    Some(&self.0),
                )
                .unwrap()
            }

            async fn seed(&self, fetched_at: u64, versions: Vec<LoaderVersion>) {
                let path = self.path();
                tokio::fs::create_dir_all(path.parent().unwrap())
                    .await
                    .unwrap();
                tokio::fs::write(
                    path,
                    serde_json::to_vec(&CachedVersions {
                        fetched_at,
                        versions,
                    })
                    .unwrap(),
                )
                .await
                .unwrap();
            }
        }

        impl Drop for CacheFixture {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }

        fn versions(version: &str) -> Vec<LoaderVersion> {
            vec![LoaderVersion {
                version: version.into(),
                stable: true,
            }]
        }

        #[tokio::test]
        async fn fresh_cache_returns_through_public_api_without_network() {
            let cache = CacheFixture::new();
            write_cache(&cache.path(), &versions("21.1.251")).await;
            let settings = crate::DownloadSettings::default();
            let result = tokio::time::timeout(
                Duration::from_secs(3),
                list_loader_versions_cached(
                    LoaderKind::NeoForge,
                    "1.21.1",
                    &settings,
                    Some(&cache.0),
                ),
            )
            .await
            .unwrap()
            .unwrap();
            assert_eq!(result[0].version, "21.1.251");
            assert!(result[0].stable);
            let result = cached_loader_versions(Some(cache.path()), async {
                panic!("fresh cache must not poll the network future");
            })
            .await
            .unwrap();
            assert_eq!(result[0].version, "21.1.251");
        }

        #[tokio::test]
        async fn cold_miss_and_background_refresh_keep_the_configured_proxy() {
            use crate::catalog::network_tests::MockProxy;

            let cache = CacheFixture::new();
            let proxy = MockProxy::new("502 Bad Gateway", Duration::ZERO).await;
            let settings = proxy.settings(0);
            let path = cache_path(LoaderKind::Fabric, "1.21.1", &settings, Some(&cache.0)).unwrap();
            assert!(
                tokio::time::timeout(
                    Duration::from_secs(3),
                    list_loader_versions_cached(
                        LoaderKind::Fabric,
                        "1.21.1",
                        &settings,
                        Some(&cache.0),
                    ),
                )
                .await
                .unwrap()
                .is_err()
            );
            assert_eq!(proxy.requests.lock().unwrap().len(), 1);
            tokio::fs::create_dir_all(path.parent().unwrap())
                .await
                .unwrap();
            let stale = serde_json::to_vec(&CachedVersions {
                fetched_at: now_secs() - LOADER_CACHE_TTL_SECS - 60,
                versions: versions("0.16.10"),
            })
            .unwrap();
            tokio::fs::write(&path, &stale).await.unwrap();
            let result = tokio::time::timeout(
                Duration::from_secs(3),
                list_loader_versions_cached(
                    LoaderKind::Fabric,
                    "1.21.1",
                    &settings,
                    Some(&cache.0),
                ),
            )
            .await
            .unwrap()
            .unwrap();
            drop(settings);
            assert_eq!(result[0].version, "0.16.10");
            tokio::time::timeout(Duration::from_secs(3), async {
                while proxy.requests.lock().unwrap().len() < 2 {
                    tokio::task::yield_now().await;
                }
            })
            .await
            .unwrap();
            assert!(
                proxy.requests.lock().unwrap().iter().all(|request| {
                    request.starts_with("CONNECT meta.fabricmc.net:443 HTTP/1.1")
                })
            );
            assert_eq!(tokio::fs::read(&path).await.unwrap(), stale);
        }

        #[tokio::test]
        async fn cache_keys_separate_sources_games_and_loaders() {
            let cache = CacheFixture::new();
            write_cache(&cache.path(), &versions("21.1.251")).await;
            let settings = crate::DownloadSettings {
                source: mc_metadata::SourcePreference::Official,
                ..Default::default()
            };
            for path in [
                cache_path(LoaderKind::NeoForge, "1.21.1", &settings, Some(&cache.0)),
                cache_path(
                    LoaderKind::Forge,
                    "1.21.1",
                    &crate::DownloadSettings::default(),
                    Some(&cache.0),
                ),
                cache_path(
                    LoaderKind::NeoForge,
                    "1.21.2",
                    &crate::DownloadSettings::default(),
                    Some(&cache.0),
                ),
            ] {
                assert!(read_cache(&path.unwrap()).await.is_none());
            }
        }

        #[tokio::test]
        async fn invalid_game_is_rejected_before_cache_access_or_fetch() {
            let cache = CacheFixture::new();
            for game in ["", "../1.21.1", "1.21.1/../../outside", "C:\\outside"] {
                let settings = crate::DownloadSettings::default();
                assert!(cache_path(LoaderKind::Fabric, game, &settings, Some(&cache.0)).is_none());
                assert!(
                    list_loader_versions_cached(
                        LoaderKind::Fabric,
                        game,
                        &settings,
                        Some(&cache.0)
                    )
                    .await
                    .is_err()
                );
            }
            assert!(!cache.0.join("loader-versions").exists());
        }

        #[tokio::test]
        async fn corrupt_oversized_expired_future_or_invalid_entries_are_cache_misses() {
            let cache = CacheFixture::new();
            for (fetched_at, entries) in [
                (
                    now_secs() - LOADER_CACHE_MAX_AGE_SECS - 60,
                    versions("21.1.1"),
                ),
                (now_secs() + 3600, versions("21.1.1")),
                (now_secs(), Vec::new()),
                (now_secs(), versions("../../outside")),
            ] {
                cache.seed(fetched_at, entries).await;
                assert!(read_cache(&cache.path()).await.is_none());
            }
            for bytes in [
                b"{ broken json".to_vec(),
                vec![b' '; LOADER_CACHE_MAX_BYTES as usize + 1],
            ] {
                tokio::fs::write(cache.path(), bytes).await.unwrap();
                assert!(read_cache(&cache.path()).await.is_none());
            }
        }

        #[tokio::test]
        async fn stale_cache_returns_before_refresh_finishes_then_is_replaced() {
            let cache = CacheFixture::new();
            cache
                .seed(
                    now_secs() - LOADER_CACHE_TTL_SECS - 60,
                    versions("21.1.250"),
                )
                .await;
            assert!(!read_cache(&cache.path()).await.unwrap().1);
            let (release, wait) = tokio::sync::oneshot::channel();
            let result = tokio::time::timeout(
                Duration::from_secs(3),
                cached_loader_versions(Some(cache.path()), async move {
                    wait.await.unwrap();
                    Ok(versions("21.1.251"))
                }),
            )
            .await
            .unwrap()
            .unwrap();
            assert_eq!(result[0].version, "21.1.250");
            release.send(()).unwrap();
            tokio::time::timeout(Duration::from_secs(3), async {
                loop {
                    let (entries, fresh) = read_cache(&cache.path()).await.unwrap();
                    if fresh && entries[0].version == "21.1.251" {
                        break;
                    }
                    tokio::task::yield_now().await;
                }
            })
            .await
            .unwrap();
        }

        #[tokio::test]
        async fn failed_background_refresh_keeps_the_stale_document() {
            let cache = CacheFixture::new();
            cache
                .seed(
                    now_secs() - LOADER_CACHE_TTL_SECS - 60,
                    versions("21.1.250"),
                )
                .await;
            let before = tokio::fs::read(cache.path()).await.unwrap();
            let (done, finished) = tokio::sync::oneshot::channel();
            let result = cached_loader_versions(Some(cache.path()), async move {
                done.send(()).unwrap();
                Err("offline".into())
            })
            .await
            .unwrap();
            assert_eq!(result[0].version, "21.1.250");
            finished.await.unwrap();
            assert_eq!(tokio::fs::read(cache.path()).await.unwrap(), before);
        }

        #[tokio::test]
        async fn cold_miss_fetches_and_persists_the_list() {
            let cache = CacheFixture::new();
            let result =
                cached_loader_versions(Some(cache.path()), async { Ok(versions("21.1.251")) })
                    .await
                    .unwrap();
            assert_eq!(result[0].version, "21.1.251");
            let (entries, fresh) = read_cache(&cache.path()).await.unwrap();
            assert_eq!(entries[0].version, "21.1.251");
            assert!(fresh);
        }

        #[tokio::test]
        async fn absent_cache_root_preserves_network_success_and_failure() {
            assert!(
                cache_path(
                    LoaderKind::NeoForge,
                    "1.21.1",
                    &crate::DownloadSettings::default(),
                    None
                )
                .is_none()
            );
            assert_eq!(
                cached_loader_versions(None, async { Ok(versions("21.1.251")) })
                    .await
                    .unwrap()[0]
                    .version,
                "21.1.251"
            );
            assert_eq!(
                cached_loader_versions(None, async { Err("offline".into()) })
                    .await
                    .unwrap_err(),
                "offline"
            );
        }

        #[tokio::test]
        async fn cache_write_failure_does_not_hide_a_network_result() {
            let cache = CacheFixture::new();
            let blocked = cache.0.join("blocked");
            tokio::fs::write(&blocked, b"not a directory")
                .await
                .unwrap();
            let result = cached_loader_versions(Some(blocked.join("entry.json")), async {
                Ok(versions("21.1.251"))
            })
            .await
            .unwrap();
            assert_eq!(result[0].version, "21.1.251");
        }

        #[tokio::test]
        async fn replacement_and_concurrent_writes_leave_complete_json_without_temp_files() {
            let cache = CacheFixture::new();
            let path = cache.path();
            write_cache(&path, &versions("21.1.250")).await;
            write_cache(&path, &versions("21.1.251")).await;
            assert_eq!(read_cache(&path).await.unwrap().0[0].version, "21.1.251");
            let a = versions("21.1.252");
            let b = versions("21.1.253");
            tokio::join!(write_cache(&path, &a), write_cache(&path, &b));
            let entries = read_cache(&path).await.unwrap().0;
            assert!(["21.1.252", "21.1.253"].contains(&entries[0].version.as_str()));
            assert_eq!(
                std::fs::read_dir(path.parent().unwrap()).unwrap().count(),
                1
            );
        }

        #[tokio::test]
        async fn empty_or_unsafe_results_do_not_replace_valid_cache() {
            let cache = CacheFixture::new();
            write_cache(&cache.path(), &versions("21.1.251")).await;
            write_cache(&cache.path(), &[]).await;
            write_cache(&cache.path(), &versions("../unsafe")).await;
            assert_eq!(
                read_cache(&cache.path()).await.unwrap().0[0].version,
                "21.1.251"
            );
        }

        #[tokio::test]
        async fn forge_requests_run_concurrently_and_keep_sorting_deduplication_and_stability() {
            let (maven_started, maven_ready) = tokio::sync::oneshot::channel();
            let (promotions_started, promotions_ready) = tokio::sync::oneshot::channel();
            let maven = async move {
                maven_started.send(()).unwrap();
                promotions_ready.await.unwrap();
                Ok(vec![
                    "1.20.1-47.4.9".into(),
                    "1.20.1-47.4.10".into(),
                    "1.20.1-47.4.9".into(),
                ])
            };
            let promotions = async move {
                promotions_started.send(()).unwrap();
                maven_ready.await.unwrap();
                Ok(std::collections::BTreeSet::from(["1.20.1-47.4.10".into()]))
            };
            let result = tokio::time::timeout(
                Duration::from_secs(3),
                collect_forge_versions(maven, promotions),
            )
            .await
            .unwrap()
            .unwrap();
            assert_eq!(result.len(), 2);
            assert_eq!(result[0].version, "1.20.1-47.4.10");
            assert!(result[0].stable);
            assert!(!result[1].stable);
        }

        #[tokio::test]
        async fn forge_promotions_are_optional_but_maven_failure_is_not_hidden() {
            let result =
                collect_forge_versions(async { Ok(vec!["1.20.1-47.4.10".into()]) }, async {
                    Err("offline".into())
                })
                .await
                .unwrap();
            assert_eq!(result.len(), 1);
            assert!(!result[0].stable);
            assert_eq!(
                collect_forge_versions(async { Err("maven offline".into()) }, async {
                    Ok(Default::default())
                })
                .await
                .unwrap_err(),
                "maven offline"
            );
        }
    }

    #[test]
    fn legacy_arguments_and_classifier_libraries_survive_overlay() {
        let base: GameVersionMetadata = serde_json::from_value(serde_json::json!({"id":"1.8.9", "minecraftArguments":"--username ${auth_player_name}", "libraries":[]})).unwrap();
        let patch = serde_json::json!({"inheritsFrom":"1.8.9", "mainClass":"net.minecraft.launchwrapper.Launch", "arguments":{"game":["--tweakClass","com.mumfrey.liteloader.launch.LiteLoaderTweaker"]}});
        let libraries = [
            "org.lwjgl:lwjgl:3:natives-windows",
            "org.lwjgl:lwjgl:3:natives-linux",
            "org.lwjgl:lwjgl:3",
        ]
        .into_iter()
        .map(|name| Library {
            name: name.into(),
            ..Default::default()
        })
        .collect();
        let merged = merge_loader_profile(base, &patch, libraries).unwrap();
        assert_eq!(merged.libraries.len(), 3);
        let args = merged.arguments.unwrap();
        assert_eq!(args.game.len(), 4);
        assert_eq!(args.jvm.len(), 3);
    }

    #[tokio::test]
    async fn every_loader_discovery_path_uses_the_proxy_including_neoforge_fallback() {
        use crate::catalog::network_tests::MockProxy;
        use std::time::Duration;

        for (kind, hosts) in [
            (LoaderKind::Fabric, vec!["meta.fabricmc.net"]),
            (LoaderKind::Quilt, vec!["meta.quiltmc.org"]),
            /*
             * Forge 要读两个来源：maven 元数据给全量版本，promotions 给稳定版
             * 标记。两者并行发出，所以这里列全两个主机。
             */
            (
                LoaderKind::Forge,
                vec!["maven.minecraftforge.net", "files.minecraftforge.net"],
            ),
            /*
             * 默认设置是 `Mixed`，NeoForge 先走镜像、失败再回退官方。
             * 顺序反了会让中文区用户每次都先白等官方超时。
             */
            (
                LoaderKind::NeoForge,
                vec!["bmclapi2.bangbang93.com", "maven.neoforged.net"],
            ),
        ] {
            let proxy = MockProxy::new("502 Bad Gateway", Duration::ZERO).await;
            assert!(
                list_loader_versions_with_settings(kind, "1.21.4", &proxy.settings(1))
                    .await
                    .is_err()
            );
            let requests = proxy.requests.lock().unwrap();
            /*
             * 断言「每条发现路径都经过代理、且只访问该访问的主机」，而不是具体
             * 请求次数或顺序。
             *
             * 次数取决于重试策略（首选源会重试一次，兜底源不会），顺序取决于
             * 并行调度——两者都是实现细节，不是这个测试要保护的行为。这里真正
             * 要守住的是：**没有请求绕过代理、也没有打到不该打的主机**。
             */
            for request in requests.iter() {
                assert!(
                    hosts
                        .iter()
                        .any(|host| request.starts_with(&format!("CONNECT {host}:443 HTTP/1.1"))),
                    "{kind:?} contacted an unexpected host: {request:?}"
                );
            }
            for host in &hosts {
                assert!(
                    requests
                        .iter()
                        .any(|request| request
                            .starts_with(&format!("CONNECT {host}:443 HTTP/1.1"))),
                    "{kind:?} never reached {host}: {requests:?}"
                );
            }
        }
    }

    /// 选「官方」时不能偷偷回退到镜像：同一个设置在不同加载器上必须是同一种含义。
    #[tokio::test]
    async fn neoforge_official_only_never_contacts_the_mirror() {
        use crate::catalog::network_tests::MockProxy;
        use std::time::Duration;

        let proxy = MockProxy::new("502 Bad Gateway", Duration::ZERO).await;
        let mut settings = proxy.settings(0);
        settings.source = mc_metadata::SourcePreference::Official;
        assert!(
            list_loader_versions_with_settings(LoaderKind::NeoForge, "1.21.4", &settings)
                .await
                .is_err()
        );
        let requests = proxy.requests.lock().unwrap();
        assert!(
            requests
                .iter()
                .all(|request| request.starts_with("CONNECT maven.neoforged.net:443 HTTP/1.1")),
            "official-only must not touch the mirror: {requests:?}"
        );
    }

    /// 选「镜像」时不能先打官方：那正是「NeoForge 特别慢」的直接原因。
    #[tokio::test]
    async fn neoforge_mirror_only_never_contacts_official() {
        use crate::catalog::network_tests::MockProxy;
        use std::time::Duration;

        let proxy = MockProxy::new("502 Bad Gateway", Duration::ZERO).await;
        let mut settings = proxy.settings(0);
        settings.source = mc_metadata::SourcePreference::Bmclapi;
        assert!(
            list_loader_versions_with_settings(LoaderKind::NeoForge, "1.21.4", &settings)
                .await
                .is_err()
        );
        let requests = proxy.requests.lock().unwrap();
        assert!(
            requests
                .iter()
                .all(|request| request.starts_with("CONNECT bmclapi2.bangbang93.com:443 HTTP/1.1")),
            "mirror-only must not touch the official host: {requests:?}"
        );
    }

    #[tokio::test]
    async fn forge_promotions_and_loader_hash_reads_keep_the_configured_proxy() {
        use crate::catalog::network_tests::MockProxy;
        use std::time::Duration;

        let proxy = MockProxy::new("502 Bad Gateway", Duration::ZERO).await;
        let client = metadata_client(&proxy.settings(0).to_engine_settings()).unwrap();
        assert!(forge_promoted("1.21.4", &client).await.is_err());
        assert!(
            client
                .get("https://maven.fabricmc.net/example.jar.sha1")
                .send()
                .await
                .is_err()
        );
        let requests = proxy.requests.lock().unwrap();
        assert_eq!(requests.len(), 2);
        assert!(requests[0].starts_with("CONNECT files.minecraftforge.net:443 HTTP/1.1"));
        assert!(requests[1].starts_with("CONNECT maven.fabricmc.net:443 HTTP/1.1"));
    }

    #[tokio::test]
    async fn invalid_loader_network_settings_fail_before_any_discovery_or_fallback() {
        let settings = crate::DownloadSettings {
            proxy_mode: download_engine::ProxyMode::Http,
            proxy_url: "http://user:secret@proxy.test".into(),
            ..Default::default()
        };
        let expected = settings
            .to_engine_settings()
            .validate_network()
            .unwrap_err()
            .to_string();
        for kind in [
            LoaderKind::Fabric,
            LoaderKind::Quilt,
            LoaderKind::Forge,
            LoaderKind::NeoForge,
        ] {
            assert_eq!(
                list_loader_versions_with_settings(kind, "1.21.4", &settings)
                    .await
                    .unwrap_err(),
                expected
            );
        }
        assert!(mirror_client(&settings.to_engine_settings()).is_err());
    }

    #[tokio::test]
    async fn loader_clients_keep_https_only_after_applying_network_settings() {
        let settings = download_engine::DownloadSettings::default();
        for client in [
            metadata_client(&settings).unwrap(),
            mirror_client(&settings).unwrap(),
        ] {
            let error = client
                .get("http://127.0.0.1:9/metadata")
                .send()
                .await
                .unwrap_err();
            assert!(error.is_builder());
        }
    }

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
        assert!(publish_profile(&directory, b"next", true, || Err("store".into())).is_err());
        assert_eq!(
            std::fs::read(directory.join("launch.json")).unwrap(),
            b"repaired"
        );
        assert_eq!(std::fs::read_dir(&directory).unwrap().count(), 1);
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
