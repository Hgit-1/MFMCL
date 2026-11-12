//! CurseForge 在线模组目录：搜索、版本列表、校验安装。
//!
//! ## 为什么 key 不在这个模块里
//!
//! CurseForge 的 API 要求每个开发者自己的 `x-api-key`。这个 key 不能编译进
//! 客户端——那等于所有用户共用一个凭据，配额和身份都会混在一起。因此 key 由
//! 用户提供、存在操作系统的凭据库里（见 `src-tauri/src/curseforge.rs`），本模块
//! 只在每次请求时接收它，从不落盘、不写日志、不进错误信息。
//!
//! ## 与 Modrinth 实现的差别
//!
//! - 认证：每个请求都要带 `x-api-key`，缺失或无效时服务端返回 403 且**响应体为空**，
//!   所以只能报「key 无效」，不能指望服务端给出原因。
//! - 分页：`pageSize` 上限 50，且 `index + pageSize` 不能超过 10000，越界直接 400。
//! - 下载地址：`downloadUrl` 可能为 `null`（作者禁止第三方分发）。这类文件必须
//!   跳过并如实告知，不能重试，也不能换成别的镜像。
//! - 校验值：`hashes` 里 algo 1 是 SHA-1，algo 2 是 MD5。只接受 SHA-1，与现有
//!   下载引擎的校验路径一致。

use crate::{InstanceRecord, catalog::catalog_client};
use download_engine::{ArtifactSpec, DownloadEngine};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::time::Duration;

const API: &str = "https://api.curseforge.com";
/// MCIM 的 CurseForge 代理前缀。
///
/// ## 为什么需要它
///
/// CurseForge 的公开 API 要求每个客户端自带 `x-api-key`，而 key 要用户自己去
/// 开发者控制台申请。对只想装个模组的玩家来说，这是一道纯行政门槛：他既不知道
/// 什么是 API key，也不理解为什么「装模组」需要去注册一个开发者账号。
///
/// PCL2 / HMCL 等启动器的做法是提供一个公共中转，让客户端在**没有用户 key**
/// 时也能查询元数据。这里沿用同一思路，指向 MCIM（`mod.mcimirror.top`）。
///
/// ## 边界（很重要）
///
/// - **只代理元数据**：搜索、版本列表、批量文件、指纹反查。文件下载仍走
///   `edge.forgecdn.net` 官方 CDN，由 API 返回的 `downloadUrl` 决定，镜像不参与。
/// - **绝不转发用户凭据**。实测给该代理附带一个无效 `x-api-key` 会返回 500
///   （而不是忽略），所以有 key 时必须直连官方，不能「顺手也发一份」。
/// - 用户自己配了 key 就优先走官方：那样配额算在他自己头上，也不依赖第三方可用性。
const MCIM_API: &str = "https://mod.mcimirror.top/curseforge";
/// Minecraft 在 CurseForge 的 gameId。
const GAME_ID: u32 = 432;
const MAX_QUERY: usize = 128;
const MAX_RESULTS: u32 = 20;
const MAX_RESPONSE: usize = 4 * 1024 * 1024;
/// 指纹与工程接口的响应上限。
///
/// 比普通列表大得多：`exactMatches` 的每一项都内嵌完整的 `latestFiles`
/// （几十个文件对象），100 个匹配就能到几 MB。用 `MAX_RESPONSE` 会让大批量
/// 识别在没有明显原因的情况下失败。
const MATCH_RESPONSE: usize = 16 * 1024 * 1024;
/// 服务端硬上限：`index + pageSize <= 10000`。
const MAX_PAGE_WINDOW: u32 = 10_000;

/// Keeps HTTPS and no-redirect enforcement when a request carries `x-api-key`.
pub struct MetadataClient(reqwest::Client);

pub fn metadata_client_with_settings(
    settings: &crate::DownloadSettings,
) -> Result<MetadataClient, String> {
    settings
        .to_engine_settings()
        .configure_metadata_client_with_post_paths(
            reqwest::Client::builder()
                .https_only(true)
                .redirect(reqwest::redirect::Policy::none())
                .user_agent(concat!("MFMCL/", env!("CARGO_PKG_VERSION"))),
            &["api.curseforge.com", "mod.mcimirror.top"],
            &[
                "/v1/mods/files",
                "/v1/mods",
                "/v1/fingerprints/432",
                "/curseforge/v1/mods/files",
                "/curseforge/v1/mods",
                "/curseforge/v1/fingerprints/432",
            ],
        )
        .map_err(|e| e.to_string())?
        .build()
        .map(MetadataClient)
        .map_err(|_| "无法初始化 CurseForge 元数据客户端。".into())
}

/// 一次 CurseForge 请求要用哪个入口。
///
/// ## 为什么要有这个类型
///
/// 之前每个接口都写着 `key: &str` 并在开头 `validate_key`，于是「没有 key」只能
/// 表达成错误。接入镜像后，没有 key 是**正常状态**——它只决定走哪个入口，
/// 不决定能不能查。把这个区分放进类型里，调用方就无法再写出「无 key 即失败」
/// 那种分支。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source<'a> {
    /// 用户提供了 key：直连官方，带上 `x-api-key`。
    Official(&'a str),
    /// 没有 key：走 MCIM 代理，不带任何凭据。
    ///
    /// 用一个不含字符串的变体是刻意的——它让「把用户的 key 发给第三方」在类型
    /// 层面就写不出来。
    Mirror,
}

impl<'a> Source<'a> {
    /// 由可选的 key 决定入口。空串与纯空格等同于「没有 key」。
    pub fn from_key(key: Option<&'a str>) -> Self {
        match key.map(str::trim).filter(|value| !value.is_empty()) {
            Some(value) => Source::Official(value),
            None => Source::Mirror,
        }
    }

    /// 目标地址前缀。
    fn base(self) -> &'static str {
        match self {
            Source::Official(_) => API,
            Source::Mirror => MCIM_API,
        }
    }

    /// 只有直连官方时才返回凭据；镜像分支永远不带 key。
    fn api_key(self) -> Option<&'a str> {
        match self {
            Source::Official(key) => Some(key),
            Source::Mirror => None,
        }
    }
}

/// CurseForge 的 classId（分类）。
fn class_id(kind: ContentKind) -> u32 {
    match kind {
        ContentKind::Mod => 6,
        ContentKind::Modpack => 4471,
        ContentKind::ResourcePack => 12,
        ContentKind::World => 17,
    }
}

/// 目录内容类型。名称与前端 `StoreCategory` 对齐。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ContentKind {
    Mod,
    Modpack,
    ResourcePack,
    World,
}

/// CurseForge 的加载器枚举。与 Modrinth 的字符串名不同，这里是数字。
fn mod_loader_id(loader: &str) -> Option<u32> {
    match loader {
        "forge" => Some(1),
        "fabric" => Some(4),
        "quilt" => Some(5),
        "neoforge" => Some(6),
        _ => None,
    }
}

/// 搜索结果里的一条。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CurseForgeMod {
    pub project_id: String,
    pub title: String,
    pub description: String,
    pub author: String,
    pub downloads: u64,
    /// CurseForge 不提供关注数，界面对这个源要隐藏该字段。
    pub follows: Option<u64>,
    pub icon_url: Option<String>,
}

/// 可安装的一个文件版本。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CurseForgeVersion {
    pub version_id: String,
    pub display_name: String,
    pub date_created: String,
    pub file_name: String,
    /// 作者禁止第三方分发时为 None。界面必须据此显示「需手动下载」。
    pub url: Option<String>,
    pub sha1: Option<String>,
    pub size: u64,
    pub game_versions: Vec<String>,
    pub loaders: Vec<String>,
}

/// 指纹匹配到的一个文件，连同它所属的工程 ID。
///
/// 必须把 `project_id` 带出来：指纹接口只给出「这个文件属于哪个工程」的数字 ID，
/// 而界面上要显示的是工程名与页面地址，那需要再查一次工程信息。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CurseForgeFileMatch {
    pub project_id: String,
    pub file: CurseForgeVersion,
}

/// CurseForge `latestFilesIndexes` 里的一条。
///
/// CurseForge 不会为每个「游戏版本 × 加载器」组合单独返回最新文件，而是给一个
/// 平铺的索引表，调用方自己按目标版本与加载器挑。PCL2 的
/// `LocalResourceLoaders.vb` 同样是先筛这张表、再按 fileId 批量取详情。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CurseForgeFileIndex {
    pub game_version: String,
    pub file_id: String,
    pub file_name: String,
    /// CurseForge 的 `ModLoaderType`：0 = 任意，1 = Forge，4 = Fabric，5 = Quilt，6 = NeoForge。
    pub mod_loader: u32,
}

/// CurseForge 工程（模组主页）的展示信息。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CurseForgeProject {
    pub project_id: String,
    pub slug: String,
    pub title: String,
    pub summary: String,
    /// 工程主页地址。由 slug 拼出，slug 不可用时退回旧式的 `/projects/{id}`。
    pub url: String,
    /// 每个「游戏版本 × 加载器」组合对应的最新文件。用于判断有没有新版本。
    pub latest_files: Vec<CurseForgeFileIndex>,
}

/// 本地文件与 CurseForge 工程的对应关系，直接给界面用。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct IdentifiedMod {
    /// 本地文件名（含 `.jar.disabled` 后缀）。
    pub file_name: String,
    pub project_id: String,
    pub project_title: String,
    pub project_url: String,
    /// 这个本地文件在 CurseForge 上对应的版本。
    pub version: CurseForgeVersion,
    /// 同一工程针对此实例的版本与加载器更新的文件；没有则 `None`。
    ///
    /// 只做展示，不在这里下载：更新要替换现有文件并保留回滚副本，那套流程
    /// 属于 `mod_updates`，重复实现只会让两条路径的失败处理各自漂移。
    pub latest: Option<CurseForgeVersion>,
}

/// CurseForge CDN 的备用主机名。
///
/// ## 为什么需要
///
/// CurseForge 把文件分布在多个 CDN 主机上（`edge` / `mediafilez` / `media`），
/// 还有历史上的 `*-service.overwolf.wtf`。同一个文件的下载地址在 API 里只给一个，
/// 而那个主机偶尔会失败——DNS、区域限制、或单纯的瞬时故障。此时把主机名换成
/// 另一个通常就能成功，因为路径规则完全相同（`/files/<前4位>/<其余>/<文件名>`）。
///
/// PCL2 用同一套思路：`ParseCurseForgeDownloadUrls` 在几种主机名之间做替换，
/// 去重后逐个尝试。
///
/// ## 只做主机替换
///
/// 不构造新路径、不猜文件名：那会越过「作者是否允许分发」的边界。这里只把
/// **同一个已知地址**在等价主机之间切换。
pub fn cdn_alternates(url: &str) -> Vec<String> {
    const HOSTS: [&str; 3] = [
        "edge.forgecdn.net",
        "mediafilez.forgecdn.net",
        "media.forgecdn.net",
    ];
    let Ok(parsed) = reqwest::Url::parse(url) else {
        return vec![url.to_owned()];
    };
    let Some(host) = parsed.host_str() else {
        return vec![url.to_owned()];
    };
    /*
     * 只处理已知的 CDN 主机。
     * 其它域名原样返回：这些替换规则对它们没有意义，改主机名只会得到 404。
     */
    if !HOSTS.contains(&host) {
        return vec![url.to_owned()];
    }
    let mut out: Vec<String> = Vec::new();
    // 原地址排第一：它能用的时候不必多试。
    out.push(url.to_owned());
    for candidate in HOSTS {
        if candidate == host {
            continue;
        }
        let mut next = parsed.clone();
        if next.set_host(Some(candidate)).is_ok() {
            let text = next.as_str().to_owned();
            if !out.contains(&text) {
                out.push(text);
            }
        }
    }
    out
}

/// 按指纹批量识别本地文件属于哪个 CurseForge 工程。
///
/// ## 为什么这条路径不可替代
///
/// 整合包里的 jar 常被改名（`jei-1.20.1-15.2.0.27.jar`、`JustEnoughItems.jar`、
/// 作者自己起的名），按文件名匹配必然失败。指纹由内容决定，改名不影响。
///
/// CurseForge 提供 `POST /v1/fingerprints/432`（432 是 Minecraft 的 gameId）：
/// 提交一批指纹，换回 `exactMatches`。一次最多 200 个指纹——与 `files_by_ids`
/// 同一个量级，所以这里也分片。
///
/// 返回 `(指纹 → 匹配)`。服务端不认识的指纹直接不在结果里：那是「不是 CurseForge
/// 上的文件」，调用方应当按普通本地模组处理，而不是报错。
///
/// 只取 `exactMatches`：`partialMatches` 是同一工程的**不同文件**（通常是小一号的
/// 变体），把它当作「这个文件」会让更新检查下载到错误的内容。
pub async fn files_by_fingerprints(
    source: Source<'_>,
    fingerprints: &[u32],
) -> Result<std::collections::HashMap<u32, CurseForgeFileMatch>, String> {
    let client = metadata_client_with_settings(&crate::DownloadSettings::default())?;
    files_by_fingerprints_with_client(&client, source, fingerprints).await
}

pub async fn files_by_fingerprints_with_client(
    client: &MetadataClient,
    source: Source<'_>,
    fingerprints: &[u32],
) -> Result<std::collections::HashMap<u32, CurseForgeFileMatch>, String> {
    if fingerprints.is_empty() {
        return Ok(std::collections::HashMap::new());
    }
    const BATCH: usize = 200;
    let mut out = std::collections::HashMap::new();
    for chunk in fingerprints.chunks(BATCH) {
        let body = serde_json::json!({ "fingerprints": chunk });
        let bytes = post_json(
            &client.0,
            source,
            format!("{}/v1/fingerprints/{GAME_ID}", source.base()),
            &body,
            MATCH_RESPONSE,
        )
        .await?;
        for (fingerprint, matched) in parse_fingerprint_matches(&bytes)? {
            out.insert(fingerprint, matched);
        }
    }
    Ok(out)
}

/// 解析指纹接口的响应。
///
/// 响应形状：`{ "data": { "exactMatches": [ { "id": <fileId>, "file": {...} } ] } }`。
///
/// 两个字段都要用：
///   - `file.modId` 是工程 ID，界面要靠它查工程名与页面地址；
///   - `file.fileFingerprint` 是我们提交的那个指纹，用来把结果映射回本地文件。
///
/// 不用 `parse_files` 复用：那个函数解析的是裸文件数组，而这里要额外取
/// `modId`、且 `downloadUrl` 可能缺失（作者禁止分发），强行走同一条路径只会
/// 增加绕行代码。
fn parse_fingerprint_matches(bytes: &[u8]) -> Result<Vec<(u32, CurseForgeFileMatch)>, String> {
    #[derive(Deserialize)]
    struct Envelope {
        data: Data,
    }
    #[derive(Deserialize)]
    struct Data {
        #[serde(default, rename = "exactMatches")]
        exact_matches: Vec<Match>,
    }
    #[derive(Deserialize)]
    struct Match {
        file: File,
    }
    #[derive(Deserialize)]
    struct File {
        #[serde(default, rename = "modId")]
        mod_id: u64,
        #[serde(default)]
        id: u64,
        #[serde(default, rename = "displayName")]
        display_name: String,
        #[serde(default, rename = "fileName")]
        file_name: String,
        #[serde(default, rename = "fileDate")]
        file_date: String,
        #[serde(default, rename = "fileLength")]
        file_length: u64,
        #[serde(default, rename = "downloadUrl")]
        download_url: Option<String>,
        #[serde(default)]
        hashes: Vec<Hash>,
        #[serde(default, rename = "gameVersions")]
        game_versions: Vec<String>,
        #[serde(default, rename = "fileFingerprint")]
        fingerprint: u64,
    }
    #[derive(Deserialize)]
    struct Hash {
        value: String,
        algo: u32,
    }

    let parsed: Envelope =
        serde_json::from_slice(bytes).map_err(|_| "CurseForge 指纹响应格式无效。")?;
    if parsed.data.exact_matches.len() > MAX_RESULTS as usize * 10 {
        return Err("CurseForge 指纹匹配数量异常。".into());
    }

    let mut out = Vec::new();
    for item in parsed.data.exact_matches {
        let file = item.file;
        /*
         * 没有工程 ID 的匹配直接跳过。
         *
         * 这种条目只能显示一个孤立文件名——点不到工程主页，也无法参与更新检查；
         * 而 `match.id` 是**文件** ID 而不是工程 ID，拿它顶替会拼出 404 的页面
         * 地址，比不显示更糟。
         */
        if file.mod_id == 0 || file.file_name.trim().is_empty() {
            continue;
        }
        let project_id = file.mod_id;
        let sha1 = file
            .hashes
            .iter()
            .find(|hash| hash.algo == 1)
            .filter(|hash| {
                hash.value.len() == 40 && hash.value.bytes().all(|b| b.is_ascii_hexdigit())
            })
            .map(|hash| hash.value.clone());
        // 与 `parse_files` 相同的域名白名单：只接受 CurseForge 官方分发主机。
        let url = file.download_url.filter(|url| {
            reqwest::Url::parse(url).is_ok_and(|parsed| {
                parsed.scheme() == "https"
                    && matches!(
                        parsed.host_str(),
                        Some("edge.forgecdn.net") | Some("mediafilez.forgecdn.net")
                    )
            })
        });
        let (game_versions, loaders) = split_versions_and_loaders(&file.game_versions);
        out.push((
            file.fingerprint as u32,
            CurseForgeFileMatch {
                project_id: project_id.to_string(),
                file: CurseForgeVersion {
                    version_id: file.id.to_string(),
                    display_name: truncate(&file.display_name, 256),
                    date_created: file.file_date,
                    file_name: truncate(&file.file_name, 256),
                    url,
                    sha1,
                    size: file.file_length,
                    game_versions,
                    loaders,
                },
            },
        ));
    }
    Ok(out)
}

/// 把「本地文件名 → 指纹」解析成带工程信息与可用更新提示的识别结果。
///
/// ## 为什么是三步，而不是一步
///
/// 指纹接口只回答「这个文件的工程 ID 是多少」，不返回工程名。界面上要显示的
/// 是「JourneyMap」而不是「32274」，还要能点进主页——所以必须再查一次工程。
/// PCL2 的 `LocalResourceLoaders.vb` 走的正是这条链路：
///
/// 1. `POST /v1/fingerprints/432` → `exactMatches[].file.modId`；
/// 2. `POST /v1/mods` → 工程名、slug、`latestFilesIndexes`；
/// 3. 按目标游戏版本与加载器从索引里挑出最新的那个文件。
///
/// 第 3 步只用来**提示**「有新版本」。真正替换文件走 `mod_updates`：那里有
/// 校验、回滚与任务上报，重复实现只会让两条路径的失败处理各自漂移。
///
/// ## 失败时的取舍
///
/// 第 2 步失败不丢弃第 1 步的结果：此时仍能显示「来自 CurseForge 的工程 #12345」，
/// 总比整片查不到、看起来「一个都没识别出来」要好。
pub async fn identify_files(
    source: Source<'_>,
    hashed: &[(String, u32)],
    game_version: Option<&str>,
    loader: Option<&str>,
) -> Result<Vec<IdentifiedMod>, String> {
    let client = metadata_client_with_settings(&crate::DownloadSettings::default())?;
    identify_files_with_client(&client, source, hashed, game_version, loader).await
}

pub async fn identify_files_with_client(
    client: &MetadataClient,
    source: Source<'_>,
    hashed: &[(String, u32)],
    game_version: Option<&str>,
    loader: Option<&str>,
) -> Result<Vec<IdentifiedMod>, String> {
    if hashed.is_empty() {
        return Ok(Vec::new());
    }
    let fingerprints: Vec<u32> = hashed.iter().map(|(_, fingerprint)| *fingerprint).collect();
    let matched = files_by_fingerprints_with_client(client, source, &fingerprints).await?;
    if matched.is_empty() {
        return Ok(Vec::new());
    }
    /*
     * 同一个工程可能对应多个本地文件（同一模组的副本，或整合包里的重复 jar）。
     * 去重后再查，避免把同一个 ID 提交多遍。
     */
    let mut project_ids: Vec<String> = Vec::new();
    for item in matched.values() {
        if !project_ids.contains(&item.project_id) {
            project_ids.push(item.project_id.clone());
        }
    }
    // 工程信息查不到时退回空表，由下面按「无名称」处理，而不是让整批失败。
    let projects = projects_by_ids_with_client(client, source, &project_ids)
        .await
        .unwrap_or_default();
    /*
     * 把「最新文件」的真实详情批量取回来。
     *
     * 索引里只有 fileId 与文件名，没有校验值——而没有 SHA-1 的文件不能安全下载，
     * 界面也无法判断「这是不是比本地那个更新」。一次批量查，比每个工程各查一次快
     * 一个数量级。
     */
    let mut latest_ids: Vec<u64> = Vec::new();
    for item in matched.values() {
        if let Some(index) = projects
            .get(&item.project_id)
            .and_then(|project| pick_latest(project, game_version, loader))
            && let Ok(id) = index.file_id.parse::<u64>()
            && !latest_ids.contains(&id)
        {
            latest_ids.push(id);
        }
    }
    /*
     * 更新提示是「锦上添花」：这里失败不该让整次识别失败。
     * 识别出「装了哪些」已经是有用的结果，丢掉它会连来源都看不到。
     */
    let latest_files = files_by_ids_with_client(client, source, &latest_ids)
        .await
        .unwrap_or_default();

    let mut out = Vec::new();
    for (file_name, fingerprint) in hashed {
        let Some(item) = matched.get(fingerprint) else {
            continue;
        };
        let project = projects.get(&item.project_id);
        let latest = project
            .and_then(|project| pick_latest(project, game_version, loader))
            .and_then(|index| index.file_id.parse::<u64>().ok())
            .and_then(|id| latest_files.get(&id).cloned())
            /*
             * 「最新文件」就是本地这个文件时不必显示更新：同一个版本显示成
             * 「可更新到 15.74」会让用户以为点了会有变化。
             */
            .filter(|latest| latest.version_id != item.file.version_id);
        out.push(IdentifiedMod {
            file_name: file_name.clone(),
            project_id: item.project_id.clone(),
            project_title: project
                .map(|project| project.title.clone())
                // 名称缺失时给出可辨识的占位：只有 ID 也比空字符串强，
                // 用户至少能据此搜索或判断是不是自己的私有模组。
                .unwrap_or_else(|| format!("工程 #{}", item.project_id)),
            project_url: project
                .map(|project| project.url.clone())
                .unwrap_or_else(|| {
                    format!("https://www.curseforge.com/projects/{}", item.project_id)
                }),
            version: item.file.clone(),
            latest,
        });
    }
    Ok(out)
}

/// 按目标游戏版本与加载器，从工程的 `latestFilesIndexes` 里挑一个文件。
///
/// 规则与 PCL2 一致，只有两条：
///   - 游戏版本必须精确相等。整合包之间的版本差异会让「1.20.1 的模组装进 1.21」
///     直接崩溃，宽松匹配是有害的；
///   - 加载器要么是 `Any`（0），要么与实例一致。`Any` 的文件通常是数据包式的
///     资源，作者标成通用。
///
/// ## 为什么取「第一个匹配」，而不是按日期取最新
///
/// CurseForge 的索引本身就是「每个组合的最新文件」——同一个组合只会出现一次。
/// 按日期再排一次没有意义，而且索引里根本没有日期字段。
fn pick_latest<'a>(
    project: &'a CurseForgeProject,
    game_version: Option<&str>,
    loader: Option<&str>,
) -> Option<&'a CurseForgeFileIndex> {
    let wanted_loader = loader.and_then(mod_loader_id);
    project.latest_files.iter().find(|index| {
        let version_ok = game_version
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .is_none_or(|value| index.game_version.trim() == value);
        /*
         * 没有加载器时不筛加载器（原版实例没有可比的类型）。
         * 有加载器时，`Any` 的文件仍然接受——作者用它标记与加载器无关的内容。
         */
        let loader_ok =
            wanted_loader.is_none_or(|wanted| index.mod_loader == 0 || index.mod_loader == wanted);
        version_ok && loader_ok
    })
}

/// 批量取工程信息（名称、slug、简介），用于把工程 ID 变成可展示的条目。
///
/// `POST /v1/mods` 一次接受一批 `modIds`（实测 200 个可用）。PCL2 也是这么做的：
/// 指纹只给工程 ID，名称与页面地址必须再查一次；逐个 `GET /v1/mods/{id}` 会让
/// 一个 300 模组的整合包发出 300 次请求。
///
/// 返回 `(工程 ID → 信息)`。服务端不返回的 ID 不在结果里（工程被删除或隐藏），
/// 调用方应当退回到「只显示工程 ID」而不是当作错误。
pub async fn projects_by_ids(
    source: Source<'_>,
    project_ids: &[String],
) -> Result<std::collections::HashMap<String, CurseForgeProject>, String> {
    let client = metadata_client_with_settings(&crate::DownloadSettings::default())?;
    projects_by_ids_with_client(&client, source, project_ids).await
}

pub async fn projects_by_ids_with_client(
    client: &MetadataClient,
    source: Source<'_>,
    project_ids: &[String],
) -> Result<std::collections::HashMap<String, CurseForgeProject>, String> {
    if project_ids.is_empty() {
        return Ok(std::collections::HashMap::new());
    }
    /*
     * 只接受纯数字 ID。
     *
     * 这些 ID 会直接拼进 JSON 请求体，非数字值会让整个批次被服务端拒绝（400），
     * 表现为「一个工程信息都取不到」。在本地过滤掉坏数据，好数据仍然能查。
     */
    let mut ids: Vec<u64> = Vec::new();
    for id in project_ids {
        let trimmed = id.trim();
        if trimmed.is_empty() || trimmed.len() > 20 {
            continue;
        }
        if let Ok(value) = trimmed.parse::<u64>()
            && !ids.contains(&value)
        {
            ids.push(value);
        }
    }
    if ids.is_empty() {
        return Ok(std::collections::HashMap::new());
    }

    const BATCH: usize = 200;
    let mut out = std::collections::HashMap::new();
    for chunk in ids.chunks(BATCH) {
        let body = serde_json::json!({ "modIds": chunk });
        let bytes = post_json(
            &client.0,
            source,
            format!("{}/v1/mods", source.base()),
            &body,
            MATCH_RESPONSE,
        )
        .await?;
        out.extend(parse_projects(&bytes)?);
    }
    Ok(out)
}

/// 解析 `POST /v1/mods` 的响应。
fn parse_projects(
    bytes: &[u8],
) -> Result<std::collections::HashMap<String, CurseForgeProject>, String> {
    #[derive(Deserialize)]
    struct Response {
        #[serde(default)]
        data: Vec<Mod>,
    }
    #[derive(Deserialize)]
    struct Mod {
        id: u64,
        #[serde(default)]
        name: String,
        #[serde(default)]
        slug: String,
        #[serde(default)]
        summary: String,
        #[serde(default, rename = "latestFilesIndexes")]
        latest_files_indexes: Vec<FileIndex>,
    }
    #[derive(Deserialize)]
    struct FileIndex {
        #[serde(default, rename = "gameVersion")]
        game_version: String,
        #[serde(default, rename = "fileId")]
        file_id: u64,
        #[serde(default)]
        filename: String,
        #[serde(default, rename = "modLoader")]
        mod_loader: u32,
    }

    let parsed: Response =
        serde_json::from_slice(bytes).map_err(|_| "CurseForge 工程信息格式无效。")?;
    if parsed.data.len() > MAX_RESULTS as usize * 10 {
        return Err("CurseForge 工程数量异常。".into());
    }

    let mut out = std::collections::HashMap::new();
    for item in parsed.data {
        let title = item.name.trim();
        if title.is_empty() || title.chars().count() > 256 {
            continue;
        }
        /*
         * slug 只接受安全字符。
         *
         * 它会被拼进展示给用户的页面地址。带 `/`、`?`、`#` 或空格的 slug 会
         * 拼出另一个路径（甚至指向别的站点区域），所以只放行 CurseForge 实际
         * 使用的字符集。
         */
        let slug = item.slug.trim();
        let slug_ok = !slug.is_empty()
            && slug.len() <= 128
            && slug
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
        let url = if slug_ok {
            format!("https://www.curseforge.com/minecraft/mc-mods/{slug}")
        } else {
            // slug 不可用时退回旧式的工程地址：它只含数字，永远安全。
            format!("https://www.curseforge.com/projects/{}", item.id)
        };
        let latest_files = item
            .latest_files_indexes
            .into_iter()
            .filter(|index| {
                index.file_id != 0
                    && !index.filename.trim().is_empty()
                    && index.filename.len() <= 256
                    && !index.game_version.trim().is_empty()
            })
            // 上限防止畸形响应把内存撑爆：真实的模组文件索引最多几百条。
            .take(2048)
            .map(|index| CurseForgeFileIndex {
                game_version: index.game_version,
                file_id: index.file_id.to_string(),
                file_name: index.filename,
                mod_loader: index.mod_loader,
            })
            .collect();
        out.insert(
            item.id.to_string(),
            CurseForgeProject {
                project_id: item.id.to_string(),
                slug: slug.to_owned(),
                title: truncate(title, 256),
                summary: truncate(item.summary.trim(), 512),
                url,
                latest_files,
            },
        );
    }
    Ok(out)
}

fn validate_key(key: &str) -> Result<(), String> {
    // key 是 bcrypt 形态的不透明字符串（60 字符）。只做长度与字符集检查，
    // 不尝试解析——格式一旦变化就会误判。真正的验证是发一次请求。
    let trimmed = key.trim();
    if trimmed.is_empty() {
        return Err("尚未设置 CurseForge API Key。".into());
    }
    if trimmed.len() > 128 || !trimmed.bytes().all(|b| b.is_ascii_graphic()) {
        return Err("CurseForge API Key 格式无效。".into());
    }
    Ok(())
}

async fn bounded(mut response: reqwest::Response, limit: usize) -> Result<Vec<u8>, String> {
    if response
        .content_length()
        .is_some_and(|length| length as usize > limit)
    {
        return Err("CurseForge 响应过大。".into());
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| "CurseForge 读取失败。")?
    {
        if bytes.len().saturating_add(chunk.len()) > limit {
            return Err("CurseForge 响应过大。".into());
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

/// 把 HTTP 状态翻成能指导用户下一步的错误。
///
/// 403 是 CurseForge 官方唯一会用的认证失败码，且响应体为空——missing key 与
/// wrong key 无法区分，所以文案必须同时覆盖两种情况。走镜像时同一个码更可能
/// 是「代理侧的 key 配额用完了」，文案要指向不同的下一步。
fn status_error(source: Source<'_>, status: reqwest::StatusCode) -> String {
    match status.as_u16() {
        400 => "CurseForge 拒绝了请求参数（分页或筛选超出允许范围）。".into(),
        401 | 403 => match source {
            Source::Official(_) => "CurseForge API Key 缺失或无效，请在内容商店里重新填写。".into(),
            // 镜像侧配额耗尽与「你的 key 有问题」是两件事，混成一句会让人去改
            // 一个根本没问题的设置。
            Source::Mirror => {
                "CurseForge 镜像当前不可用（配额或限流）；稍后再试，或在设置里填入自己的 API Key。"
                    .into()
            }
        },
        404 => "CurseForge 上找不到该项目。".into(),
        429 => "CurseForge 限流中，请稍后再试。".into(),
        _ => format!("CurseForge 请求失败（HTTP {}）。", status.as_u16()),
    }
}

/// 发一个 GET 并返回原始响应体。
///
/// 凭据只在这里附加：`Source` 决定要不要带 `x-api-key`，调用方无法绕过。
async fn get_json(
    client: &reqwest::Client,
    source: Source<'_>,
    url: reqwest::Url,
) -> Result<Vec<u8>, String> {
    let mut request = client.get(url).header("Accept", "application/json");
    if let Some(key) = source.api_key() {
        validate_key(key)?;
        request = request.header("x-api-key", key);
    }
    let response = request.send().await.map_err(|_| connect_error(source))?;
    if !response.status().is_success() {
        return Err(status_error(source, response.status()));
    }
    bounded(response, MAX_RESPONSE).await
}

/// 发一个 POST（JSON 请求体）并返回原始响应体。
///
/// 与 `get_json` 共用凭据附加规则：镜像分支永远不带 key。
async fn post_json(
    client: &reqwest::Client,
    source: Source<'_>,
    url: String,
    body: &serde_json::Value,
    limit: usize,
) -> Result<Vec<u8>, String> {
    let mut request = client
        .post(url)
        .header("Accept", "application/json")
        .json(body);
    if let Some(key) = source.api_key() {
        validate_key(key)?;
        request = request.header("x-api-key", key);
    }
    let response = request.send().await.map_err(|_| connect_error(source))?;
    if !response.status().is_success() {
        return Err(status_error(source, response.status()));
    }
    bounded(response, limit).await
}

/// 连不上时区分两种入口：镜像不通与官方不通，用户能做的事不一样。
fn connect_error(source: Source<'_>) -> String {
    match source {
        Source::Official(_) => "无法连接 CurseForge。".into(),
        Source::Mirror => {
            "无法连接 CurseForge 镜像；可以在设置里填入自己的 API Key 直连官方。".into()
        }
    }
}

/// 批量按 fileID 查询文件详情，用于 CurseForge 格式整合包的导入。
///
/// CurseForge 的 `manifest.json` 只给 `projectID`/`fileID`，不含下载地址与哈希。
/// 官方提供 `POST /v1/mods/files` 一次查一批（实测 200 个 ID 可用），比逐个请求
/// 快两个数量级——一个中等整合包有 200+ 个文件，逐个查会慢到不可用。
///
/// 返回 `(fileID → 详情)`。调用方按清单里的顺序取用，缺失的 ID 说明服务端不认识
/// 或已被删除，由调用方决定是跳过还是报错。
pub async fn files_by_ids(
    source: Source<'_>,
    file_ids: &[u64],
) -> Result<std::collections::HashMap<u64, CurseForgeVersion>, String> {
    let client = metadata_client_with_settings(&crate::DownloadSettings::default())?;
    files_by_ids_with_client(&client, source, file_ids).await
}

pub async fn files_by_ids_with_client(
    client: &MetadataClient,
    source: Source<'_>,
    file_ids: &[u64],
) -> Result<std::collections::HashMap<u64, CurseForgeVersion>, String> {
    if file_ids.is_empty() {
        return Ok(std::collections::HashMap::new());
    }
    // 服务端上限是 200 个一批；调用方可能给更多，这里分片以免整单失败。
    const BATCH: usize = 200;
    let mut out = std::collections::HashMap::new();
    for chunk in file_ids.chunks(BATCH) {
        let body = serde_json::json!({ "fileIds": chunk });
        let bytes = post_json(
            &client.0,
            source,
            format!("{}/v1/mods/files", source.base()),
            &body,
            MAX_RESPONSE,
        )
        .await?;
        for version in parse_files(&bytes)? {
            if let Ok(id) = version.version_id.parse::<u64>() {
                out.insert(id, version);
            }
        }
    }
    Ok(out)
}

pub async fn pack_files_with_client(
    client: &MetadataClient,
    source: Source<'_>,
    file_ids: &[u64],
) -> Result<std::collections::HashMap<u64, crate::modpacks::CurseForgePackFile>, String> {
    #[derive(Deserialize)]
    struct Response {
        data: Vec<crate::modpacks::CurseForgePackFile>,
    }
    let mut out = std::collections::HashMap::new();
    for chunk in file_ids.chunks(200) {
        let bytes = post_json(
            &client.0,
            source,
            format!("{}/v1/mods/files", source.base()),
            &serde_json::json!({ "fileIds": chunk }),
            MAX_RESPONSE,
        )
        .await?;
        let response: Response =
            serde_json::from_slice(&bytes).map_err(|_| "CurseForge 文件详情格式错误。")?;
        for file in response.data {
            if !chunk.contains(&file.id) || out.insert(file.id, file).is_some() {
                return Err("CurseForge 文件详情标识重复或不匹配。".into());
            }
        }
    }
    Ok(out)
}

/// 搜索指定类型的内容。
///
/// `game_version` 与 `loader` 同时给出时按两者收窄；只给版本时按版本收窄。
/// 这与官方文档的要求一致：`modLoaderType` 必须与 `gameVersion` 搭配使用。
pub async fn search(
    source: Source<'_>,
    kind: ContentKind,
    query: &str,
    game_version: Option<&str>,
    loader: Option<&str>,
    index: u32,
    page_size: u32,
) -> Result<Vec<CurseForgeMod>, String> {
    let client = metadata_client_with_settings(&crate::DownloadSettings::default())?;
    search_with_client(
        &client,
        source,
        kind,
        query,
        game_version,
        loader,
        index,
        page_size,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
pub async fn search_with_client(
    client: &MetadataClient,
    source: Source<'_>,
    kind: ContentKind,
    query: &str,
    game_version: Option<&str>,
    loader: Option<&str>,
    index: u32,
    page_size: u32,
) -> Result<Vec<CurseForgeMod>, String> {
    let query = query.trim();
    // 空关键词 = 浏览模式：不传 searchFilter，配合 sortField=6 得到按总下载量排序的列表。
    // 超过长度上限才是有问题的输入。
    if query.chars().count() > MAX_QUERY {
        return Err("搜索词不能超过 128 个字符。".into());
    }
    let page_size = page_size.clamp(1, MAX_RESULTS);
    if index.saturating_add(page_size) > MAX_PAGE_WINDOW {
        return Err("已经翻到 CurseForge 允许的最深分页。".into());
    }

    let mut params: Vec<(String, String)> = vec![
        ("gameId".into(), GAME_ID.to_string()),
        ("classId".into(), class_id(kind).to_string()),
        ("index".into(), index.to_string()),
        ("pageSize".into(), page_size.to_string()),
        /*
         * 必须显式指定排序。
         *
         * 实测：不传 `searchFilter` 时服务端默认按名称字母序返回，不是按热度；
         * 「浏览该版本最受欢迎的模组」只能靠 `sortField=6`（总下载量）实现。
         * 带关键词时同样需要它——默认排序下 `searchFilter=sodium` 的第一条并不是
         * Sodium 本身。
         *
         * 注意服务端排序是**近似**降序（实测约 1.9% 的相邻对逆序），所以调用方
         * 需要对拿到的这一页再按 `downloads` 排一次，不要把它当权威排行榜。
         */
        ("sortField".into(), "6".into()), // 6 = total downloads
        ("sortOrder".into(), "desc".into()),
    ];
    push_search_filters(&mut params, query, game_version, loader);

    let url =
        reqwest::Url::parse_with_params(&format!("{}/v1/mods/search", source.base()), &params)
            .map_err(|_| "搜索地址无效。")?;
    let bytes = get_json(&client.0, source, url).await?;
    parse_search(&bytes)
}

/// 追加关键词与版本/加载器筛选。
///
/// 抽成独立函数是为了能被单元测试固定住：浏览模式（空关键词）**不能**发送
/// `searchFilter`，否则服务端会按字面空串处理语义变化，而这条约束只靠读代码
/// 很容易在后续重构中被破坏。
fn push_search_filters(
    params: &mut Vec<(String, String)>,
    query: &str,
    game_version: Option<&str>,
    loader: Option<&str>,
) {
    // 浏览模式不传 searchFilter：空串与不传等价，但少发一个参数更明确。
    if !query.is_empty() {
        params.push(("searchFilter".into(), query.to_owned()));
    }
    if let Some(version) = game_version.map(str::trim).filter(|v| !v.is_empty()) {
        params.push(("gameVersion".into(), version.to_owned()));
        if let Some(loader_id) = loader.and_then(mod_loader_id) {
            params.push(("modLoaderType".into(), loader_id.to_string()));
        }
    }
}

fn parse_search(bytes: &[u8]) -> Result<Vec<CurseForgeMod>, String> {
    #[derive(Deserialize)]
    struct Logo {
        #[serde(default, rename = "thumbnailUrl")]
        thumbnail_url: Option<String>,
        #[serde(default)]
        url: Option<String>,
    }
    #[derive(Deserialize)]
    struct Author {
        #[serde(default)]
        name: Option<String>,
    }
    #[derive(Deserialize)]
    struct Mod {
        id: u64,
        name: String,
        #[serde(default)]
        summary: Option<String>,
        #[serde(default, rename = "downloadCount")]
        download_count: Option<u64>,
        #[serde(default)]
        logo: Option<Logo>,
        #[serde(default)]
        authors: Vec<Author>,
    }
    #[derive(Deserialize)]
    struct Response {
        #[serde(default)]
        data: Vec<Mod>,
    }

    let parsed: Response =
        serde_json::from_slice(bytes).map_err(|_| "CurseForge 搜索结果无效。")?;
    if parsed.data.len() > MAX_RESULTS as usize {
        return Err("CurseForge 搜索结果数量异常。".into());
    }
    // 单条畸形结果跳过而不是整批失败：浏览体验不能被一个坏条目打断。
    Ok(parsed
        .data
        .into_iter()
        .filter(|item| !item.name.trim().is_empty() && item.name.chars().count() <= 256)
        .map(|item| CurseForgeMod {
            project_id: item.id.to_string(),
            title: item.name,
            description: truncate(item.summary.as_deref().unwrap_or(""), 512),
            author: truncate(
                item.authors
                    .first()
                    .and_then(|author| author.name.as_deref())
                    .unwrap_or(""),
                128,
            ),
            downloads: item.download_count.unwrap_or(0),
            follows: None,
            icon_url: item
                .logo
                .and_then(|logo| logo.thumbnail_url.or(logo.url))
                .filter(|url| {
                    reqwest::Url::parse(url).is_ok_and(|parsed| parsed.scheme() == "https")
                }),
        })
        .collect())
}

fn truncate(value: &str, limit: usize) -> String {
    value.chars().take(limit).collect()
}

/// 读取某个项目可用于此实例的文件列表。
///
/// 与 Modrinth 一样按实例的游戏版本与加载器收窄；没有加载器时（原版实例）只按
/// 游戏版本筛。返回顺序沿用服务端（通常是新的在前）。
pub async fn files(
    source: Source<'_>,
    project_id: &str,
    game_version: &str,
    loader: Option<&str>,
) -> Result<Vec<CurseForgeVersion>, String> {
    let client = metadata_client_with_settings(&crate::DownloadSettings::default())?;
    files_with_client(&client, source, project_id, game_version, loader).await
}

pub async fn files_with_client(
    client: &MetadataClient,
    source: Source<'_>,
    project_id: &str,
    game_version: &str,
    loader: Option<&str>,
) -> Result<Vec<CurseForgeVersion>, String> {
    if project_id.is_empty()
        || project_id.len() > 20
        || !project_id.bytes().all(|b| b.is_ascii_digit())
    {
        return Err("项目标识无效。".into());
    }
    let mut params: Vec<(String, String)> = vec![("pageSize".into(), "50".into())];
    /*
     * 版本号为空就不传 `gameVersion`。
     *
     * 整合包自带它需要的那套版本，调用方没有「目标实例」可参考时会传空串；
     * 发一个 `gameVersion=` 给服务端会被当成筛选「版本号字面为空」，返回空列表，
     * 看起来像是「这个包没有文件」。
     */
    if !game_version.trim().is_empty() {
        params.push(("gameVersion".into(), game_version.trim().to_owned()));
    }
    if let Some(loader_id) = loader.and_then(mod_loader_id) {
        params.push(("modLoaderType".into(), loader_id.to_string()));
    }
    let url = reqwest::Url::parse_with_params(
        &format!("{}/v1/mods/{project_id}/files", source.base()),
        &params,
    )
    .map_err(|_| "版本地址无效。")?;

    let bytes = get_json(&client.0, source, url).await?;
    parse_files(&bytes)
}

fn parse_files(bytes: &[u8]) -> Result<Vec<CurseForgeVersion>, String> {
    #[derive(Deserialize)]
    struct Hash {
        value: String,
        algo: u32,
    }
    #[derive(Deserialize)]
    struct File {
        id: u64,
        #[serde(rename = "displayName")]
        display_name: String,
        #[serde(rename = "fileName")]
        file_name: String,
        #[serde(rename = "fileDate")]
        file_date: String,
        #[serde(default, rename = "fileLength")]
        file_length: u64,
        #[serde(default, rename = "downloadUrl")]
        download_url: Option<String>,
        #[serde(default)]
        hashes: Vec<Hash>,
        #[serde(default, rename = "gameVersions")]
        game_versions: Vec<String>,
    }
    #[derive(Deserialize)]
    struct Response {
        #[serde(default)]
        data: Vec<File>,
    }

    let parsed: Response =
        serde_json::from_slice(bytes).map_err(|_| "CurseForge 版本列表无效。")?;
    if parsed.data.len() > 100 {
        return Err("CurseForge 版本数量异常。".into());
    }
    Ok(parsed
        .data
        .into_iter()
        .filter(|file| !file.file_name.trim().is_empty())
        .map(|file| {
            // 只接受 SHA-1：下载引擎按 SHA-1 校验，混入 MD5 会让校验路径分叉。
            let sha1 = file
                .hashes
                .iter()
                .find(|hash| hash.algo == 1)
                .filter(|hash| {
                    hash.value.len() == 40 && hash.value.bytes().all(|b| b.is_ascii_hexdigit())
                })
                .map(|hash| hash.value.clone());
            // 只接受 CurseForge 官方分发域名，避免被服务端返回的任意地址带偏。
            let url = file.download_url.filter(|url| {
                reqwest::Url::parse(url).is_ok_and(|parsed| {
                    parsed.scheme() == "https"
                        && matches!(
                            parsed.host_str(),
                            Some("edge.forgecdn.net") | Some("mediafilez.forgecdn.net")
                        )
                })
            });
            /*
             * CurseForge 把加载器名混在 `gameVersions` 里一起返回。
             *
             * 例如一个 Fabric 整合包会给出 `["1.20.1", "Fabric"]`，Forge 的则是
             * `["1.20.1", "Forge"]`。直接把它当「游戏版本列表」用会出错：界面如果
             * 在这个数组里取「最大的那个」当分组标题，会得到 "Fabric" 而不是
             * "1.20.1"。所以在这里按形态拆开：形如 x.y(.z) 的是游戏版本，其余
             * 归为加载器（Java 版本号这类既非版本也非加载器的条目直接丢弃）。
             */
            let (game_versions, loaders) = split_versions_and_loaders(&file.game_versions);
            CurseForgeVersion {
                version_id: file.id.to_string(),
                display_name: truncate(&file.display_name, 256),
                date_created: file.file_date,
                file_name: truncate(&file.file_name, 256),
                url,
                sha1,
                size: file.file_length,
                game_versions,
                loaders,
            }
        })
        .collect())
}

/// 把 CurseForge 的 `gameVersions` 拆成「游戏版本」与「加载器」两组。
///
/// CurseForge 的这个字段是个混合数组：`["1.20.1", "Fabric", "Java 17"]`。
/// 界面需要分开用——游戏版本用于筛选与分组标题，加载器用于在版本行上标注。
///
/// 判定规则：
///   - 形如 `1.20` / `1.20.1` 的数字点号串 → 游戏版本；
///   - 已知加载器名（不区分大小写，含 `Fabric`/`Forge`/`NeoForge`/`Quilt`/`Liteloader`）→ 加载器；
///   - 其余（例如 `Java 17`）丢弃：它既不是版本也不能装进实例。
fn split_versions_and_loaders(values: &[String]) -> (Vec<String>, Vec<String>) {
    let mut versions = Vec::new();
    let mut loaders = Vec::new();
    for value in values {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            continue;
        }
        if is_numeric_version(trimmed) {
            if versions.len() < 64 {
                versions.push(trimmed.to_owned());
            }
        } else if let Some(canonical) = canonical_loader_opt(trimmed) {
            if !loaders.iter().any(|existing| existing == canonical) {
                loaders.push(canonical.to_owned());
            }
        }
    }
    (versions, loaders)
}

/// 形如 `1`、`1.20`、`1.20.1` 的点号数字串。快照号（`24w14a`）不算——整合包
/// 极少只针对快照，而把快照当分组标题会让列表碎成一堆一次性分组。
fn is_numeric_version(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 32
        && value.starts_with(|c: char| c.is_ascii_digit())
        && value.chars().all(|c| c.is_ascii_digit() || c == '.')
        && !value.ends_with('.')
}

/// 归一化成界面统一使用的写法，避免同一加载器因大小写不同被当成两个。
///
/// 返回 `&'static str` 而不是借用入参：调用方只在命中时使用它的结果，
/// 所以永远取到的是这里的字面量，不会回落到入参。
fn canonical_loader_opt(value: &str) -> Option<&'static str> {
    if value.eq_ignore_ascii_case("fabric") {
        Some("Fabric")
    } else if value.eq_ignore_ascii_case("quilt") {
        Some("Quilt")
    } else if value.eq_ignore_ascii_case("neoforge") {
        Some("NeoForge")
    } else if value.eq_ignore_ascii_case("forge") {
        Some("Forge")
    } else if value.eq_ignore_ascii_case("liteloader") {
        Some("LiteLoader")
    } else {
        None
    }
}

/// 内容类型 → 安装目录名（相对实例的游戏目录）。
///
/// 四类内容在磁盘上的位置不同，混用会让游戏读不到文件：
///   - mod → `mods/`
///   - resourcepack → `resourcepacks/`（复数，游戏客户端从这里加载）
///   - world → `saves/`（每个存档是子目录，这里放的是地图压缩包）
///   - modpack → 不走这个函数：整合包要创建新实例，由 `modpacks` 模块处理
fn install_dir(kind: ContentKind) -> Option<&'static str> {
    match kind {
        ContentKind::Mod => Some("mods"),
        ContentKind::ResourcePack => Some("resourcepacks"),
        ContentKind::World => Some("saves"),
        // 整合包不是「往实例里放一个文件」，调用方必须走导入流程。
        ContentKind::Modpack => None,
    }
}

/// 内容类型的中文名，用于提示文案。
fn kind_label(kind: ContentKind) -> &'static str {
    match kind {
        ContentKind::Mod => "模组",
        ContentKind::Modpack => "整合包",
        ContentKind::ResourcePack => "资源包",
        ContentKind::World => "地图",
    }
}

/// 安装一个 CurseForge 文件到实例对应目录，按 SHA-1 校验。
///
/// `kind` 决定目标目录（mods / resourcepacks / saves）与临时文件扩展名。之前只支持
/// 模组、并把扩展名硬编码成 `.jar`，接入资源包和地图后会把 `.zip` 写进 `mods/`，
/// 游戏既不会加载也会被当成损坏的模组。
///
/// 前置条件比 Modrinth 严格一成：`sha1` 与 `url` 都必须存在。作者禁止第三方分发
/// 的文件没有 `downloadUrl`，这类必须由用户在浏览器里手动下载，启动器不能代劳。
///
/// 发布方式与 Modrinth 一致：先下到临时文件并校验，再用 `hard_link` 原子发布——
/// `hard_link` 在目标已存在时会失败，因此「绝不覆盖已有文件」是靠文件系统保证的，
/// 而不是靠先检查再写入（后者存在竞态）。
pub async fn install(
    engine: &DownloadEngine,
    instance: &InstanceRecord,
    version: &CurseForgeVersion,
    kind: ContentKind,
) -> Result<String, String> {
    let dir_name = install_dir(kind)
        .ok_or_else(|| "整合包会导入为新的实例，不能用「安装到当前实例」的方式处理。".to_owned())?;
    let label = kind_label(kind);
    let url = version.url.as_deref().ok_or_else(|| {
        format!("该{label}不允许第三方启动器下载，请到 CurseForge 网页手动下载。")
    })?;
    let sha1 = version
        .sha1
        .as_deref()
        .ok_or_else(|| format!("该{label}缺少 SHA-1 校验值，无法安全安装。"))?;
    if !crate::mod_updates::is_safe_mod_name(&version.file_name) {
        return Err("此版本的文件名不适用于安全安装。".into());
    }

    let target_dir = instance.game_dir.join(dir_name);
    download_verified(
        engine,
        kind,
        version.file_name.clone(),
        url,
        sha1,
        version.size,
        Some(version.version_id.as_str()),
        label,
        &target_dir,
        &format!("{dir_name} 目录"),
    )
    .await?;
    Ok(version.file_name.clone())
}

/// 直接把文件下载到用户指定的目录，不装进实例。
///
/// 与 [`install`] 的区别只在落盘位置：校验、原子发布、拒绝覆盖的策略完全相同。
/// 用于「只想拿到 jar / 整合包本身」的场景（例如备份、手动放进别的实例）。
pub async fn download_to(
    engine: &DownloadEngine,
    version: &CurseForgeVersion,
    kind: ContentKind,
    directory: &Path,
) -> Result<String, String> {
    let label = kind_label(kind);
    let url = version.url.as_deref().ok_or_else(|| {
        format!("该{label}不允许第三方启动器下载，请到 CurseForge 网页手动下载。")
    })?;
    let sha1 = version
        .sha1
        .as_deref()
        .ok_or_else(|| format!("该{label}缺少 SHA-1 校验值，无法安全下载。"))?;
    download_verified(
        engine,
        kind,
        version.file_name.clone(),
        url,
        sha1,
        version.size,
        Some(version.version_id.as_str()),
        label,
        directory,
        "此目录",
    )
    .await
}

/// 下载一个 Modrinth 的 `.mrpack` 到指定目录。
///
/// ## 为什么复用 CurseForge 的下载实现
///
/// 「下载 → 校验 SHA-1 → 原子发布」这套逻辑与来源无关，中间涉及断点、临时文件
/// 命名、同名冲突、发布时的 rename 回滚。为 Modrinth 再写一份，等于把这个最容易
/// 出错的部分复制一遍，两份迟早会漂移。
///
/// 唯一的差异是**域名与校验值来源**：Modrinth 的文件在 `cdn.modrinth.com`，校验
/// 值是 SHA-1（与 CurseForge 相同），`CatalogVersion` 里已经带上了。所以这里只做
/// 一层薄适配，校验逻辑完全共用。
pub async fn download_modrinth_pack(
    engine: &DownloadEngine,
    version: &crate::catalog::CatalogVersion,
    directory: &Path,
) -> Result<String, String> {
    /*
     * 只接受 Modrinth 官方分发域名。
     *
     * `parse_versions` 已经校验过一次，但那是「读列表时」的保证；这里再校验一次
     * 是因为这个函数可以从别处调用，而「URL 从哪来」不该由调用方决定。
     */
    let url = reqwest::Url::parse(&version.url).map_err(|_| "下载地址无效。".to_owned())?;
    if url.scheme() != "https" || url.host_str() != Some("cdn.modrinth.com") {
        return Err("文件不来自 Modrinth 官方分发域名。".into());
    }
    if version.sha1.len() != 40 || !version.sha1.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("此整合包缺少有效 SHA-1 校验值，无法安全下载。".into());
    }
    download_verified(
        engine,
        ContentKind::Modpack,
        version.file_name.clone(),
        &version.url,
        &version.sha1,
        version.size,
        Some(version.version_id.as_str()),
        "整合包",
        directory,
        "此目录",
    )
    .await
}

/// 共享的「下载 → 校验 → 原子发布」实现。
///
/// `directory_hint` 只用于错误文案：实例目录叫「mods 目录」，用户自选目录叫「此目录」。
#[allow(clippy::too_many_arguments)]
pub(crate) async fn download_verified(
    engine: &DownloadEngine,
    kind: ContentKind,
    file_name: String,
    url: &str,
    sha1: &str,
    size: u64,
    version_id: Option<&str>,
    label: &str,
    directory: &Path,
    directory_hint: &str,
) -> Result<String, String> {
    /*
     * 扩展名规则按内容类型分派。
     *
     * 模组/资源包/地图落进实例的子目录，`.jar` 与 `.zip` 的语义不同，用 mod 的
     * 规则；整合包是压缩包（CurseForge 用 `.zip`、Modrinth 用 `.mrpack`），
     * 用 `.jar` 的规则会把每一个真实整合包都拒掉。
     */
    let name_ok = if kind == ContentKind::Modpack {
        crate::mod_updates::is_safe_pack_name(&file_name)
    } else {
        crate::mod_updates::is_safe_mod_name(&file_name)
    };
    if !name_ok {
        return Err(format!("此{label}的文件名不适用于安全下载。"));
    }
    std::fs::create_dir_all(directory)
        .map_err(|error| format!("无法创建{directory_hint}：{error}"))?;
    let destination = directory.join(&file_name);
    if std::fs::symlink_metadata(&destination).is_ok() {
        return Err(format!(
            "{directory_hint}中已存在同名文件 {file_name}；如需替换请先删除它。"
        ));
    }
    let extension = std::path::Path::new(&file_name)
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or("bin");
    let temporary = directory.join(format!(
        ".install-{}.{extension}",
        uuid::Uuid::new_v4().simple()
    ));
    /*
     * 按 CDN 备用主机逐个尝试。
     *
     * 只换主机名、不换路径：同一个文件的等价地址，作者是否允许分发与内容都不变。
     * 每个候选都带同一个 SHA-1，所以即便某个主机返回了错误内容，也会被校验拦下，
     * 不会写进游戏目录。第一个成功即止。
     *
     * 不在候选之间清理临时文件：下载引擎的续传状态以校验值为身份，几个镜像
     * 提供的是同一份字节，续传是安全的；若某个主机返回了坏内容，引擎会在校验
     * 失败时自己丢掉分片并重新开始。
     */
    let mut downloaded = false;
    for candidate in cdn_alternates(url) {
        let result = engine
            .download(
                &ArtifactSpec {
                    id: match version_id {
                        Some(id) => format!("curseforge-download:{id}"),
                        None => format!("download:{}", uuid::Uuid::new_v4().simple()),
                    },
                    label: format!("下载{label} {file_name}"),
                    url: candidate,
                    mirror_url: None,
                    destination: temporary.clone(),
                    size: (size > 0).then_some(size),
                    sha1: Some(sha1.to_owned()),
                    sha256: None,
                },
                None,
            )
            .await;
        if result.is_ok() {
            downloaded = true;
            break;
        }
    }
    if !downloaded {
        return Err(format!("{label}下载或校验失败；没有改动任何文件。"));
    }
    // hard_link 会拒绝已存在的目标（含悬空符号链接），因此不会覆盖用户文件。
    if let Err(error) = std::fs::hard_link(&temporary, &destination) {
        let _ = std::fs::remove_file(&temporary);
        return Err(format!("无法写入{label}文件（不会覆盖已有文件）：{error}"));
    }
    std::fs::remove_file(&temporary)
        .map_err(|error| format!("{label}已下载，但清理临时文件失败：{error}"))?;
    Ok(destination.to_string_lossy().into_owned())
}

/// 探测一个 key 是否可用。用于「保存前先验证」，避免存进凭据库一个必然失败的 key。
pub async fn verify_key(key: &str) -> Result<(), String> {
    validate_key(key)?;
    /*
     * 只直连官方。
     *
     * 镜像不转发 `/v1/games/{id}`（实测 404），而且「验证 key」这件事本身就不能
     * 经过第三方——那等于把凭据交给它。这里固定用 `API` 而不是 `source.base()`。
     */
    let url =
        reqwest::Url::parse(&format!("{API}/v1/games/{GAME_ID}")).map_err(|_| "验证地址无效。")?;
    let client = catalog_client()?;
    let response = client
        .get(url)
        .header("x-api-key", key.trim())
        .header("Accept", "application/json")
        .timeout(Duration::from_secs(20))
        .send()
        .await
        .map_err(|_| "无法连接 CurseForge。")?;
    if response.status().is_success() {
        Ok(())
    } else {
        Err(status_error(
            Source::Official(key.trim()),
            response.status(),
        ))
    }
}

#[cfg(test)]
mod network_tests {
    use super::*;
    use crate::catalog::network_tests::MockProxy;

    #[tokio::test]
    async fn official_and_mirror_metadata_queries_use_settings_for_every_lookup() {
        for (source, host) in [
            (Source::Official("test-key"), "api.curseforge.com"),
            (Source::Mirror, "mod.mcimirror.top"),
        ] {
            let proxy = MockProxy::new("502 Bad Gateway", Duration::ZERO).await;
            let client = metadata_client_with_settings(&proxy.settings(0)).unwrap();
            assert!(
                search_with_client(&client, source, ContentKind::Mod, "", None, None, 0, 20)
                    .await
                    .is_err()
            );
            assert!(
                files_with_client(&client, source, "123", "", None)
                    .await
                    .is_err()
            );
            assert!(
                files_by_ids_with_client(&client, source, &[123])
                    .await
                    .is_err()
            );
            assert!(
                projects_by_ids_with_client(&client, source, &["123".into()])
                    .await
                    .is_err()
            );
            assert!(
                identify_files_with_client(&client, source, &[("m.jar".into(), 123)], None, None)
                    .await
                    .is_err()
            );
            assert!(
                pack_files_with_client(&client, source, &[123])
                    .await
                    .is_err()
            );
            let requests = proxy.requests.lock().unwrap();
            assert_eq!(requests.len(), 6);
            assert!(
                requests
                    .iter()
                    .all(|request| request.starts_with(&format!("CONNECT {host}:443 HTTP/1.1")))
            );
        }
    }

    #[tokio::test]
    async fn only_explicit_read_only_post_paths_are_retried() {
        for retries in [0, 2] {
            for (path, expected_retries) in [("/v1/mods/files", retries), ("/unlisted", 0)] {
                let proxy = MockProxy::new("503 Service Unavailable", Duration::ZERO).await;
                let client = proxy
                    .settings(retries)
                    .to_engine_settings()
                    .configure_metadata_client_with_post_paths(
                        reqwest::Client::builder(),
                        &["metadata.test"],
                        &["/v1/mods/files"],
                    )
                    .unwrap()
                    .build()
                    .unwrap();
                let response = client
                    .post(format!("http://metadata.test{path}"))
                    .json(&serde_json::json!({ "fileIds": [123] }))
                    .send()
                    .await
                    .unwrap();
                assert_eq!(response.status(), reqwest::StatusCode::SERVICE_UNAVAILABLE);
                let requests = proxy.requests.lock().unwrap();
                assert_eq!(requests.len(), usize::from(expected_retries) + 1);
                assert!(
                    requests
                        .iter()
                        .all(|request| request.contains("\"fileIds\":[123]"))
                );
            }
        }
    }

    #[test]
    fn invalid_network_configuration_does_not_fall_back_to_default() {
        let settings = crate::DownloadSettings {
            timeout_sec: 0,
            ..Default::default()
        };
        assert!(metadata_client_with_settings(&settings).is_err());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// CDN 备用地址：原地址排第一，其余是同一路径的等价主机。
    ///
    /// 只换主机名、不动路径——这是「同一个已知地址的容错」与「构造新地址」之间的
    /// 分界线，后者会越过作者的分布设置。
    #[test]
    fn cdn_alternates_keep_the_path_and_swap_hosts() {
        let url = "https://edge.forgecdn.net/files/9323/500/jei.jar";
        let alts = cdn_alternates(url);
        assert_eq!(alts[0], url, "original must come first");
        assert!(alts.len() >= 2, "expected at least one fallback host");
        for candidate in &alts {
            assert!(
                candidate.ends_with("/files/9323/500/jei.jar"),
                "path must be preserved: {candidate}"
            );
        }
        // 去重：不该出现两个相同地址。
        let mut unique = alts.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(unique.len(), alts.len());
    }

    #[test]
    fn cdn_alternates_leave_unknown_hosts_alone() {
        // 非 CDN 域名原样返回，不做任何替换。
        let url = "https://example.com/files/a.jar";
        assert_eq!(cdn_alternates(url), vec![url.to_owned()]);
    }

    fn search_json() -> Vec<u8> {
        // 真实响应形状的裁剪版：两个有效条目 + 一个空名条目（应被丢弃）。
        r#"{"data":[
            {"id":238222,"name":"Just Enough Items","summary":"查看配方","downloadCount":641245268,
             "logo":{"thumbnailUrl":"https://media.forgecdn.net/a.png"},
             "authors":[{"name":"mezz"}]},
            {"id":448233,"name":"Entity Culling","summary":"","downloadCount":10,
             "logo":{"url":"http://insecure.example/x.png"},
             "authors":[]},
            {"id":1,"name":"   ","summary":"blank","downloadCount":0}
        ]}"#
        .as_bytes()
        .to_vec()
    }

    #[test]
    fn search_results_are_parsed_and_blank_names_dropped() {
        let parsed = parse_search(&search_json()).unwrap();
        assert_eq!(parsed.len(), 2, "空名条目必须被丢弃");
        assert_eq!(parsed[0].project_id, "238222");
        assert_eq!(parsed[0].title, "Just Enough Items");
        assert_eq!(parsed[0].author, "mezz");
        assert_eq!(parsed[0].downloads, 641_245_268);
        // CurseForge 没有关注数：必须是 None，而不是伪造一个 0。
        assert_eq!(parsed[0].follows, None);
    }

    #[test]
    fn non_https_icons_are_dropped() {
        let parsed = parse_search(&search_json()).unwrap();
        // 第二条只有 http 图标，必须被过滤掉而不是原样透传。
        assert_eq!(parsed[1].icon_url, None);
    }

    #[test]
    fn unparsable_response_is_an_error_not_empty_list() {
        // 空列表会让界面显示「没有结果」，掩盖真正的解析失败。
        assert!(parse_search(b"not json").is_err());
    }

    fn files_json() -> Vec<u8> {
        r#"{"data":[
            {"id":9323500,"displayName":"JEI 15.74","fileName":"jei-1.20.1-fabric.jar",
             "fileDate":"2024-08-01T00:00:00Z","fileLength":1234,
             "downloadUrl":"https://edge.forgecdn.net/files/9323/500/jei.jar",
             "hashes":[{"value":"e23ae7e4cbb5aa0d1a3b0e9df7c1a1b2c3d4e5f6","algo":1},
                       {"value":"9d1d65341d7cb0d1a3b0e9df7c1a1b2","algo":2}],
             "gameVersions":["1.20.1","Fabric"]},
            {"id":2,"displayName":"restricted","fileName":"restricted.jar",
             "fileDate":"2024-08-01T00:00:00Z","fileLength":10,
             "downloadUrl":null,
             "hashes":[{"value":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","algo":1}],
             "gameVersions":["1.20.1"]},
            {"id":3,"displayName":"evil","fileName":"evil.jar",
             "fileDate":"2024-08-01T00:00:00Z","fileLength":10,
             "downloadUrl":"https://attacker.example/evil.jar",
             "hashes":[{"value":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb","algo":1}],
             "gameVersions":["1.20.1"]}
        ]}"#
        .as_bytes()
        .to_vec()
    }

    #[test]
    fn game_versions_and_loaders_are_split_apart() {
        /*
         * CurseForge 把加载器混在 `gameVersions` 里。不拆开的话，界面按「最大的
         * 那个」取分组标题会得到 "Fabric" 而不是版本号——这是实测踩到的坑。
         */
        let raw = vec![
            "1.20.1".to_owned(),
            "Fabric".to_owned(),
            "Java 17".to_owned(),
        ];
        let (versions, loaders) = split_versions_and_loaders(&raw);
        assert_eq!(versions, vec!["1.20.1"]);
        assert_eq!(loaders, vec!["Fabric"]);

        // 大小写与重复要被归一化。
        let raw = vec![
            "1.19.2".to_owned(),
            "forge".to_owned(),
            "Forge".to_owned(),
            "1.18.2".to_owned(),
        ];
        let (versions, loaders) = split_versions_and_loaders(&raw);
        assert_eq!(versions, vec!["1.19.2", "1.18.2"]);
        assert_eq!(loaders, vec!["Forge"]);

        // 什么都不像的条目（例如 Java 版本）应被丢弃，而不是当成版本号或加载器。
        let (versions, loaders) = split_versions_and_loaders(&["Java 17".to_owned()]);
        assert!(versions.is_empty());
        assert!(loaders.is_empty());

        // 快照号不算游戏版本：把 24w14a 当分组标题会让列表碎成一次性分组。
        let (versions, _) = split_versions_and_loaders(&["24w14a".to_owned()]);
        assert!(versions.is_empty());
    }

    #[test]
    fn numeric_version_shape_rejects_trailing_dot_and_letters() {
        assert!(is_numeric_version("1"));
        assert!(is_numeric_version("1.20"));
        assert!(is_numeric_version("1.20.1"));
        assert!(!is_numeric_version("1.20."));
        assert!(!is_numeric_version("v1.20"));
        assert!(!is_numeric_version("1.20.1-pre1"));
        assert!(!is_numeric_version(""));
    }

    #[test]
    fn files_keep_only_sha1_and_trusted_hosts() {
        let parsed = parse_files(&files_json()).unwrap();
        assert_eq!(parsed.len(), 3);

        // 正常文件：只取 algo 1（SHA-1），不混入 MD5。
        assert_eq!(
            parsed[0].sha1.as_deref(),
            Some("e23ae7e4cbb5aa0d1a3b0e9df7c1a1b2c3d4e5f6")
        );
        assert!(parsed[0].url.is_some());

        // 作者禁止第三方分发：downloadUrl 为 null，URL 必须保持 None。
        assert_eq!(parsed[1].url, None);

        // 攻击者域名必须被丢弃，否则启动器会下载任意外部地址。
        assert_eq!(parsed[2].url, None);
    }

    #[test]
    fn restricted_file_cannot_be_installed() {
        let parsed = parse_files(&files_json()).unwrap();
        let instance = InstanceRecord {
            id: "i".into(),
            name: "n".into(),
            game_version: "1.20.1".into(),
            loader: "fabric".into(),
            instance_dir: std::env::temp_dir(),
            game_dir: std::env::temp_dir(),
            isolated: true,
            state: crate::InstanceState::Ready,
            created_at: String::new(),
            last_launched_at: None,
        };
        let engine = download_engine::DownloadEngine::new(Default::default()).unwrap();
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let result = runtime.block_on(install(&engine, &instance, &parsed[1], ContentKind::Mod));
        assert!(result.is_err());
        // 错误文案必须指向「手动下载」，而不是含糊的失败。
        assert!(result.unwrap_err().contains("手动下载"));
    }

    #[test]
    fn loader_and_class_mappings_match_documented_values() {
        assert_eq!(class_id(ContentKind::Mod), 6);
        assert_eq!(class_id(ContentKind::Modpack), 4471);
        assert_eq!(class_id(ContentKind::ResourcePack), 12);
        assert_eq!(class_id(ContentKind::World), 17);
        assert_eq!(mod_loader_id("forge"), Some(1));
        assert_eq!(mod_loader_id("fabric"), Some(4));
        assert_eq!(mod_loader_id("quilt"), Some(5));
        assert_eq!(mod_loader_id("neoforge"), Some(6));
        assert_eq!(mod_loader_id("vanilla"), None);
    }

    #[test]
    fn each_content_kind_installs_into_its_own_directory() {
        // 四类内容在磁盘上的位置不同：写错目录游戏就读不到。
        assert_eq!(install_dir(ContentKind::Mod), Some("mods"));
        assert_eq!(
            install_dir(ContentKind::ResourcePack),
            Some("resourcepacks")
        );
        assert_eq!(install_dir(ContentKind::World), Some("saves"));
        // 整合包要创建新实例，不能落到某个目录里。
        assert_eq!(install_dir(ContentKind::Modpack), None);
    }

    #[test]
    fn installing_a_modpack_into_an_instance_is_refused() {
        let instance = InstanceRecord {
            id: "i".into(),
            name: "n".into(),
            game_version: "1.20.1".into(),
            loader: "fabric".into(),
            instance_dir: std::env::temp_dir(),
            game_dir: std::env::temp_dir(),
            isolated: true,
            state: crate::InstanceState::Ready,
            created_at: String::new(),
            last_launched_at: None,
        };
        let version = CurseForgeVersion {
            version_id: "1".into(),
            display_name: "pack".into(),
            date_created: String::new(),
            file_name: "pack.zip".into(),
            url: Some("https://edge.forgecdn.net/pack.zip".into()),
            sha1: Some("a".repeat(40)),
            size: 1,
            game_versions: vec![],
            loaders: vec![],
        };
        let engine = download_engine::DownloadEngine::new(Default::default()).unwrap();
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let result = runtime.block_on(install(&engine, &instance, &version, ContentKind::Modpack));
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("导入为新的实例"));
    }

    #[test]
    fn browse_mode_omits_search_filter_but_keeps_sorting() {
        // 浏览模式：空关键词不发送 searchFilter，但保留 sortField=6。
        let mut params = Vec::new();
        push_search_filters(&mut params, "", Some("1.21.1"), Some("fabric"));
        assert!(
            !params.iter().any(|(key, _)| key == "searchFilter"),
            "browse mode must not send searchFilter: {params:?}"
        );
        assert!(
            params
                .iter()
                .any(|(key, value)| key == "gameVersion" && value == "1.21.1")
        );
        // fabric 在 CurseForge 的 modLoaderType 是 4。
        assert!(
            params
                .iter()
                .any(|(key, value)| key == "modLoaderType" && value == "4")
        );

        // 有关键词时才发送。
        let mut params = Vec::new();
        push_search_filters(&mut params, "sodium", None, None);
        assert!(
            params
                .iter()
                .any(|(key, value)| key == "searchFilter" && value == "sodium")
        );
        // 没有版本时不发送加载器：文档要求两者配对使用。
        assert!(!params.iter().any(|(key, _)| key == "modLoaderType"));
    }

    #[test]
    fn empty_or_absurd_keys_are_rejected_before_any_request() {
        assert!(validate_key("").is_err());
        assert!(validate_key("   ").is_err());
        assert!(validate_key("has space").is_err());
        assert!(validate_key(&"a".repeat(200)).is_err());
        // 形如真实 key 的值通过格式检查（是否有效由服务端判定）。
        assert!(validate_key("$2a$10$abcdefghijklmnopqrstuv").is_ok());
    }

    #[test]
    fn paging_beyond_the_server_limit_is_refused_locally() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let result = runtime.block_on(search(
            Source::from_key(Some("$2a$10$abcdefghijklmnopqrstuv")),
            ContentKind::Mod,
            "jei",
            None,
            None,
            9_990,
            50,
        ));
        assert!(result.is_err());
    }

    /// 没有 key 时必须走镜像，而不是「无法搜索」。
    ///
    /// 这条约束容易在重构里被破坏：只要有人在某个接口开头重新加一句
    /// 「key 为空就返回错误」，CurseForge 面板就又变回需要开发者账号才能用。
    #[test]
    fn missing_key_selects_the_mirror() {
        assert_eq!(Source::from_key(None), Source::Mirror);
        assert_eq!(Source::from_key(Some("   ")), Source::Mirror);
        assert_eq!(Source::from_key(Some("")), Source::Mirror);
        assert_eq!(
            Source::from_key(Some("$2a$10$abcdefghijklmnopqrstuv")),
            Source::Official("$2a$10$abcdefghijklmnopqrstuv")
        );
    }

    /// 凭据只能出现在官方入口上。
    ///
    /// 实测 MCIM 对附带无效 `x-api-key` 的请求返回 500 而不是忽略，所以这里不只
    /// 是「会不会泄漏」，也是「发错了就直接坏掉」。
    #[test]
    fn only_the_official_source_carries_a_key() {
        let key = "$2a$10$abcdefghijklmnopqrstuv";
        assert_eq!(Source::Official(key).api_key(), Some(key));
        assert_eq!(Source::Mirror.api_key(), None);
    }

    /// 两个入口指向不同主机，且镜像路径仍带 `/curseforge` 前缀。
    #[test]
    fn sources_point_at_their_own_hosts() {
        let key = "$2a$10$abcdefghijklmnopqrstuv";
        assert_eq!(Source::Official(key).base(), "https://api.curseforge.com");
        assert_eq!(
            Source::Mirror.base(),
            "https://mod.mcimirror.top/curseforge"
        );
    }

    fn fingerprint_json() -> Vec<u8> {
        // 真实响应形状的裁剪版：两条命中 + 一条无 modId（应被丢弃）。
        r#"{"data":{"exactMatches":[
            {"id":9323500,"file":{
                "id":9323500,"modId":238222,
                "displayName":"JEI 15.74","fileName":"jei-1.20.1-fabric.jar",
                "fileDate":"2024-08-01T00:00:00Z","fileLength":1234,
                "downloadUrl":"https://edge.forgecdn.net/files/9323/500/jei.jar",
                "hashes":[{"value":"e23ae7e4cbb5aa0d1a3b0e9df7c1a1b2c3d4e5f6","algo":1}],
                "gameVersions":["1.20.1","Fabric"],
                "fileFingerprint":1234567890}},
            {"id":44,"file":{
                "id":44,"modId":306612,
                "displayName":"Sodium","fileName":"sodium.jar",
                "fileDate":"2024-07-01T00:00:00Z","fileLength":99,
                "downloadUrl":null,
                "hashes":[],
                "gameVersions":["1.20.1"],
                "fileFingerprint":42}},
            {"id":5,"file":{
                "id":5,"modId":0,"fileName":"orphan.jar",
                "fileFingerprint":7}}
        ]}}"#
            .as_bytes()
            .to_vec()
    }

    #[test]
    fn fingerprints_map_to_projects_and_keep_restricted_files() {
        let parsed = parse_fingerprint_matches(&fingerprint_json()).unwrap();
        assert_eq!(parsed.len(), 2, "没有 modId 的条目必须被丢弃");

        let (fingerprint, first) = &parsed[0];
        assert_eq!(*fingerprint, 1_234_567_890);
        assert_eq!(first.project_id, "238222");
        assert_eq!(first.file.file_name, "jei-1.20.1-fabric.jar");
        assert!(first.file.url.is_some());
        assert_eq!(first.file.loaders, vec!["Fabric"]);

        // 作者禁止第三方分发：工程仍然识别出来了，但 URL 保持 None。
        let (_, second) = &parsed[1];
        assert_eq!(second.project_id, "306612");
        assert_eq!(second.file.url, None);
    }

    #[test]
    fn fingerprint_response_is_not_silently_empty_on_garbage() {
        // 解析失败必须报错：返回空表会让界面显示「一个都没识别出来」。
        assert!(parse_fingerprint_matches(b"not json").is_err());
    }

    fn projects_json() -> Vec<u8> {
        r#"{"data":[
            {"id":238222,"name":"Just Enough Items","slug":"jei","summary":"查看配方",
             "latestFilesIndexes":[
                {"gameVersion":"1.20.1","fileId":901,"filename":"jei-1.20.1-fabric.jar","modLoader":4},
                {"gameVersion":"1.20.1","fileId":0,"filename":"broken.jar","modLoader":4},
                {"gameVersion":"1.20.1","fileId":903,"filename":"","modLoader":4}
             ]},
            {"id":306612,"name":"Sodium","slug":"sodium","summary":"渲染优化"},
            {"id":1,"name":"   ","slug":"blank","summary":""},
            {"id":2,"name":"Bad Slug","slug":"../../etc/passwd","summary":""}
        ]}"#
        .as_bytes()
        .to_vec()
    }

    #[test]
    fn projects_become_titles_and_safe_urls() {
        let parsed = parse_projects(&projects_json()).unwrap();
        assert_eq!(parsed.len(), 3, "空名工程必须被丢弃");
        assert_eq!(parsed["238222"].title, "Just Enough Items");
        assert_eq!(
            parsed["238222"].url,
            "https://www.curseforge.com/minecraft/mc-mods/jei"
        );
        /*
         * 带路径分隔符的 slug 会拼出另一个站点区域，必须退回只含数字的工程地址。
         * 这条是安全边界，不是样式问题。
         */
        assert_eq!(parsed["2"].url, "https://www.curseforge.com/projects/2");
    }

    #[test]
    fn latest_file_indexes_skip_malformed_entries() {
        let parsed = parse_projects(&projects_json()).unwrap();
        let latest = &parsed["238222"].latest_files;
        // fileId 为 0、文件名为空的条目必须被丢掉：它们不能下载，也不能比较版本。
        assert_eq!(latest.len(), 1);
        assert_eq!(latest[0].file_id, "901");
        assert_eq!(latest[0].game_version, "1.20.1");
        assert_eq!(latest[0].mod_loader, 4);
        // 没有索引的工程给空表，而不是一个「存在但无效」的条目。
        assert!(parsed["306612"].latest_files.is_empty());
    }

    #[test]
    fn project_urls_never_contain_a_path_escape() {
        // 把所有能想到的分隔符都过一遍：任何一个漏掉都会让链接指向别处。
        for slug in ["a/b", "a\\b", "a?b", "a#b", "a b", "a:b", ""] {
            let json =
                format!(r#"{{"data":[{{"id":7,"name":"x","slug":{slug:?},"summary":""}}]}}"#);
            let parsed = parse_projects(json.as_bytes()).unwrap();
            let url = &parsed["7"].url;
            assert!(
                url == "https://www.curseforge.com/projects/7",
                "slug {slug:?} must fall back, got {url}"
            );
        }
    }

    fn project_with_latest() -> CurseForgeProject {
        CurseForgeProject {
            project_id: "238222".into(),
            slug: "jei".into(),
            title: "Just Enough Items".into(),
            summary: String::new(),
            url: "https://www.curseforge.com/minecraft/mc-mods/jei".into(),
            latest_files: vec![
                CurseForgeFileIndex {
                    game_version: "1.21.1".into(),
                    file_id: "900".into(),
                    file_name: "jei-1.21.1.jar".into(),
                    mod_loader: 4,
                },
                CurseForgeFileIndex {
                    game_version: "1.20.1".into(),
                    file_id: "901".into(),
                    file_name: "jei-1.20.1-fabric.jar".into(),
                    mod_loader: 4,
                },
                CurseForgeFileIndex {
                    game_version: "1.20.1".into(),
                    file_id: "902".into(),
                    file_name: "jei-1.20.1-forge.jar".into(),
                    mod_loader: 1,
                },
                CurseForgeFileIndex {
                    game_version: "1.20.1".into(),
                    file_id: "903".into(),
                    file_name: "jei-1.20.1-any.jar".into(),
                    mod_loader: 0,
                },
            ],
        }
    }

    #[test]
    fn latest_file_matches_game_version_and_loader_exactly() {
        let project = project_with_latest();
        // Fabric 实例：必须是 1.20.1 + Fabric，不能是 1.21.1，也不能是 Forge 那份。
        let picked = pick_latest(&project, Some("1.20.1"), Some("fabric")).unwrap();
        assert_eq!(picked.file_id, "901");
        // Forge 实例拿到的是 Forge 的文件。
        let picked = pick_latest(&project, Some("1.20.1"), Some("forge")).unwrap();
        assert_eq!(picked.file_id, "902");
    }

    #[test]
    fn loader_any_is_accepted_but_never_wrong_version() {
        let mut project = project_with_latest();
        // 只留一个标成 Any 的文件：它应当被接受（作者声明与加载器无关）。
        project.latest_files = vec![CurseForgeFileIndex {
            game_version: "1.20.1".into(),
            file_id: "903".into(),
            file_name: "jei-any.jar".into(),
            mod_loader: 0,
        }];
        assert_eq!(
            pick_latest(&project, Some("1.20.1"), Some("fabric"))
                .unwrap()
                .file_id,
            "903"
        );
        // 版本不符时必须返回 None：跨版本安装会让游戏直接崩溃。
        assert!(pick_latest(&project, Some("1.21.1"), Some("fabric")).is_none());
    }

    #[test]
    fn latest_lookup_without_filters_takes_the_first_index() {
        // 原版实例没有加载器可比：不筛加载器，也不因为缺少版本信息就报错。
        let project = project_with_latest();
        assert!(pick_latest(&project, None, None).is_some());
        // 空字符串按「没给」处理，而不是按字面空版本去匹配。
        assert!(pick_latest(&project, Some("  "), None).is_some());
    }
}
