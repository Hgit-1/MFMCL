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
/// Minecraft 在 CurseForge 的 gameId。
const GAME_ID: u32 = 432;
const MAX_QUERY: usize = 128;
const MAX_RESULTS: u32 = 20;
const MAX_RESPONSE: usize = 4 * 1024 * 1024;
/// 服务端硬上限：`index + pageSize <= 10000`。
const MAX_PAGE_WINDOW: u32 = 10_000;

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
/// 403 是 CurseForge 唯一会用的认证失败码，且响应体为空——missing key 与 wrong key
/// 无法区分，所以文案必须同时覆盖两种情况。
fn status_error(status: reqwest::StatusCode) -> String {
    match status.as_u16() {
        400 => "CurseForge 拒绝了请求参数（分页或筛选超出允许范围）。".into(),
        401 | 403 => "CurseForge API Key 缺失或无效，请在内容商店里重新填写。".into(),
        404 => "CurseForge 上找不到该项目。".into(),
        429 => "CurseForge 限流中，请稍后再试。".into(),
        _ => format!("CurseForge 请求失败（HTTP {}）。", status.as_u16()),
    }
}

async fn get_json(
    client: &reqwest::Client,
    key: &str,
    url: reqwest::Url,
) -> Result<Vec<u8>, String> {
    validate_key(key)?;
    let response = client
        .get(url)
        .header("x-api-key", key.trim())
        .header("Accept", "application/json")
        .send()
        .await
        .map_err(|_| "无法连接 CurseForge。")?;
    if !response.status().is_success() {
        return Err(status_error(response.status()));
    }
    bounded(response, MAX_RESPONSE).await
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
    key: &str,
    file_ids: &[u64],
) -> Result<std::collections::HashMap<u64, CurseForgeVersion>, String> {
    validate_key(key)?;
    if file_ids.is_empty() {
        return Ok(std::collections::HashMap::new());
    }
    // 服务端上限是 200 个一批；调用方可能给更多，这里分片以免整单失败。
    const BATCH: usize = 200;
    let client = catalog_client()?;
    let mut out = std::collections::HashMap::new();
    for chunk in file_ids.chunks(BATCH) {
        let body = serde_json::json!({ "fileIds": chunk });
        let response = client
            .post(format!("{API}/v1/mods/files"))
            .header("x-api-key", key.trim())
            .header("Accept", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|_| "无法连接 CurseForge。")?;
        if !response.status().is_success() {
            return Err(status_error(response.status()));
        }
        let bytes = bounded(response, MAX_RESPONSE).await?;
        for version in parse_files(&bytes)? {
            if let Ok(id) = version.version_id.parse::<u64>() {
                out.insert(id, version);
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
    key: &str,
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

    let url = reqwest::Url::parse_with_params(&format!("{API}/v1/mods/search"), &params)
        .map_err(|_| "搜索地址无效。")?;
    let client = catalog_client()?;
    let bytes = get_json(&client, key, url).await?;
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
    key: &str,
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
    let url =
        reqwest::Url::parse_with_params(&format!("{API}/v1/mods/{project_id}/files"), &params)
            .map_err(|_| "版本地址无效。")?;

    let client = catalog_client()?;
    let bytes = get_json(&client, key, url).await?;
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

/// 共享的「下载 → 校验 → 原子发布」实现。
///
/// `directory_hint` 只用于错误文案：实例目录叫「mods 目录」，用户自选目录叫「此目录」。
#[allow(clippy::too_many_arguments)]
async fn download_verified(
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
    engine
        .download(
            &ArtifactSpec {
                id: match version_id {
                    Some(id) => format!("curseforge-download:{id}"),
                    None => format!("download:{}", uuid::Uuid::new_v4().simple()),
                },
                label: format!("下载{label} {file_name}"),
                url: url.to_owned(),
                mirror_url: None,
                destination: temporary.clone(),
                size: (size > 0).then_some(size),
                sha1: Some(sha1.to_owned()),
                sha256: None,
            },
            None,
        )
        .await
        .map_err(|_| format!("{label}下载或校验失败；没有改动任何文件。"))?;
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
        Err(status_error(response.status()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
            "$2a$10$abcdefghijklmnopqrstuv",
            ContentKind::Mod,
            "jei",
            None,
            None,
            9_990,
            50,
        ));
        assert!(result.is_err());
    }
}
