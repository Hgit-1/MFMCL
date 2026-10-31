//! Modrinth online mod catalogue: search, per-project versions, verified install.
//!
//! Same trust surface as mod updates: `api.modrinth.com` for metadata,
//! `cdn.modrinth.com` for files, SHA-1 pinned before publication. Searches
//! send the query and the instance's game version/loader facets to Modrinth;
//! the UI discloses this. Installing never overwrites an existing mod file.
use crate::InstanceRecord;
use download_engine::{ArtifactSpec, DownloadEngine};
use serde::{Deserialize, Serialize};
use std::{path::Path, time::Duration};

const API: &str = "https://api.modrinth.com/v2";
const MAX_QUERY: usize = 128;
const MAX_RESULTS: usize = 20;
const MAX_RESPONSE: usize = 4 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CatalogMod {
    pub project_id: String,
    pub title: String,
    pub description: String,
    pub author: String,
    pub downloads: u64,
    pub follows: u64,
    pub icon_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CatalogVersion {
    pub version_id: String,
    pub version_number: String,
    pub date_created: String,
    pub file_name: String,
    pub url: String,
    pub sha1: String,
    pub size: u64,
}

pub fn catalog_client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .https_only(true)
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(30))
        .user_agent(concat!("MFMCL/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|_| "无法初始化目录客户端。".to_owned())
}

fn loader_of(instance: &InstanceRecord) -> Result<&'static str, String> {
    crate::mod_updates::instance_loader_name(instance)
        .ok_or_else(|| "在线模组目录需要 Fabric 或 Quilt 实例。".to_owned())
}

async fn bounded(mut response: reqwest::Response, limit: usize) -> Result<Vec<u8>, String> {
    if response
        .content_length()
        .is_some_and(|length| length as usize > limit)
    {
        return Err("Modrinth 响应过大。".into());
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| "Modrinth 读取失败。")? {
        if bytes.len().saturating_add(chunk.len()) > limit {
            return Err("Modrinth 响应过大。".into());
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

/// 一次内容搜索的筛选条件。
///
/// 对应 PCL2 搜索面板的四组输入：名称、来源、版本、类型（标签）。
/// 全部字段可选，空值表示「不限制」——这样同一个入口既用于关键词搜索，
/// 也用于「什么都不填、直接按下载量浏览」，不需要两条代码路径。
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ModQuery {
    /// 关键词。空字符串表示按下载量排序浏览。
    #[serde(default)]
    pub query: String,
    /// 内容类型：`mod` / `resourcepack` / `shader` / `datapack` / `modpack`。
    ///
    /// 空字符串等价于 `mod`，保持既有调用方的语义不变。这个字段决定了三件事：
    /// Modrinth 的 `project_type` facet、是否按加载器筛选、以及文件装到哪个目录。
    #[serde(default)]
    pub project_type: String,
    /// 游戏版本。`None` 表示用目标实例的版本。
    #[serde(default)]
    pub game_version: Option<String>,
    /// 加载器（fabric / quilt / forge / neoforge）。`None` 表示用目标实例的加载器。
    #[serde(default)]
    pub loader: Option<String>,
    /// Modrinth 分类标签，例如 `optimization`。空表示不筛选。
    #[serde(default)]
    pub category: String,
    /// 排序字段：`relevance` / `downloads` / `newest` / `updated` / `follows`。
    #[serde(default)]
    pub index: Option<String>,
    /// 偏移量，用于翻页。
    #[serde(default)]
    pub offset: u32,
}

/// 允许的内容类型。
///
/// 白名单而不是直接透传：`project_type` 会被拼进 Modrinth 的 facet 字符串，
/// 任意输入等于把请求参数的构造权交给了调用方。
///
/// 资源包、光影包与数据包**不按加载器筛选**——它们的版本里 `loaders` 字段是
/// `["minecraft"]`，用 `fabric` 之类的值去筛一定返回空。这是最容易写错的一点。
pub fn normalize_project_type(value: &str) -> Result<&'static str, String> {
    match value.trim() {
        "" | "mod" => Ok("mod"),
        "resourcepack" => Ok("resourcepack"),
        "shader" => Ok("shader"),
        "datapack" => Ok("datapack"),
        "modpack" => Ok("modpack"),
        other => Err(format!("不支持的内容类型：{other}")),
    }
}

/// 该内容类型是否按 Mod 加载器筛选版本。
fn filters_by_loader(project_type: &str) -> bool {
    // 只有模组与整合包有加载器概念；资源包/光影包/数据包是全局资源。
    matches!(project_type, "mod" | "modpack")
}

/// 允许的排序字段。只接受白名单，避免把任意串拼进请求。
fn sort_index(value: Option<&str>) -> &'static str {
    match value {
        Some("downloads") => "downloads",
        Some("newest") => "newest",
        Some("updated") => "updated",
        Some("follows") => "follows",
        _ => "relevance",
    }
}

/// 解析请求里的加载器筛选。
///
/// 三种输入必须区分开，否则「想看不限加载器的全部模组」和「没填、跟随实例」
/// 会混成一种行为：
/// - `None` / 空串：跟随目标实例（原版实例则不加加载器 facet）
/// - `Some("any")`：明确不限制加载器
/// - 其他：按该加载器筛选
fn resolve_loader<'a>(requested: Option<&'a str>, instance: &'a InstanceRecord) -> Option<&'a str> {
    match requested.map(str::trim) {
        Some(loader) if loader.eq_ignore_ascii_case("any") => None,
        Some(loader) if !loader.is_empty() => Some(loader),
        _ => crate::mod_updates::instance_loader_name(instance),
    }
}

/// 搜索模组。
///
/// 与之前只接受关键词的版本相比，这里允许**空关键词**：Modrinth 的搜索接口在
/// `query` 为空时按 facet 列出项目，配合 `index=downloads` 就是「这个版本+加载器
/// 下最受欢迎的模组」。PCL2 进入页面即可看到列表，正是靠这一点；只要求输入关键词
/// 会让第一次打开的面板永远是空的。
pub async fn search_mods(
    client: &reqwest::Client,
    instance: &InstanceRecord,
    request: &ModQuery,
) -> Result<Vec<CatalogMod>, String> {
    let query = request.query.trim();
    if query.chars().count() > MAX_QUERY {
        return Err("搜索词不能超过 128 个字符。".into());
    }
    let project_type = normalize_project_type(&request.project_type)?;
    // 未指定时沿用目标实例的版本，保持「搜的就是能装进这个实例的」。
    let game_version = request
        .game_version
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(instance.game_version.as_str());
    /*
     * 加载器只在模组/整合包上生效。
     *
     * 资源包的版本里 `loaders` 是 `["minecraft"]`，按 `fabric` 筛会得到空列表——
     * 用户看到「没有结果」却不知道是自己选错了筛选维度。这里直接不带加载器 facet，
     * 让资源包按游戏版本正常列出。
     */
    let loader = if filters_by_loader(project_type) {
        resolve_loader(request.loader.as_deref(), instance)
    } else {
        None
    };
    if game_version.len() > 64
        || loader.is_some_and(|loader| {
            loader.len() > 32
                || !loader
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-')
        })
        || request.category.len() > 64
        || !request
            .category
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-')
    {
        return Err("搜索条件无效。".into());
    }

    // 每个 facet 组之间是 AND，组内多个值是 OR。
    let mut facets = vec![format!("[\"project_type:{project_type}\"]")];
    if let Some(loader) = loader {
        facets.push(format!("[\"categories:{loader}\"]"));
    }
    if !game_version.is_empty() {
        facets.push(format!("[\"versions:{game_version}\"]"));
    }
    if !request.category.is_empty() {
        facets.push(format!("[\"categories:{}\"]", request.category));
    }

    let offset = request.offset.min(10_000);
    let mut params: Vec<(&str, String)> = vec![
        ("facets", format!("[{}]", facets.join(","))),
        ("limit", MAX_RESULTS.to_string()),
        ("index", sort_index(request.index.as_deref()).to_owned()),
        ("offset", offset.to_string()),
    ];
    // 空 query 就不发这个参数：Modrinth 对空字符串与缺省的处理不同。
    if !query.is_empty() {
        params.push(("query", query.to_owned()));
    }
    let url = reqwest::Url::parse_with_params(&format!("{API}/search"), &params)
        .map_err(|_| "搜索地址无效。")?;
    let response = client
        .get(url)
        .send()
        .await
        .map_err(|_| "无法连接 Modrinth。")?
        .error_for_status()
        .map_err(|_| "Modrinth 拒绝了搜索请求。")?;
    let bytes = bounded(response, MAX_RESPONSE).await?;
    parse_search(&bytes)
}

fn parse_search(bytes: &[u8]) -> Result<Vec<CatalogMod>, String> {
    #[derive(Deserialize)]
    struct Hit {
        project_id: String,
        title: String,
        description: String,
        author: String,
        downloads: u64,
        follows: u64,
        #[serde(default)]
        icon_url: Option<String>,
    }
    #[derive(Deserialize)]
    struct SearchResponse {
        #[serde(default)]
        hits: Vec<Hit>,
    }
    let parsed: SearchResponse =
        serde_json::from_slice(bytes).map_err(|_| "Modrinth 搜索结果无效。")?;
    if parsed.hits.len() > MAX_RESULTS {
        return Err("Modrinth 搜索结果数量异常。".into());
    }
    // A single malformed hit is skipped, not fatal: browsing stays usable.
    let valid = parsed
        .hits
        .into_iter()
        .filter_map(|hit| {
            if hit.project_id.is_empty() || hit.title.chars().count() > 256 {
                return None;
            }
            Some(CatalogMod {
                project_id: hit.project_id,
                title: hit.title,
                description: truncate(&hit.description, 512),
                author: truncate(&hit.author, 128),
                downloads: hit.downloads,
                follows: hit.follows,
                icon_url: hit.icon_url.filter(|url| {
                    reqwest::Url::parse(url).is_ok_and(|parsed| parsed.scheme() == "https")
                }),
            })
        })
        .collect::<Vec<_>>();
    Ok(valid)
}

fn truncate(value: &str, limit: usize) -> String {
    value.chars().take(limit).collect()
}

/// List installable versions of a project for this instance.
pub async fn mod_versions(
    client: &reqwest::Client,
    instance: &InstanceRecord,
    project_id: &str,
) -> Result<Vec<CatalogVersion>, String> {
    mod_versions_of(client, instance, project_id, "mod").await
}

/// 列出一个项目的可安装版本。
///
/// `project_type` 决定是否按加载器筛选：模组需要，资源包/光影包/数据包不需要
/// （它们的 `loaders` 是 `["minecraft"]`，按 `fabric` 筛会是空列表）。
pub async fn mod_versions_of(
    client: &reqwest::Client,
    instance: &InstanceRecord,
    project_id: &str,
    project_type: &str,
) -> Result<Vec<CatalogVersion>, String> {
    let project_type = normalize_project_type(project_type)?;
    if project_id.is_empty()
        || project_id.len() > 128
        || !project_id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        return Err("项目标识无效。".into());
    }
    let mut params: Vec<(&str, String)> =
        vec![("game_versions", format!("[\"{}\"]", instance.game_version))];
    if filters_by_loader(project_type) {
        let loader = loader_of(instance)?;
        params.push(("loaders", format!("[\"{loader}\"]")));
    }
    let url =
        reqwest::Url::parse_with_params(&format!("{API}/project/{project_id}/version"), &params)
            .map_err(|_| "版本地址无效。")?;
    let response = client
        .get(url)
        .send()
        .await
        .map_err(|_| "无法连接 Modrinth。")?
        .error_for_status()
        .map_err(|_| "Modrinth 项目查询失败。")?;
    let bytes = bounded(response, MAX_RESPONSE).await?;
    parse_versions(&bytes)
}

fn parse_versions(bytes: &[u8]) -> Result<Vec<CatalogVersion>, String> {
    #[derive(Deserialize)]
    struct File {
        url: String,
        filename: String,
        #[serde(default)]
        primary: bool,
        size: u64,
        #[serde(default)]
        hashes: std::collections::HashMap<String, String>,
    }
    #[derive(Deserialize)]
    struct Version {
        id: String,
        version_number: String,
        date_created: String,
        #[serde(default)]
        files: Vec<File>,
    }
    let parsed: Vec<Version> =
        serde_json::from_slice(bytes).map_err(|_| "Modrinth 版本列表无效。")?;
    if parsed.len() > 100 {
        return Err("Modrinth 版本数量异常。".into());
    }
    parsed
        .into_iter()
        .map(|version| {
            let file = version
                .files
                .iter()
                .find(|file| file.primary)
                .or_else(|| version.files.first())
                .ok_or_else(|| "此版本没有可下载文件。".to_owned())?;
            let url = reqwest::Url::parse(&file.url).map_err(|_| "下载地址无效。")?;
            if url.scheme() != "https" || url.host_str() != Some("cdn.modrinth.com") {
                return Err("文件不来自 Modrinth 官方分发域名。".into());
            }
            let sha1 = file
                .hashes
                .get("sha1")
                .filter(|hash| hash.len() == 40 && hash.bytes().all(|b| b.is_ascii_hexdigit()))
                .ok_or_else(|| "此版本缺少有效 SHA-1 校验值。".to_owned())?;
            if !crate::mod_updates::is_safe_mod_name(&file.filename) {
                return Err("此版本的文件名不适用于安全安装。".into());
            }
            Ok(CatalogVersion {
                version_id: version.id,
                version_number: truncate(&version.version_number, 128),
                date_created: version.date_created,
                file_name: file.filename.clone(),
                url: file.url.clone(),
                sha1: sha1.clone(),
                size: file.size,
            })
        })
        .collect()
}

/// Verified install into the instance's mods directory. An existing file with
/// the same name is never overwritten; the update flow handles replacements.
pub async fn install_mod_version(
    update_target: &CatalogVersion,
    mods_dir: &Path,
    engine: &DownloadEngine,
) -> Result<String, String> {
    download_mod_version(update_target, mods_dir, engine, "mods 目录", "安装").await
}

/// 直接把模组文件下载到用户指定的目录，不装进实例。
///
/// 校验与「不覆盖同名文件」的保证和 [`install_mod_version`] 完全一致，只有落盘
/// 位置不同。用于「只想拿到 jar 本身」的场景。
pub async fn download_mod_version_to(
    update_target: &CatalogVersion,
    directory: &Path,
    engine: &DownloadEngine,
) -> Result<String, String> {
    download_mod_version(update_target, directory, engine, "所选目录", "下载").await
}

async fn download_mod_version(
    update_target: &CatalogVersion,
    directory: &Path,
    engine: &DownloadEngine,
    directory_hint: &str,
    verb: &str,
) -> Result<String, String> {
    if !crate::mod_updates::is_safe_mod_name(&update_target.file_name) {
        return Err("模组文件名无效。".into());
    }
    std::fs::create_dir_all(directory)
        .map_err(|error| format!("无法创建{directory_hint}：{error}"))?;
    let destination = directory.join(&update_target.file_name);
    if std::fs::symlink_metadata(&destination).is_ok() {
        return Err(format!(
            "{directory_hint}中已存在同名文件 {}；如需替换请先删除它。",
            update_target.file_name
        ));
    }
    let temporary = directory.join(format!(".install-{}.jar", uuid::Uuid::new_v4().simple()));
    engine
        .download(
            &ArtifactSpec {
                id: format!("mod-install:{}", update_target.version_id),
                label: format!("{verb}模组 {}", update_target.file_name),
                url: update_target.url.clone(),
                mirror_url: None,
                destination: temporary.clone(),
                size: Some(update_target.size),
                sha1: Some(update_target.sha1.clone()),
                sha256: None,
            },
            None,
        )
        .await
        .map_err(|_| "模组下载或校验失败；没有改动任何文件。")?;
    // hard_link atomically refuses an existing destination (also dangling symlinks).
    if let Err(error) = std::fs::hard_link(&temporary, &destination) {
        let _ = std::fs::remove_file(&temporary);
        return Err(format!("无法发布模组文件（不会覆盖已有文件）：{error}"));
    }
    std::fs::remove_file(&temporary)
        .map_err(|e| format!("模组已{verb}，但清理临时文件失败：{e}"))?;
    Ok(destination.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn search_json() -> Vec<u8> {
        r#"{"hits":[{"project_id":"p1","title":"Sodium","description":"渲染优化","author":"作者","downloads":1000,"follows":50,"icon_url":"https://cdn.modrinth.com/icon.png"},{"project_id":"","title":"bad","description":"","author":"","downloads":0,"follows":0}]}"#.as_bytes().to_vec()
    }

    #[test]
    fn search_results_are_bounded_and_validated() {
        let parsed = parse_search(&search_json()).unwrap();
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].project_id, "p1");
        assert_eq!(parsed[0].title, "Sodium");
        assert!(parsed[0].icon_url.is_some());
        assert!(parse_search(b"{").is_err());
        let oversized = format!(
            "{{\"hits\":[{}]}}",
            (0..21)
                .map(|i| format!("{{\"project_id\":\"p{i}\",\"title\":\"t\",\"description\":\"\",\"author\":\"\",\"downloads\":0,\"follows\":0}}"))
                .collect::<Vec<_>>()
                .join(",")
        );
        assert!(parse_search(oversized.as_bytes()).is_err());
    }

    #[test]
    fn versions_require_cdn_https_and_strong_hash() {
        let base = r#"[{"id":"v1","version_number":"1.0","date_created":"2026-01-01T00:00:00Z","files":[{"url":"https://cdn.modrinth.com/m.jar","filename":"m.jar","primary":true,"size":10,"hashes":{"sha1":"%HASH%"}}]}]"#;
        let good = base.replace("%HASH%", &"a".repeat(40));
        let parsed = parse_versions(good.as_bytes()).unwrap();
        assert_eq!(parsed[0].version_number, "1.0");
        assert_eq!(parsed[0].file_name, "m.jar");
        for bad in [
            base.replace("cdn.modrinth.com", "evil.example.com"),
            base.replace("%HASH%", "short"),
            base.replace("%HASH%", &"z".repeat(40)),
            base.replace("m.jar", "../m.jar"),
            base.replace("https://", "http://"),
        ] {
            assert!(parse_versions(bad.as_bytes()).is_err(), "{bad}");
        }
    }

    #[tokio::test]
    async fn install_refuses_existing_name_and_never_overwrites() {
        let root = std::env::temp_dir().join(format!("mfmcl-catalog-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(root.join("mods")).unwrap();
        std::fs::write(root.join("mods/taken.jar"), b"existing").unwrap();
        let version = CatalogVersion {
            version_id: "v1".into(),
            version_number: "1.0".into(),
            date_created: String::new(),
            file_name: "taken.jar".into(),
            url: "https://cdn.modrinth.com/taken.jar".into(),
            sha1: "0".repeat(40),
            size: 1,
        };
        let engine = download_engine::DownloadEngine::new(Default::default()).unwrap();
        let result = install_mod_version(&version, &root.join("mods"), &engine).await;
        assert!(result.is_err());
        assert_eq!(
            std::fs::read(root.join("mods/taken.jar")).unwrap(),
            b"existing"
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn direct_download_writes_into_the_chosen_directory() {
        let root = std::env::temp_dir().join(format!("mfmcl-direct-{}", uuid::Uuid::new_v4()));
        let target = root.join("user-picked");
        std::fs::create_dir_all(&target).unwrap();
        std::fs::write(target.join("taken.jar"), b"existing").unwrap();
        let version = CatalogVersion {
            version_id: "v1".into(),
            version_number: "1.0".into(),
            date_created: String::new(),
            file_name: "taken.jar".into(),
            url: "https://cdn.modrinth.com/taken.jar".into(),
            sha1: "0".repeat(40),
            size: 1,
        };
        let engine = download_engine::DownloadEngine::new(Default::default()).unwrap();
        // 与安装路径共用同一套保护：同名文件绝不被覆盖。
        let error = download_mod_version_to(&version, &target, &engine)
            .await
            .unwrap_err();
        assert!(error.contains("已存在同名文件"), "{error}");
        assert_eq!(
            std::fs::read(target.join("taken.jar")).unwrap(),
            b"existing"
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn sort_index_only_accepts_known_values() {
        // 白名单：任意串都会退回 relevance，不会被拼进请求。
        assert_eq!(sort_index(Some("downloads")), "downloads");
        assert_eq!(sort_index(Some("newest")), "newest");
        assert_eq!(sort_index(Some("updated")), "updated");
        assert_eq!(sort_index(Some("follows")), "follows");
        assert_eq!(sort_index(Some("relevance")), "relevance");
        assert_eq!(sort_index(Some("; DROP TABLE")), "relevance");
        assert_eq!(sort_index(None), "relevance");
    }

    #[test]
    fn mod_query_defaults_describe_a_browse_request() {
        // 默认值就是「什么都不填」——空关键词必须被接受，否则面板首次打开永远是空的。
        let request = ModQuery::default();
        assert!(request.query.is_empty());
        assert_eq!(request.offset, 0);
        assert!(request.game_version.is_none());
        assert!(request.loader.is_none());
        // 空 projectType 等价于 mod，既有调用方不必改。
        assert_eq!(
            normalize_project_type(&request.project_type).unwrap(),
            "mod"
        );
    }

    #[test]
    fn project_type_is_whitelisted() {
        for (input, expected) in [
            ("", "mod"),
            ("mod", "mod"),
            ("resourcepack", "resourcepack"),
            ("shader", "shader"),
            ("datapack", "datapack"),
            ("modpack", "modpack"),
            (" resourcepack ", "resourcepack"),
        ] {
            assert_eq!(normalize_project_type(input).unwrap(), expected, "{input}");
        }
        // 任意串不能直接拼进 facet。
        for bad in ["mod; DROP", "resourcepack\"", "../etc", "unknown"] {
            assert!(normalize_project_type(bad).is_err(), "{bad}");
        }
    }

    /// 资源包/光影包/数据包的版本里 `loaders` 是 `["minecraft"]`，按 `fabric`
    /// 之类去筛一定是空列表。这条测试钉住「只有模组和整合包按加载器筛选」。
    #[test]
    fn only_mods_and_modpacks_filter_by_loader() {
        assert!(filters_by_loader("mod"));
        assert!(filters_by_loader("modpack"));
        assert!(!filters_by_loader("resourcepack"));
        assert!(!filters_by_loader("shader"));
        assert!(!filters_by_loader("datapack"));
    }

    #[test]
    fn empty_query_is_not_rejected_by_length_check() {
        // len 检查只针对过长输入；空串应当通过，交由调用方按浏览处理。
        let request = ModQuery {
            query: " ".into(),
            ..ModQuery::default()
        };
        assert!(request.query.trim().chars().count() <= MAX_QUERY);
    }

    fn fabric_instance() -> InstanceRecord {
        InstanceRecord {
            id: "i".into(),
            name: "n".into(),
            game_version: "1.21.1".into(),
            // 真实写入路径是 `fabric@0.16.x` 形态（`loaders.rs` 用 `format!("{}@{version}")`），
            // `instance_loader_name` 只认这种带 `@` 的值。
            loader: "fabric@0.16.10".into(),
            instance_dir: std::path::PathBuf::from("/tmp/i"),
            game_dir: std::path::PathBuf::from("/tmp/i/game"),
            isolated: true,
            state: crate::InstanceState::Ready,
            created_at: String::new(),
            last_launched_at: None,
        }
    }

    #[test]
    fn loader_filter_distinguishes_any_from_unspecified() {
        let instance = fabric_instance();
        // 未指定 → 跟随实例
        assert_eq!(resolve_loader(None, &instance), Some("fabric"));
        assert_eq!(resolve_loader(Some("  "), &instance), Some("fabric"));
        // 明确「任意」→ 不加加载器 facet
        assert_eq!(resolve_loader(Some("any"), &instance), None);
        assert_eq!(resolve_loader(Some("ANY"), &instance), None);
        // 显式覆盖 → 用用户选的
        assert_eq!(
            resolve_loader(Some("neoforge"), &instance),
            Some("neoforge")
        );
    }

    #[test]
    fn vanilla_instance_has_no_loader_facet_by_default() {
        let mut instance = fabric_instance();
        instance.loader = "vanilla".into();
        assert_eq!(resolve_loader(None, &instance), None);
    }
}
