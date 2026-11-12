//! Standalone vanilla OptiFine, using links published by https://optifine.net/downloads.
//!
//! The public `(Mirror)` link leads to `adloadx`; its HTML supplies the complete
//! `downloadx` URL, including the short-lived token. Tokens are never synthesized.
//! OptiFine publishes no authoritative digest here: transport provenance, bounded
//! ZIP/CRC inspection and Config.class constants are checked, NOT an official hash.
//! Only the official JAR's `optifine.Patcher` is executed, against a private copy of
//! the Mojang-verified client. This is trusted code execution, not an OS sandbox.

use crate::{InstanceRecord, InstanceState, LauncherStore};
use download_engine::{
    DownloadControl, DownloadEngine, DownloadProgress, DownloadSettings as EngineSettings,
    DownloadStage,
};
use mc_metadata::{Artifact, GameVersionMetadata, Library, LibraryDownloads};
use reqwest::{Client, Url};
use sha1::{Digest, Sha1};
use sha2::Sha256;
use std::{
    collections::{BTreeMap, HashSet},
    fs::{self, File, OpenOptions},
    future::Future,
    io::{Cursor, Read, Write},
    path::{Component, Path, PathBuf},
    process::Stdio,
    time::Duration,
};
use tokio::sync::mpsc;

const DOWNLOADS: &str = "https://optifine.net/downloads";
const HTML_LIMIT: usize = 4 * 1024 * 1024;
const JAR_LIMIT: usize = 64 * 1024 * 1024;
const CLIENT_LIMIT: usize = 128 * 1024 * 1024;
const ENTRY_LIMIT: u64 = 64 * 1024 * 1024;
const EXPANDED_LIMIT: u64 = 512 * 1024 * 1024;
const PATCH_TIMEOUT: Duration = Duration::from_secs(180);
const VERIFICATION: &str = "officialHttpsAndInspectedJar";
type Progress = Option<mpsc::UnboundedSender<DownloadProgress>>;

#[derive(Debug, Clone)]
struct Release {
    version: String,
    file: String,
    mirror: Url,
    stable: bool,
}

fn official_url(base: &Url, href: &str) -> Result<Url, String> {
    if href.len() > 4096 || href.chars().any(char::is_control) {
        return Err("OptiFine URL 无效。".into());
    }
    let mut url = base.join(href).map_err(|_| "OptiFine URL 无效。")?;
    if !matches!(url.host_str(), Some("optifine.net" | "www.optifine.net"))
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
        || url.port().is_some()
    {
        return Err("OptiFine 链接不在获准的官方主机上。".into());
    }
    // The official downloads page historically publishes http:// Mirror anchors.
    // Upgrade that published link before making any request; never send plaintext.
    if url.scheme() == "http" {
        url.set_scheme("https").map_err(|_| "OptiFine URL 无效。")?;
    }
    if url.scheme() != "https" {
        return Err("OptiFine 下载仅允许官方 HTTPS。".into());
    }
    Ok(url)
}

fn official_client(settings: &EngineSettings) -> Result<Client, String> {
    settings
        .configure_metadata_client(
            Client::builder()
                .https_only(true)
                .user_agent(concat!("MFMCL/", env!("CARGO_PKG_VERSION")))
                .redirect(reqwest::redirect::Policy::none()),
            &["optifine.net", "www.optifine.net"],
        )
        .map_err(|e| e.to_string())?
        .build()
        .map_err(|_| "无法初始化 OptiFine HTTPS 客户端。".into())
}

fn anchors(html: &str) -> Result<Vec<String>, String> {
    let regex = regex::Regex::new(r#"(?is)<a\b[^>]*?\s+href\s*=\s*(?:"([^"]*)"|'([^']*)')"#)
        .map_err(|e| e.to_string())?;
    Ok(regex
        .captures_iter(html)
        .filter_map(|capture| capture.get(1).or_else(|| capture.get(2)))
        .map(|value| {
            value
                .as_str()
                .replace("&amp;", "&")
                .replace("&#38;", "&")
                .replace("&#x26;", "&")
        })
        .collect())
}

fn file_parameter(url: &Url, download: bool) -> Result<String, String> {
    let mut file = None;
    let mut token = None;
    for (key, value) in url.query_pairs() {
        match key.as_ref() {
            "f" if file.is_none() => file = Some(value.into_owned()),
            "x" if download && token.is_none() => token = Some(value.into_owned()),
            _ => return Err("OptiFine 下载链接包含未知或重复参数。".into()),
        }
    }
    if download
        && !token.as_ref().is_some_and(|token| {
            !token.is_empty()
                && token.len() <= 256
                && token.bytes().all(|b| b.is_ascii_alphanumeric())
        })
    {
        return Err("官方 OptiFine 下载链接缺少有效的临时令牌。".into());
    }
    file.ok_or_else(|| "OptiFine 链接缺少文件名。".into())
}

fn version_from_file(file: &str, game: &str) -> Result<(String, bool), String> {
    super::safe_version(file)?;
    let (file, preview) = match file.strip_prefix("preview_") {
        Some(file) => (file, true),
        None => (file, false),
    };
    let version = file
        .strip_prefix("OptiFine_")
        .and_then(|file| file.strip_suffix(".jar"))
        .ok_or("官方 OptiFine 文件名无效。")?;
    let release = version
        .strip_prefix(&format!("{game}_HD_U_"))
        .filter(|release| !release.is_empty())
        .ok_or("此 OptiFine 文件不匹配 Minecraft 版本或不是 HD U。")?;
    super::safe_version(version)?;
    Ok((version.to_owned(), !preview && !release.contains("pre")))
}

fn parse_releases(html: &str, game: &str) -> Result<Vec<Release>, String> {
    let base = Url::parse(DOWNLOADS).map_err(|e| e.to_string())?;
    let mut seen = HashSet::new();
    let mut releases = Vec::new();
    for href in anchors(html)? {
        let Ok(url) = official_url(&base, &href) else {
            continue;
        };
        if url.path() != "/adloadx" {
            continue;
        }
        let Ok(file) = file_parameter(&url, false) else {
            continue;
        };
        let Ok((version, stable)) = version_from_file(&file, game) else {
            continue;
        };
        if seen.insert(version.clone()) {
            releases.push(Release {
                version,
                file,
                mirror: url,
                stable,
            });
        }
    }
    if releases.is_empty() && !html.contains("OptiFine") {
        return Err("官方 OptiFine 版本页无效或需要人工验证；不会绕过验证。".into());
    }
    Ok(releases)
}

fn parse_download(html: &str, release: &Release) -> Result<Url, String> {
    let mut urls = Vec::new();
    for href in anchors(html)? {
        let Ok(url) = official_url(&release.mirror, &href) else {
            continue;
        };
        if url.path() == "/downloadx"
            && file_parameter(&url, true).is_ok_and(|file| file == release.file)
            && !urls.contains(&url)
        {
            urls.push(url);
        }
    }
    if urls.len() != 1 {
        return Err(
            "官方 OptiFine 页面未提供唯一的公开下载链接，或需要人工验证；不会绕过验证码或登录。"
                .into(),
        );
    }
    Ok(urls.remove(0))
}

async fn cancellable<T>(
    control: &DownloadControl,
    future: impl Future<Output = Result<T, String>>,
) -> Result<T, String> {
    tokio::pin!(future);
    let mut tick = tokio::time::interval(Duration::from_millis(100));
    loop {
        if control.is_cancelled() {
            return Err("OptiFine 安装已取消。".into());
        }
        tokio::select! {
            result = &mut future => return result,
            _ = tick.tick() => {},
        }
    }
}

async fn fetch(
    client: &Client,
    url: &Url,
    limit: usize,
    control: &DownloadControl,
    progress: &Progress,
) -> Result<Vec<u8>, String> {
    cancellable(control, async {
        let mut url = url.clone();
        let mut response = None;
        for _ in 0..=3 {
            let result = client
                .get(url.clone())
                .send()
                .await
                .map_err(|_| "无法连接 OptiFine 官方服务；请检查网络或代理设置。")?;
            if result.status().is_redirection() {
                let location = result
                    .headers()
                    .get(reqwest::header::LOCATION)
                    .and_then(|v| v.to_str().ok())
                    .ok_or("官方 OptiFine 重定向无效。")?;
                let next = official_url(&url, location)?;
                if next.path() != url.path() || next.query() != url.query() {
                    return Err("官方 OptiFine 下载重定向改变了文件或入口，已停止。".into());
                }
                url = next;
                continue;
            }
            response = Some(
                result
                    .error_for_status()
                    .map_err(|_| "OptiFine 官方服务拒绝了请求；不会绕过验证或登录。")?,
            );
            break;
        }
        let mut response = response.ok_or("OptiFine 官方重定向次数过多。")?;
        let total = response.content_length();
        if total.is_some_and(|length| length > limit as u64) {
            return Err("OptiFine 官方响应超过大小限制。".into());
        }
        let is_jar = url.path() == "/downloadx";
        let mut bytes = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| "OptiFine 响应读取失败。")?
        {
            if bytes
                .len()
                .checked_add(chunk.len())
                .is_none_or(|size| size > limit)
            {
                return Err("OptiFine 官方响应超过大小限制。".into());
            }
            bytes.extend_from_slice(&chunk);
            if is_jar {
                emit(
                    progress,
                    DownloadStage::Downloading,
                    bytes.len() as u64,
                    total,
                );
            }
        }
        if total.is_some_and(|length| length != bytes.len() as u64) {
            return Err("OptiFine 下载字节数与服务器声明不符。".into());
        }
        Ok(bytes)
    })
    .await
}

fn emit(progress: &Progress, stage: DownloadStage, received: u64, total: Option<u64>) {
    if let Some(progress) = progress {
        let _ = progress.send(DownloadProgress {
            artifact_id: "optifine:official-installer".into(),
            label: "OptiFine".into(),
            received,
            total,
            source: "https://optifine.net".into(),
            stage,
            task: None,
        });
    }
}

async fn releases(
    client: &Client,
    game: &str,
    control: &DownloadControl,
) -> Result<Vec<Release>, String> {
    let bytes = fetch(
        client,
        &Url::parse(DOWNLOADS).map_err(|e| e.to_string())?,
        HTML_LIMIT,
        control,
        &None,
    )
    .await?;
    let html = std::str::from_utf8(&bytes).map_err(|_| "OptiFine 版本页不是有效文本。")?;
    parse_releases(html, game)
}

pub async fn list_versions(
    game_version: &str,
    settings: &EngineSettings,
) -> Result<Vec<super::LoaderVersion>, String> {
    super::safe_version(game_version)?;
    let client = official_client(settings)?;
    Ok(releases(&client, game_version, &DownloadControl::new())
        .await?
        .into_iter()
        .map(|release| super::LoaderVersion {
            version: release.version,
            stable: release.stable,
        })
        .collect())
}

#[allow(clippy::too_many_arguments)]
pub async fn install(
    root: &Path,
    instance: &InstanceRecord,
    store: &LauncherStore,
    engine: &DownloadEngine,
    version: &str,
    progress: Progress,
    replace: bool,
) -> Result<(), String> {
    super::safe_version(&instance.game_version)?;
    super::safe_version(version)?;
    let expected_loader = format!("optifine@{version}");
    if instance.state != InstanceState::Ready {
        return Err("请先完成原版安装。".into());
    }
    if instance.loader != "vanilla" && !(replace && instance.loader == expected_loader) {
        return Err(
            "OptiFine 当前仅支持独立原版安装，不支持与 Forge/Fabric/其他加载器组合。".into(),
        );
    }
    let replacing = replace && instance.loader == expected_loader;
    check_directory(root)?;
    let directory = instance.version_dir();
    check_directory(&directory)?;
    let _lock = InstallLock::acquire(directory.join(".optifine-install.lock"))?;
    if !replacing && fs::symlink_metadata(directory.join("launch.json")).is_ok() {
        return Err("实例已有独立启动配置；不会覆盖。".into());
    }
    let base_bytes = read_regular(
        &directory.join(format!("{}.json", instance.game_version)),
        16 * 1024 * 1024,
    )?;
    let base: GameVersionMetadata =
        serde_json::from_slice(&base_bytes).map_err(|_| "原版版本配置无效。")?;
    if base.id != instance.game_version
        || base.main_class.as_deref() != Some("net.minecraft.client.main.Main")
    {
        return Err("OptiFine 需要匹配的原版客户端配置。".into());
    }
    let client_bytes = read_regular(
        &directory.join(format!("{}.jar", instance.game_version)),
        CLIENT_LIMIT,
    )?;
    verify_base(&base, &client_bytes)?;
    let control = engine.control();
    control.checkpoint().await.map_err(|e| e.to_string())?;
    let client = official_client(engine.settings())?;
    let release = releases(&client, &instance.game_version, &control)
        .await?
        .into_iter()
        .find(|release| release.version == version)
        .ok_or("指定 OptiFine 版本不在当前官方版本列表中。")?;
    let mirror_html = fetch(&client, &release.mirror, HTML_LIMIT, &control, &None).await?;
    let download = parse_download(
        std::str::from_utf8(&mirror_html).map_err(|_| "OptiFine 下载页无效。")?,
        &release,
    )?;
    emit(&progress, DownloadStage::Connecting, 0, None);
    let installer = fetch(&client, &download, JAR_LIMIT, &control, &progress).await?;
    let inspected = inspect_installer(&installer, &instance.game_version, version)?;
    let work = WorkDirectory::new(&directory)?;
    write_new(&work.0.join("installer.jar"), &installer)?;
    write_new(&work.0.join("client.jar"), &client_bytes)?;
    write_new(&work.0.join("launchwrapper.jar"), &inspected.wrapper)?;
    control.checkpoint().await.map_err(|e| e.to_string())?;
    let java = find_java(root, store)?;
    run_patcher(&java, &work.0, &control, PATCH_TIMEOUT).await?;
    control.checkpoint().await.map_err(|e| e.to_string())?;
    let patched = read_regular(&work.0.join("OptiFine.jar"), JAR_LIMIT)?;
    inspect_runtime(&patched, &instance.game_version, version)?;
    let nonce = uuid::Uuid::new_v4().to_string();
    let relative = format!("optifine/mfmcl-{nonce}");
    let mut profile = make_profile(
        base,
        version,
        &relative,
        &nonce,
        &inspected.wrapper_version,
        patched.len(),
        inspected.wrapper.len(),
    )?;
    profile.extra.insert(
        "mfmclOptifine".into(),
        serde_json::json!({
            "mode": "standaloneVanilla",
            "version": version,
            "verification": VERIFICATION,
            "officialHashVerified": false,
            "source": release.mirror.as_str(),
            // Local fingerprints detect later corruption; they are not publisher digests.
            "installerLocalSha256": hex::encode(Sha256::digest(&installer)),
            "runtimeLocalSha256": hex::encode(Sha256::digest(&patched)),
        }),
    );
    let bytes = serde_json::to_vec_pretty(&profile).map_err(|e| e.to_string())?;
    let parent = root.join("libraries/optifine");
    create_checked_directory(&parent)?;
    let mut artifacts = WorkDirectory::new(&parent)?;
    write_new(&artifacts.0.join("OptiFine.jar"), &patched)?;
    write_new(&artifacts.0.join("launchwrapper.jar"), &inspected.wrapper)?;
    control.checkpoint().await.map_err(|e| e.to_string())?;
    let destination = root.join("libraries").join(&relative);
    fs::rename(&artifacts.0, &destination).map_err(|e| format!("无法发布 OptiFine 运行库：{e}"))?;
    artifacts.0 = destination;
    let published = publish_profile(&directory, &bytes, replacing, || {
        store
            .set_instance_loader(&instance.id, &expected_loader)
            .map_err(|e| e.to_string())
    });
    if let Err(error) = published {
        // If the filesystem itself also refused rollback, do not leave the visible
        // profile referencing artifacts that this guard would otherwise delete.
        if read_regular(&directory.join("launch.json"), 16 * 1024 * 1024)
            .is_ok_and(|current| current == bytes)
        {
            artifacts.0 = PathBuf::new();
        }
        return Err(error);
    }
    // Published launch.json now owns these immutable, installation-specific artifacts.
    artifacts.0 = PathBuf::new();
    emit(
        &progress,
        DownloadStage::Completed,
        installer.len() as u64,
        Some(installer.len() as u64),
    );
    Ok(())
}

fn verify_base(base: &GameVersionMetadata, bytes: &[u8]) -> Result<(), String> {
    let artifact = base
        .downloads
        .get("client")
        .ok_or("原版配置缺少官方客户端校验信息。")?;
    let sha1 = artifact
        .sha1
        .as_deref()
        .filter(|hash| hash.len() == 40 && hash.bytes().all(|b| b.is_ascii_hexdigit()))
        .ok_or("原版客户端缺少有效的 Mojang SHA-1。")?;
    if artifact.size.is_some_and(|size| size != bytes.len() as u64)
        || !hex::encode(Sha1::digest(bytes)).eq_ignore_ascii_case(sha1)
    {
        return Err("原版客户端校验失败；请先补全官方文件。".into());
    }
    inspect_zip(bytes)?;
    Ok(())
}

struct InspectedInstaller {
    wrapper_version: String,
    wrapper: Vec<u8>,
}

fn inspect_zip(bytes: &[u8]) -> Result<HashSet<String>, String> {
    if !bytes.starts_with(b"PK\x03\x04") {
        return Err("OptiFine 响应不是 JAR（可能是 HTML 或验证页面）。".into());
    }
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).map_err(|_| "JAR ZIP 结构无效。")?;
    if zip.len() > 40_000 {
        return Err("JAR 条目数量超过限制。".into());
    }
    let mut names = HashSet::new();
    let mut folded = HashSet::new();
    let mut expanded = 0u64;
    for index in 0..zip.len() {
        let mut entry = zip.by_index(index).map_err(|_| "JAR ZIP 条目无效。")?;
        let name = entry.name().map_err(|e| e.to_string())?.into_owned();
        if entry.enclosed_name().is_none()
            || name.contains(['\\', ':'])
            || name.chars().any(char::is_control)
            || entry
                .unix_mode()
                .is_some_and(|mode| mode & 0o170000 == 0o120000)
        {
            return Err("JAR 包含不安全的路径或符号链接。".into());
        }
        let relative = name.trim_end_matches('/');
        crate::modpacks::safe_pack_path(relative)?;
        if !folded.insert(name.to_ascii_lowercase()) {
            return Err("JAR 包含重复或大小写冲突的条目。".into());
        }
        expanded = expanded
            .checked_add(entry.size())
            .ok_or("JAR 展开大小溢出。")?;
        if entry.size() > ENTRY_LIMIT || expanded > EXPANDED_LIMIT {
            return Err("JAR 展开大小超过限制。".into());
        }
        if !entry.is_dir() {
            let size = entry.size();
            let copied = std::io::copy(
                &mut entry.by_ref().take(ENTRY_LIMIT + 1),
                &mut std::io::sink(),
            )
            .map_err(|_| "JAR 条目读取或 CRC 校验失败。")?;
            if copied != size {
                return Err("JAR 条目大小不符。".into());
            }
        }
        names.insert(name);
    }
    Ok(names)
}

fn zip_bytes(bytes: &[u8], name: &str, limit: u64) -> Result<Vec<u8>, String> {
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).map_err(|_| "JAR ZIP 结构无效。")?;
    let mut entry = zip
        .by_name(name)
        .map_err(|_| format!("官方 JAR 缺少 {name}。"))?;
    if entry.size() > limit {
        return Err("JAR 内嵌文件过大。".into());
    }
    let mut out = Vec::new();
    entry
        .by_ref()
        .take(limit + 1)
        .read_to_end(&mut out)
        .map_err(|_| "JAR 内嵌文件读取失败。")?;
    if out.len() as u64 > limit {
        return Err("JAR 内嵌文件过大。".into());
    }
    Ok(out)
}

fn inspect_version(
    bytes: &[u8],
    names: &HashSet<String>,
    game: &str,
    version: &str,
) -> Result<(), String> {
    let name = [
        "net/optifine/Config.class",
        "Config.class",
        "notch/net/optifine/Config.class",
    ]
    .into_iter()
    .find(|name| names.contains(*name))
    .ok_or("JAR 缺少 OptiFine Config.class。")?;
    let fields = class_string_constants(&zip_bytes(bytes, name, 2 * 1024 * 1024)?)?;
    let mc = fields
        .get("MC_VERSION")
        .ok_or("OptiFine 缺少 MC_VERSION 常量。")?;
    let edition = fields
        .get("OF_EDITION")
        .ok_or("OptiFine 缺少 OF_EDITION 常量。")?;
    let release = fields
        .get("OF_RELEASE")
        .ok_or("OptiFine 缺少 OF_RELEASE 常量。")?;
    if mc != game || format!("{mc}_{edition}_{release}") != version {
        return Err("JAR 内的精确 OptiFine/Minecraft 版本与所选版本不符。".into());
    }
    Ok(())
}

fn inspect_installer(
    bytes: &[u8],
    game: &str,
    version: &str,
) -> Result<InspectedInstaller, String> {
    let names = inspect_zip(bytes)?;
    inspect_version(bytes, &names, game, version)?;
    if !names.contains("optifine/Patcher.class") || !names.contains("patch.cfg") {
        return Err("官方 JAR 缺少受支持的 OptiFine Patcher 安装结构。".into());
    }
    let wrapper_version = String::from_utf8(zip_bytes(bytes, "launchwrapper-of.txt", 256)?)
        .map_err(|_| "OptiFine LaunchWrapper 版本不是有效文本。")?
        .trim()
        .to_owned();
    super::safe_version(&wrapper_version)?;
    let wrapper = zip_bytes(
        bytes,
        &format!("launchwrapper-of-{wrapper_version}.jar"),
        16 * 1024 * 1024,
    )?;
    let names = inspect_zip(&wrapper)?;
    if !names.contains("net/minecraft/launchwrapper/Launch.class") {
        return Err("OptiFine 内嵌 LaunchWrapper 缺少入口类。".into());
    }
    Ok(InspectedInstaller {
        wrapper_version,
        wrapper,
    })
}

fn inspect_runtime(bytes: &[u8], game: &str, version: &str) -> Result<(), String> {
    let names = inspect_zip(bytes)?;
    inspect_version(bytes, &names, game, version)?;
    if !names.contains("optifine/OptiFineTweaker.class") {
        return Err("OptiFine Patcher 没有产出可启动的 OptiFineTweaker。".into());
    }
    Ok(())
}

// Read ConstantValue field attributes rather than merely finding version-like
// strings in the constant pool (unrelated strings are not version declarations).
fn class_string_constants(bytes: &[u8]) -> Result<BTreeMap<String, String>, String> {
    #[derive(Clone)]
    enum Constant {
        Other,
        Utf8(String),
        String(u16),
    }
    struct Reader<'a>(&'a [u8]);
    impl<'a> Reader<'a> {
        fn take(&mut self, size: usize) -> Result<&'a [u8], String> {
            let data = self.0.get(..size).ok_or("Java class 文件被截断。")?;
            self.0 = &self.0[size..];
            Ok(data)
        }
        fn u1(&mut self) -> Result<u8, String> {
            Ok(self.take(1)?[0])
        }
        fn u2(&mut self) -> Result<u16, String> {
            let b = self.take(2)?;
            Ok(u16::from_be_bytes([b[0], b[1]]))
        }
        fn u4(&mut self) -> Result<u32, String> {
            let b = self.take(4)?;
            Ok(u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
        }
    }
    let mut r = Reader(bytes);
    if r.u4()? != 0xcafebabe {
        return Err("OptiFine class 魔数无效。".into());
    }
    r.take(4)?;
    let count = r.u2()? as usize;
    let mut pool = vec![Constant::Other; count];
    let mut index = 1;
    while index < count {
        match r.u1()? {
            1 => {
                let len = r.u2()? as usize;
                pool[index] = Constant::Utf8(String::from_utf8_lossy(r.take(len)?).into_owned());
            }
            8 => pool[index] = Constant::String(r.u2()?),
            3 | 4 | 9 | 10 | 11 | 12 | 17 | 18 => {
                r.take(4)?;
            }
            5 | 6 => {
                r.take(8)?;
                index += 1;
            }
            7 | 16 | 19 | 20 => {
                r.take(2)?;
            }
            15 => {
                r.take(3)?;
            }
            _ => return Err("OptiFine class 常量池标签无效。".into()),
        }
        index += 1;
    }
    let utf8 = |index: u16| -> Result<&str, String> {
        match pool.get(index as usize) {
            Some(Constant::Utf8(value)) => Ok(value),
            _ => Err("OptiFine class 常量池引用无效。".into()),
        }
    };
    r.take(6)?;
    let interfaces = r.u2()? as usize;
    r.take(interfaces * 2)?;
    let fields = r.u2()?;
    let mut out = BTreeMap::new();
    for _ in 0..fields {
        let access = r.u2()?;
        let name = utf8(r.u2()?)?.to_owned();
        let desc = utf8(r.u2()?)?;
        let attrs = r.u2()?;
        for _ in 0..attrs {
            let attr = utf8(r.u2()?)?;
            let len = r.u4()? as usize;
            if attr == "ConstantValue"
                && len == 2
                && desc == "Ljava/lang/String;"
                && access & 0x18 == 0x18
            {
                let value = match pool.get(r.u2()? as usize) {
                    Some(Constant::String(index)) => utf8(*index)?.to_owned(),
                    _ => return Err("OptiFine 字符串常量值无效。".into()),
                };
                if out.insert(name.clone(), value).is_some() {
                    return Err("OptiFine class 包含重复常量字段。".into());
                }
            } else {
                r.take(len)?;
            }
        }
    }
    Ok(out)
}

#[allow(clippy::too_many_arguments)]
fn make_profile(
    mut base: GameVersionMetadata,
    version: &str,
    relative: &str,
    nonce: &str,
    wrapper_version: &str,
    runtime_size: usize,
    wrapper_size: usize,
) -> Result<GameVersionMetadata, String> {
    crate::modpacks::safe_pack_path(relative)?;
    base.main_class = Some("net.minecraft.launchwrapper.Launch".into());
    base.inherits_from = None;
    let tweak = "--tweakClass optifine.OptiFineTweaker";
    if let Some(arguments) = base.arguments.as_mut() {
        arguments.game.extend([
            mc_metadata::ArgumentEntry::Literal("--tweakClass".into()),
            mc_metadata::ArgumentEntry::Literal("optifine.OptiFineTweaker".into()),
        ]);
    } else if let Some(arguments) = base.minecraft_arguments.as_mut() {
        arguments.push(' ');
        arguments.push_str(tweak);
    } else {
        return Err("原版配置缺少启动参数。".into());
    }
    base.libraries.insert(
        0,
        local_library(
            format!("optifine:OptiFine:{version}-{nonce}"),
            format!("{relative}/OptiFine.jar"),
            runtime_size,
        ),
    );
    base.libraries.insert(
        0,
        local_library(
            format!("optifine:launchwrapper-of:{wrapper_version}-{nonce}"),
            format!("{relative}/launchwrapper.jar"),
            wrapper_size,
        ),
    );
    Ok(base)
}

fn local_library(name: String, path: String, size: usize) -> Library {
    Library {
        name,
        downloads: LibraryDownloads {
            artifact: Some(Artifact {
                path: Some(path),
                url: String::new(),
                sha1: None,
                size: Some(size as u64),
            }),
            ..Default::default()
        },
        ..Default::default()
    }
}

fn find_java(root: &Path, store: &LauncherStore) -> Result<PathBuf, String> {
    let preferences = store.load_java_preferences().map_err(|e| e.to_string())?;
    let mut candidates: Vec<PathBuf> = preferences
        .ordered_paths
        .iter()
        .map(PathBuf::from)
        .collect();
    let executable = if cfg!(windows) { "java.exe" } else { "java" };
    if let Some(home) = std::env::var_os("JAVA_HOME") {
        candidates.push(PathBuf::from(home).join("bin").join(executable));
    }
    if let Some(path) = std::env::var_os("PATH") {
        candidates.extend(
            std::env::split_paths(&path)
                .filter(|p| p.is_absolute())
                .map(|p| p.join(executable)),
        );
    }
    if let Ok(managed) = fs::read_dir(root.join("java-runtimes")) {
        for entry in managed.flatten().take(256) {
            if !entry.file_type().is_ok_and(|kind| kind.is_dir()) {
                continue;
            }
            let Ok(marker) = read_regular(&entry.path().join(".mfmcl-complete"), 4096) else {
                continue;
            };
            let Ok(relative) = std::str::from_utf8(&marker) else {
                continue;
            };
            if crate::modpacks::safe_pack_path(relative).is_ok() {
                candidates.push(entry.path().join(relative));
            }
        }
    }
    candidates
        .into_iter()
        .map(|path| crate::java_runtime::java_executable(&path))
        .find(|path| path.is_absolute() && path.is_file())
        .and_then(|path| path.canonicalize().ok())
        .ok_or_else(|| "找不到 Java；请先安装 Java 或配置 Java 路径。".into())
}

async fn run_patcher(
    java: &Path,
    work: &Path,
    control: &DownloadControl,
    timeout: Duration,
) -> Result<(), String> {
    let work = work.canonicalize().map_err(|e| e.to_string())?;
    let mut command = tokio::process::Command::new(java);
    command
        .current_dir(&work)
        .env_remove("JAVA_TOOL_OPTIONS")
        .env_remove("JDK_JAVA_OPTIONS")
        .env_remove("_JAVA_OPTIONS")
        .env_remove("CLASSPATH")
        .env("HOME", &work)
        .env("USERPROFILE", &work)
        .env("APPDATA", &work)
        .env("LOCALAPPDATA", &work)
        .arg("-Xmx512M")
        .arg("-Djava.awt.headless=true")
        .arg(format!("-Duser.home={}", work.display()))
        .arg(format!("-Djava.io.tmpdir={}", work.display()))
        .arg("-cp")
        .arg(work.join("installer.jar"))
        .arg("optifine.Patcher")
        .arg(work.join("client.jar"))
        .arg(work.join("installer.jar"))
        .arg(work.join("OptiFine.jar"));
    run_child(&mut command, control, timeout).await
}

async fn run_child(
    command: &mut tokio::process::Command,
    control: &DownloadControl,
    timeout: Duration,
) -> Result<(), String> {
    if control.is_cancelled() {
        return Err("OptiFine 安装已取消。".into());
    }
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    let mut child = crate::process::hide_console_async(command)
        .spawn()
        .map_err(|e| format!("无法启动 OptiFine Patcher：{e}"))?;
    let result = tokio::time::timeout(
        timeout,
        cancellable(control, async {
            let status = child.wait().await.map_err(|e| e.to_string())?;
            if !status.success() {
                return Err(format!(
                    "官方 OptiFine Patcher 失败（{status}）；原版未修改。"
                ));
            }
            Ok(())
        }),
    )
    .await
    .unwrap_or_else(|_| Err("OptiFine Patcher 超时；原版未修改。".into()));
    if result.is_err() {
        let _ = child.start_kill();
        let _ = tokio::time::timeout(Duration::from_secs(5), child.wait()).await;
    }
    result
}

fn is_link(metadata: &fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        metadata.file_type().is_symlink() || metadata.file_attributes() & 0x400 != 0
    }
    #[cfg(not(windows))]
    {
        metadata.file_type().is_symlink()
    }
}

fn check_directory(path: &Path) -> Result<(), String> {
    let absolute = std::path::absolute(path).map_err(|e| e.to_string())?;
    if absolute
        .components()
        .any(|part| matches!(part, Component::ParentDir))
    {
        return Err("OptiFine 安装目录包含父路径跳转。".into());
    }
    for path in absolute.ancestors() {
        let metadata = fs::symlink_metadata(path).map_err(|e| e.to_string())?;
        if is_link(&metadata) || !metadata.is_dir() {
            return Err("OptiFine 安装路径不能经过符号链接、联接点或普通文件。".into());
        }
    }
    Ok(())
}

fn create_checked_directory(path: &Path) -> Result<(), String> {
    match fs::symlink_metadata(path) {
        Ok(_) => check_directory(path),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let parent = path.parent().ok_or("OptiFine 目录没有父目录。")?;
            create_checked_directory(parent)?;
            match fs::create_dir(path) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(error) => return Err(error.to_string()),
            }
            check_directory(path)
        }
        Err(error) => Err(error.to_string()),
    }
}

fn read_regular(path: &Path, limit: usize) -> Result<Vec<u8>, String> {
    check_directory(path.parent().ok_or("文件没有父目录。")?)?;
    let metadata =
        fs::symlink_metadata(path).map_err(|e| format!("无法读取 {}：{e}", path.display()))?;
    if is_link(&metadata) || !metadata.is_file() || metadata.len() > limit as u64 {
        return Err("OptiFine 输入/产物不是大小受限的普通文件。".into());
    }
    let mut bytes = Vec::new();
    File::open(path)
        .map_err(|e| e.to_string())?
        .take(limit as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() > limit {
        return Err("OptiFine 文件超过大小限制。".into());
    }
    Ok(bytes)
}

fn write_new(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|e| e.to_string())?;
    file.write_all(bytes)
        .and_then(|_| file.sync_all())
        .map_err(|e| e.to_string())
}

struct WorkDirectory(PathBuf);
impl WorkDirectory {
    fn new(parent: &Path) -> Result<Self, String> {
        check_directory(parent)?;
        let path = parent.join(format!(".optifine-work-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&path).map_err(|e| e.to_string())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o700))
                .map_err(|e| e.to_string())?;
        }
        Ok(Self(path))
    }
}
impl Drop for WorkDirectory {
    fn drop(&mut self) {
        if !self.0.as_os_str().is_empty() {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
}
struct InstallLock {
    path: PathBuf,
    file: Option<File>,
}
impl InstallLock {
    fn acquire(path: PathBuf) -> Result<Self, String> {
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(
                |_| "此实例正在安装 OptiFine，或上次安装异常退出留下了 .optifine-install.lock。",
            )?;
        Ok(Self {
            path,
            file: Some(file),
        })
    }
}
impl Drop for InstallLock {
    fn drop(&mut self) {
        drop(self.file.take());
        let _ = fs::remove_file(&self.path);
    }
}

fn publish_profile(
    directory: &Path,
    bytes: &[u8],
    replace: bool,
    commit: impl FnOnce() -> Result<(), String>,
) -> Result<(), String> {
    check_directory(directory)?;
    let target = directory.join("launch.json");
    let temp = directory.join(format!(".optifine-{}.pending", uuid::Uuid::new_v4()));
    let backup = directory.join(format!(".optifine-{}.backup", uuid::Uuid::new_v4()));
    let old = match fs::symlink_metadata(&target) {
        Ok(_) => Some(read_regular(&target, 16 * 1024 * 1024)?),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(error.to_string()),
    };
    if old.is_some() && !replace {
        return Err("已有启动配置；不会覆盖。".into());
    }
    write_new(&temp, bytes)?;
    let mut published = false;
    let result = (|| {
        if old.is_some() {
            fs::hard_link(&target, &backup).map_err(|e| e.to_string())?;
        }
        if replace && old.is_some() {
            // rename replaces atomically on supported local filesystems; no delete gap.
            fs::rename(&temp, &target).map_err(|e| e.to_string())?;
        } else {
            fs::hard_link(&temp, &target).map_err(|e| e.to_string())?;
        }
        published = true;
        commit()
    })();
    if let Err(error) = result {
        if published {
            let rollback = if old.is_some() {
                fs::rename(&backup, &target)
            } else {
                fs::remove_file(&target)
            };
            if let Err(rollback) = rollback {
                let _ = fs::remove_file(&temp);
                return Err(format!(
                    "{error}；回滚失败：{rollback}；旧配置保留在 {}",
                    backup.display()
                ));
            }
        }
        let _ = fs::remove_file(&temp);
        let _ = fs::remove_file(&backup);
        return Err(error);
    }
    let _ = fs::remove_file(&temp);
    let _ = fs::remove_file(&backup);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use zip::write::SimpleFileOptions;

    const GAME: &str = "1.21.4";
    const VERSION: &str = "1.21.4_HD_U_J3";
    const LIST: &str = r#"<html>OptiFine Minecraft
      <a href="http://optifine.net/adloadx?f=preview_OptiFine_1.21.4_HD_U_J4_pre2.jar">(Mirror)</a>
      <a href='adloadx?f=OptiFine_1.21.4_HD_U_J3.jar'>(Mirror)</a>
      <a href='adloadx?f=OptiFine_1.21.4_HD_U_J3.jar'>duplicate</a>
      <a href='adloadx?f=OptiFine_1.21.40_HD_U_J3.jar'>other game</a>
      <a href='https://evil.test/adloadx?f=OptiFine_1.21.4_HD_U_X1.jar'>external</a>
      <a data-href='adloadx?f=OptiFine_1.21.4_HD_U_X2.jar'>not href</a>
    </html>"#;

    fn fixture_directory() -> WorkDirectory {
        let parent = std::env::temp_dir().canonicalize().unwrap();
        WorkDirectory::new(&parent).unwrap()
    }

    fn zip_fixture(entries: Vec<(&str, Vec<u8>)>) -> Vec<u8> {
        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        for (name, bytes) in entries {
            writer
                .start_file(name, SimpleFileOptions::default())
                .unwrap();
            writer.write_all(&bytes).unwrap();
        }
        writer.finish().unwrap().into_inner()
    }

    // A tiny structural class-file fixture; it is never executed or redistributed.
    fn config_class(game: &str, release: &str) -> Vec<u8> {
        let mut constants: Vec<Vec<u8>> = Vec::new();
        fn utf8(constants: &mut Vec<Vec<u8>>, text: &str) -> u16 {
            let mut bytes = vec![1];
            bytes.extend_from_slice(&(text.len() as u16).to_be_bytes());
            bytes.extend_from_slice(text.as_bytes());
            constants.push(bytes);
            constants.len() as u16
        }
        let desc = utf8(&mut constants, "Ljava/lang/String;");
        let attr = utf8(&mut constants, "ConstantValue");
        let mut fields = Vec::new();
        for (name, value) in [
            ("MC_VERSION", game),
            ("OF_EDITION", "HD_U"),
            ("OF_RELEASE", release),
        ] {
            let name = utf8(&mut constants, name);
            let value = utf8(&mut constants, value);
            let mut string = vec![8];
            string.extend_from_slice(&value.to_be_bytes());
            constants.push(string);
            fields.push((name, constants.len() as u16));
        }
        let mut out = Vec::new();
        out.extend_from_slice(&0xcafebabeu32.to_be_bytes());
        out.extend_from_slice(&0u16.to_be_bytes());
        out.extend_from_slice(&52u16.to_be_bytes());
        out.extend_from_slice(&((constants.len() + 1) as u16).to_be_bytes());
        for bytes in constants {
            out.extend(bytes);
        }
        out.extend_from_slice(&[0; 8]);
        out.extend_from_slice(&(fields.len() as u16).to_be_bytes());
        for (name, value) in fields {
            for short in [0x19u16, name, desc, 1, attr] {
                out.extend_from_slice(&short.to_be_bytes());
            }
            out.extend_from_slice(&2u32.to_be_bytes());
            out.extend_from_slice(&value.to_be_bytes());
        }
        out.extend_from_slice(&[0; 4]);
        out
    }

    fn installer_fixture(game: &str, release: &str) -> Vec<u8> {
        let wrapper = zip_fixture(vec![(
            "net/minecraft/launchwrapper/Launch.class",
            b"fixture".to_vec(),
        )]);
        zip_fixture(vec![
            ("net/optifine/Config.class", config_class(game, release)),
            ("optifine/Patcher.class", b"fixture".to_vec()),
            ("patch.cfg", b"fixture".to_vec()),
            ("launchwrapper-of.txt", b"3.0\n".to_vec()),
            ("launchwrapper-of-3.0.jar", wrapper),
        ])
    }

    fn base_fixture(client: &[u8]) -> GameVersionMetadata {
        serde_json::from_value(serde_json::json!({
            "id": GAME, "mainClass": "net.minecraft.client.main.Main",
            "libraries": [], "assets": "fixture",
            "javaVersion": {"component": "java-runtime-delta", "majorVersion": 21},
            "downloads": {"client": {"url": "https://piston-data.mojang.com/fixture.jar",
                "size": client.len(), "sha1": hex::encode(Sha1::digest(client))}},
            "arguments": {"jvm": ["-cp", "${classpath}"], "game": ["--username", "${auth_player_name}"]}
        })).unwrap()
    }

    #[test]
    fn official_list_matches_exact_game_and_keeps_preview_identity() {
        let releases = parse_releases(LIST, GAME).unwrap();
        assert_eq!(releases.len(), 2);
        assert_eq!(releases[0].version, "1.21.4_HD_U_J4_pre2");
        assert!(!releases[0].stable);
        assert!(releases[0].file.starts_with("preview_"));
        assert_eq!(releases[0].mirror.scheme(), "https");
        assert_eq!(releases[1].version, VERSION);
        assert!(releases[1].stable);
        assert!(parse_releases(LIST, "1.20.1").unwrap().is_empty());
        assert!(parse_releases("access denied", GAME).is_err());
    }

    #[test]
    fn download_link_comes_from_the_official_page_including_its_token() {
        let release = parse_releases(LIST, GAME).unwrap().remove(1);
        let html = r#"<a href="downloadx?f=OptiFine_1.21.4_HD_U_J3.jar&amp;x=abc123">Download</a>"#;
        let url = parse_download(html, &release).unwrap();
        assert_eq!(
            url.as_str(),
            "https://optifine.net/downloadx?f=OptiFine_1.21.4_HD_U_J3.jar&x=abc123"
        );
        for bad in [
            "<form>captcha/login</form>",
            r#"<a href="downloadx?f=OptiFine_1.21.4_HD_U_J3.jar">missing token</a>"#,
            r#"<a href="https://evil.test/downloadx?f=OptiFine_1.21.4_HD_U_J3.jar&x=abc">external</a>"#,
            r#"<a href="downloadx?f=OptiFine_1.21.4_HD_U_J2.jar&x=abc">wrong file</a>"#,
            r#"<a href="downloadx?f=OptiFine_1.21.4_HD_U_J3.jar&x=abc&f=other.jar">duplicate</a>"#,
        ] {
            assert!(parse_download(bad, &release).is_err(), "{bad}");
        }
    }

    #[test]
    fn official_url_policy_rejects_credentials_ports_and_foreign_hosts() {
        let base = Url::parse(DOWNLOADS).unwrap();
        for bad in [
            "https://optifine.net.evil.test/downloads",
            "https://user@optifine.net/downloads",
            "https://optifine.net:444/downloads",
            "//evil.test/downloads",
            "file:///tmp/installer.jar",
            "https://optifine.net/downloads#fragment",
        ] {
            assert!(official_url(&base, bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn installer_inspection_checks_precise_class_constants_and_wrapper() {
        let bytes = installer_fixture(GAME, "J3");
        let inspected = inspect_installer(&bytes, GAME, VERSION).unwrap();
        assert_eq!(inspected.wrapper_version, "3.0");
        assert!(
            inspect_zip(&inspected.wrapper)
                .unwrap()
                .contains("net/minecraft/launchwrapper/Launch.class")
        );
        assert!(inspect_installer(&installer_fixture(GAME, "J2"), GAME, VERSION).is_err());
        assert!(inspect_installer(&installer_fixture("1.21.3", "J3"), GAME, VERSION).is_err());
        assert!(
            inspect_installer(b"<html>OptiFine download challenge</html>", GAME, VERSION).is_err()
        );
        let runtime = zip_fixture(vec![
            ("net/optifine/Config.class", config_class(GAME, "J3")),
            ("optifine/OptiFineTweaker.class", b"fixture".to_vec()),
        ]);
        inspect_runtime(&runtime, GAME, VERSION).unwrap();
        assert!(inspect_runtime(&bytes, GAME, VERSION).is_err());
    }

    #[test]
    fn constant_reader_rejects_truncation_not_just_scans_version_strings() {
        let class = config_class(GAME, "J3");
        assert_eq!(class_string_constants(&class).unwrap()["MC_VERSION"], GAME);
        for cut in [0, 1, 8, 12, class.len() - 8] {
            assert!(class_string_constants(&class[..cut]).is_err(), "cut={cut}");
        }
        assert!(
            class_string_constants(b"MC_VERSION 1.21.4 OF_EDITION HD_U OF_RELEASE J3").is_err()
        );
    }

    #[test]
    fn zip_inspection_rejects_path_traversal_and_case_collisions() {
        for entries in [
            vec![("../escape.class", b"x".to_vec())],
            vec![("a\\escape.class", b"x".to_vec())],
            vec![
                ("Config.class", b"x".to_vec()),
                ("config.class", b"y".to_vec()),
            ],
        ] {
            assert!(inspect_zip(&zip_fixture(entries)).is_err());
        }
        let mut bytes = installer_fixture(GAME, "J3");
        bytes.truncate(bytes.len() / 2);
        assert!(inspect_zip(&bytes).is_err());
    }

    #[test]
    fn vanilla_client_requires_mojang_digest_and_matching_bytes() {
        let client = zip_fixture(vec![(
            "net/minecraft/client/main/Main.class",
            b"fixture".to_vec(),
        )]);
        let mut base = base_fixture(&client);
        verify_base(&base, &client).unwrap();
        assert!(verify_base(&base, b"corrupt client").is_err());
        base.downloads.get_mut("client").unwrap().sha1 = None;
        assert!(verify_base(&base, &client).is_err());
    }

    #[test]
    fn separate_profile_preserves_vanilla_and_handles_modern_and_legacy_arguments() {
        let client = zip_fixture(vec![("Main.class", b"fixture".to_vec())]);
        let base = base_fixture(&client);
        let profile = make_profile(
            base.clone(),
            VERSION,
            "optifine/mfmcl-fixture",
            "fixture",
            "3.0",
            10,
            20,
        )
        .unwrap();
        assert_eq!(
            base.main_class.as_deref(),
            Some("net.minecraft.client.main.Main")
        );
        assert_eq!(profile.id, GAME);
        assert_eq!(
            profile.main_class.as_deref(),
            Some("net.minecraft.launchwrapper.Launch")
        );
        assert!(profile.inherits_from.is_none());
        assert_eq!(profile.libraries.len(), 2);
        assert!(
            profile.libraries.iter().all(|library| library
                .downloads
                .artifact
                .as_ref()
                .unwrap()
                .sha1
                .is_none())
        );
        let game = serde_json::to_value(profile.arguments.unwrap().game).unwrap();
        assert_eq!(
            game.as_array().unwrap().last().unwrap(),
            "optifine.OptiFineTweaker"
        );
        let mut legacy = base;
        legacy.arguments = None;
        legacy.minecraft_arguments = Some("--username ${auth_player_name}".into());
        let profile = make_profile(
            legacy,
            VERSION,
            "optifine/mfmcl-fixture",
            "fixture",
            "3.0",
            10,
            20,
        )
        .unwrap();
        assert!(
            profile
                .minecraft_arguments
                .unwrap()
                .ends_with("--tweakClass optifine.OptiFineTweaker")
        );
    }

    #[test]
    fn publishing_failure_restores_old_profile_and_never_changes_original_client() {
        let directory = fixture_directory();
        let client = directory.0.join("1.21.4.jar");
        fs::write(&client, b"original").unwrap();
        let target = directory.0.join("launch.json");
        assert!(publish_profile(&directory.0, b"new", false, || Err("store".into())).is_err());
        assert!(!target.exists());
        fs::write(&target, b"old").unwrap();
        assert!(publish_profile(&directory.0, b"new", false, || Ok(())).is_err());
        assert_eq!(fs::read(&target).unwrap(), b"old");
        assert!(publish_profile(&directory.0, b"new", true, || Err("store".into())).is_err());
        assert_eq!(fs::read(&target).unwrap(), b"old");
        publish_profile(&directory.0, b"committed", true, || Ok(())).unwrap();
        assert_eq!(fs::read(&target).unwrap(), b"committed");
        assert_eq!(fs::read(&client).unwrap(), b"original");
        assert_eq!(fs::read_dir(&directory.0).unwrap().count(), 2);
    }

    #[test]
    fn install_lock_is_exclusive_and_released_on_drop() {
        let directory = fixture_directory();
        let path = directory.0.join(".optifine-install.lock");
        let lock = InstallLock::acquire(path.clone()).unwrap();
        assert!(InstallLock::acquire(path.clone()).is_err());
        drop(lock);
        assert!(InstallLock::acquire(path).is_ok());
    }

    #[cfg(unix)]
    #[test]
    fn symbolic_link_parents_and_outputs_are_rejected() {
        use std::os::unix::fs::symlink;
        let directory = fixture_directory();
        fs::create_dir(directory.0.join("real")).unwrap();
        symlink(directory.0.join("real"), directory.0.join("link")).unwrap();
        assert!(check_directory(&directory.0.join("link")).is_err());
        fs::write(directory.0.join("real/file"), b"x").unwrap();
        symlink(directory.0.join("real/file"), directory.0.join("output")).unwrap();
        assert!(read_regular(&directory.0.join("output"), 100).is_err());
    }

    #[tokio::test]
    async fn configured_proxy_is_used_and_plain_http_is_not_allowed() {
        use crate::catalog::network_tests::MockProxy;
        let proxy = MockProxy::new("502 Bad Gateway", Duration::ZERO).await;
        assert!(
            list_versions(GAME, &proxy.settings(0).to_engine_settings())
                .await
                .is_err()
        );
        let requests = proxy.requests.lock().unwrap();
        assert_eq!(requests.len(), 1);
        assert!(requests[0].starts_with("CONNECT optifine.net:443 HTTP/1.1"));
        drop(requests);
        let client = official_client(&EngineSettings::default()).unwrap();
        assert!(
            client
                .get("http://127.0.0.1:9/")
                .send()
                .await
                .unwrap_err()
                .is_builder()
        );
        let invalid = EngineSettings {
            timeout_sec: 0,
            ..Default::default()
        };
        assert!(official_client(&invalid).is_err());
    }

    #[tokio::test]
    async fn response_size_limits_cover_declared_and_chunked_bodies_without_public_network() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        for response in [
            "HTTP/1.1 200 OK\r\nContent-Length: 1024\r\nConnection: close\r\n\r\n",
            "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n10\r\n0123456789abcdef\r\n0\r\n\r\n",
        ] {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr().unwrap();
            let server = tokio::spawn(async move {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut request = [0; 4096];
                let _ = stream.read(&mut request).await.unwrap();
                stream.write_all(response.as_bytes()).await.unwrap();
            });
            // A private helper is exercised with a loopback-only fixture client;
            // production always uses official_client and parsed official links.
            let client = Client::builder()
                .no_proxy()
                .timeout(Duration::from_secs(3))
                .build()
                .unwrap();
            let url = Url::parse(&format!("http://{address}/fixture")).unwrap();
            let error = fetch(&client, &url, 8, &DownloadControl::new(), &None)
                .await
                .unwrap_err();
            assert!(error.contains("大小限制"), "{error}");
            server.await.unwrap();
        }
    }

    #[tokio::test]
    async fn cancellation_interrupts_stalled_network_work_without_waiting_for_timeout() {
        let control = DownloadControl::new();
        let cancellation = control.clone();
        let cancel = async move {
            cancellation.cancel();
        };
        let work = cancellable(&control, std::future::pending::<Result<(), String>>());
        let (_, result) = tokio::join!(cancel, work);
        assert!(result.unwrap_err().contains("取消"));
    }

    #[cfg(any(unix, windows))]
    fn sleeper() -> tokio::process::Command {
        #[cfg(windows)]
        {
            let mut command = tokio::process::Command::new("powershell.exe");
            command.args([
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                "Start-Sleep -Seconds 30",
            ]);
            command
        }
        #[cfg(unix)]
        {
            let mut command = tokio::process::Command::new("sleep");
            command.arg("30");
            command
        }
    }

    #[cfg(any(unix, windows))]
    #[tokio::test]
    async fn child_timeout_and_cancel_are_bounded_and_reaped() {
        let result = run_child(
            &mut sleeper(),
            &DownloadControl::new(),
            Duration::from_millis(100),
        )
        .await;
        assert!(result.unwrap_err().contains("超时"));
        let control = DownloadControl::new();
        let cancellation = control.clone();
        let cancel = async move {
            tokio::time::sleep(Duration::from_millis(100)).await;
            cancellation.cancel();
        };
        let mut command = sleeper();
        let child = run_child(&mut command, &control, Duration::from_secs(10));
        let (_, result) = tokio::join!(cancel, child);
        assert!(result.unwrap_err().contains("取消"));
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn generated_overlay_can_prepare_launch_with_an_offline_mock_java_probe() {
        let directory = fixture_directory();
        let root = &directory.0;
        let instance = InstanceRecord {
            id: "fixture".into(),
            name: "fixture".into(),
            game_version: GAME.into(),
            loader: format!("optifine@{VERSION}"),
            instance_dir: root.join("instance"),
            game_dir: root.join("game"),
            isolated: true,
            state: InstanceState::Ready,
            created_at: String::new(),
            last_launched_at: None,
        };
        fs::create_dir_all(instance.version_dir()).unwrap();
        fs::create_dir_all(&instance.game_dir).unwrap();
        fs::create_dir_all(root.join("libraries/optifine/mfmcl-fixture")).unwrap();
        let client = zip_fixture(vec![("Main.class", b"fixture".to_vec())]);
        fs::write(instance.version_dir().join(format!("{GAME}.jar")), &client).unwrap();
        let profile = make_profile(
            base_fixture(&client),
            VERSION,
            "optifine/mfmcl-fixture",
            "fixture",
            "3.0",
            1,
            1,
        )
        .unwrap();
        fs::write(
            root.join("libraries/optifine/mfmcl-fixture/OptiFine.jar"),
            b"x",
        )
        .unwrap();
        fs::write(
            root.join("libraries/optifine/mfmcl-fixture/launchwrapper.jar"),
            b"x",
        )
        .unwrap();
        #[cfg(windows)]
        let java = {
            let path = root.join("mock-java.cmd");
            fs::write(
                &path,
                "@echo off\r\necho openjdk version \"21.0.1\"\r\nexit /b 0\r\n",
            )
            .unwrap();
            path
        };
        #[cfg(unix)]
        let java = {
            use std::os::unix::fs::PermissionsExt;
            let path = root.join("mock-java");
            fs::write(&path, "#!/bin/sh\nprintf 'openjdk version \"21.0.1\"\\n'\n").unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
            path
        };
        let account = crate::AccountProfile {
            id: "fixture".into(),
            display_name: "FixturePlayer".into(),
            kind: crate::AccountKind::Offline,
            auth_server: None,
            credential_ref: None,
            selected: false,
            created_at: String::new(),
        };
        let prepared =
            crate::launch::prepare_offline_launch(root, &instance, &account, &profile, Some(java))
                .unwrap();
        assert!(
            prepared
                .args
                .contains(&"net.minecraft.launchwrapper.Launch".into())
        );
        assert!(
            prepared
                .args
                .windows(2)
                .any(|args| args == ["--tweakClass", "optifine.OptiFineTweaker"])
        );
        let classpath = prepared
            .args
            .windows(2)
            .find(|args| args[0] == "-cp")
            .unwrap()[1]
            .clone();
        assert!(classpath.contains("OptiFine.jar"));
        assert!(classpath.contains("launchwrapper.jar"));
        assert!(classpath.contains("1.21.4.jar"));
        assert_eq!(prepared.working_directory, instance.game_dir);
    }
}
