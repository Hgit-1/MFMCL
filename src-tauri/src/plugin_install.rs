//! Tauri commands for installing, removing, and sourcing plugin packages.
//!
//! ## Split from `plugin_package`
//!
//! `plugin_package` owns the archive handling and knows nothing about Tauri.
//! This module is the thin boundary: it resolves the plugin root from the app's
//! data directory, builds the HTTP client with the policy the project uses
//! everywhere else, and translates errors into the strings the UI shows.
//!
//! Keeping the split matters for tests: the archive rules are the part that must
//! not drift, and they are exercised without a Tauri context.

use crate::plugin_package::{self, PackageError};
use launcher_core::LauncherStore;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tauri::{Manager, State};

/// Directory under the app data root that holds installed plugins.
///
/// A subdirectory rather than the app data root itself: the root also holds the
/// database, the aria2 workdir and the model cache, and a plugin called
/// `mfmcl.sqlite` would otherwise be a directory-collision waiting to happen.
const PLUGIN_DIR: &str = "plugins";

fn plugin_root(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    let base = app
        .path()
        .app_data_dir()
        .map_err(|error| error.to_string())?;
    let root = base.join(PLUGIN_DIR);
    std::fs::create_dir_all(&root).map_err(|error| format!("无法创建插件目录：{error}"))?;
    Ok(root)
}

/// Let the webview read files under one plugin's directory, and nowhere else.
///
/// ## Why the scope is granted per plugin instead of up front
///
/// The asset protocol is how a plugin's own artwork reaches an `<img>` tag. Tauri
/// checks every asset request against a scope, and the scope is a list of
/// allowed paths. The tempting shortcut — allow the whole app data directory
/// once — would also expose `mfmcl.sqlite` (accounts, launch history) to any
/// markup a plugin manages to inject. Granting each installed plugin's own
/// directory keeps the blast radius to "a plugin can read its own files".
///
/// ## Why it is re-granted on every list/reload
///
/// The scope lives for the process, and the set of installed plugins changes at
/// runtime (install, remove, the user deleting a folder). Re-granting on each
/// read makes the two agree without a watcher. `allow_directory` is idempotent.
fn allow_asset_scope(app: &tauri::AppHandle, directory: &Path) {
    // `Manager::asset_protocol_scope` is the supported accessor; it exists only
    // with the `protocol-asset` feature, which `tauri.conf.json` now requires.
    let scope = app.asset_protocol_scope();
    // Recursive: a plugin may put images in `assets/` subdirectories.
    let _ = scope.allow_directory(directory, true);
}

/// The HTTP client used for plugin downloads.
///
/// ## Policy, and why each part of it is here
///
///   - **`https_only(true)`**: the transport rule is enforced by the client, not
///     only by the URL check in `fetch_package`. Both exist because the URL check
///     catches the caller passing `http://`, and this catches a redirect to it.
///   - **`redirect(Policy::limited(3))`**: plugin sources on GitHub redirect
///     (release assets go to objects.githubusercontent.com), so redirects must
///     work; but an unbounded chain lets a source bounce the request through
///     ten hosts, and each hop is another chance to be downgraded.
///   - **`referer(false)`**: nothing about the plugin system needs the launcher
///     to leak where a request came from.
///   - **Timeouts**: a source that accepts the connection and then stalls must
///     not hang the install dialog forever.
fn http_client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .user_agent(concat!("MFMCL/", env!("CARGO_PKG_VERSION")))
        .https_only(true)
        .redirect(reqwest::redirect::Policy::limited(3))
        .referer(false)
        .connect_timeout(std::time::Duration::from_secs(15))
        .timeout(std::time::Duration::from_secs(120))
        .build()
        .map_err(|error| format!("无法创建网络客户端：{error}"))
}

fn package_error(error: PackageError) -> String {
    error.to_string()
}

// ---------------------------------------------------------------------------
// Installed plugins
// ---------------------------------------------------------------------------

/// One installed plugin, as the frontend sees it.
///
/// `manifest` is the raw JSON rather than a typed record: the frontend already
/// owns `normalizePlugin`, which validates every display field and localises it.
/// Typing it twice would create two definitions of "a valid plugin", and the one
/// in Rust would be the one that goes stale.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstalledPluginRecord {
    pub id: String,
    pub version: String,
    /// Directory name under the plugin root; also the on-disk id.
    pub directory: String,
    pub manifest: serde_json::Value,
    /// Absolute path to the package directory, so the UI can offer "show files".
    pub path: String,
    /// Base URL the webview can load this plugin's files from.
    ///
    /// `asset://localhost/<path>` on macOS/Linux and `http://asset.localhost/<path>`
    /// on Windows; Tauri rewrites the form per platform, so the frontend asks for
    /// this instead of building the URL itself. Empty until the directory has been
    /// added to the asset scope.
    #[serde(rename = "assetBase")]
    pub asset_base: String,
}

/// Build the record for one installed plugin, granting its asset scope first.
fn record(
    app: &tauri::AppHandle,
    root: &Path,
    plugin: &plugin_package::InstalledPlugin,
) -> Option<InstalledPluginRecord> {
    let (manifest, directory) = plugin_package::read_installed(root, &plugin.id).ok()?;
    allow_asset_scope(app, &directory);
    let asset_base = asset_url(&directory);
    Some(InstalledPluginRecord {
        id: plugin.id.clone(),
        version: plugin.version.clone(),
        directory: plugin.id.clone(),
        manifest,
        path: directory.to_string_lossy().into_owned(),
        asset_base,
    })
}

/// The asset-protocol URL for a directory, with a trailing slash.
///
/// Encoding: the path goes into the URL verbatim. Tauri decodes it before the
/// scope check, so a space or a non-ASCII character in the app data path
/// (a Windows user profile like `C:\Users\张三`) would otherwise produce a URL
/// that never matches. Percent-encoding only the characters that cannot appear
/// raw keeps it readable in a debugger while still round-tripping.
fn asset_url(directory: &Path) -> String {
    let raw = directory.to_string_lossy().replace('\\', "/");
    let mut encoded = String::with_capacity(raw.len() + 8);
    for byte in raw.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'/' | b':' | b'-' | b'.' | b'_' | b'~' => {
                encoded.push(byte as char)
            }
            _ => encoded.push_str(&format!("%{byte:02X}")),
        }
    }
    if !encoded.ends_with('/') {
        encoded.push('/');
    }
    if cfg!(windows) {
        format!("http://asset.localhost/{encoded}")
    } else {
        format!("asset://localhost/{encoded}")
    }
}

#[tauri::command]
pub fn plugin_list_installed(app: tauri::AppHandle) -> Result<Vec<InstalledPluginRecord>, String> {
    let root = plugin_root(&app)?;
    let mut records = Vec::new();
    for installed in plugin_package::scan_installed(&root) {
        // A directory that scanned cleanly but cannot be re-read is a race with
        // another process removing it; skipping is correct and reporting would
        // make the whole list fail for one stale entry.
        if let Some(record) = record(&app, &root, &installed) {
            records.push(record);
        }
    }
    Ok(records)
}

/// Install a package from a local file the user picked.
///
/// The path comes from a native dialog, so it is a real path the user chose.
/// It is still checked: only regular files, and only extensions that can be a
/// ZIP. A user pointing this at `C:\Windows\System32` by mistake should get an
/// error, not a confusing zip parse failure.
#[tauri::command]
pub fn plugin_install_file(
    app: tauri::AppHandle,
    path: String,
) -> Result<InstalledPluginRecord, String> {
    let source = PathBuf::from(&path);
    let metadata = std::fs::symlink_metadata(&source)
        .map_err(|error| format!("无法读取选中的文件：{error}"))?;
    if metadata.file_type().is_symlink() || !metadata.file_type().is_file() {
        return Err("请选择一个插件包文件。".into());
    }
    let extension = source
        .extension()
        .map(|value| value.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    if !matches!(extension.as_str(), "zip" | "mflp" | "mfplugin") {
        return Err(format!(
            "不支持的文件类型「.{extension}」；插件包应为 .zip、.mflp 或 .mfplugin。"
        ));
    }
    let bytes = std::fs::read(&source).map_err(|error| format!("无法读取插件包：{error}"))?;
    install_bytes(&app, &bytes)
}

/// Install a package fetched from an HTTPS URL.
///
/// `expected_sha256` comes from a source index. When present the bytes are checked
/// before anything is unpacked; when absent (a URL the user typed from a README)
/// the install proceeds on transport trust alone, which is what the user asked
/// for by pasting it.
#[tauri::command]
pub async fn plugin_install_url(
    app: tauri::AppHandle,
    url: String,
    expected_sha256: Option<String>,
) -> Result<InstalledPluginRecord, String> {
    let client = http_client()?;
    let bytes = plugin_package::fetch_package(&client, &url)
        .await
        .map_err(package_error)?;
    if let Some(expected) = expected_sha256
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    {
        plugin_package::verify_package_sha256(&bytes, expected).map_err(package_error)?;
    }
    install_bytes(&app, &bytes)
}

fn install_bytes(app: &tauri::AppHandle, bytes: &[u8]) -> Result<InstalledPluginRecord, String> {
    let root = plugin_root(app)?;
    let installed = plugin_package::install_package(&root, bytes).map_err(package_error)?;
    record(app, &root, &installed).ok_or_else(|| "插件已解压，但清单无法重新读取。".to_string())
}

/// Remove one installed plugin.
#[tauri::command]
pub fn plugin_remove_installed(
    app: tauri::AppHandle,
    store: State<'_, LauncherStore>,
    id: String,
) -> Result<(), String> {
    let root = plugin_root(&app)?;
    plugin_package::remove_plugin(&root, &id).map_err(package_error)?;

    /*
     * Drop the plugin's persisted state too.
     *
     * Leaving the `enabled` and `options` rows behind would be invisible — the UI
     * renders from the installed list — but if the user later installs a plugin
     * that reuses the id (a fork, a re-release), the old switch state would
     * silently apply to it. Removing state with the plugin is the only way the
     * two stay in sync.
     */
    let mut state = store
        .load_plugin_state()
        .map_err(|error| error.to_string())?;
    state.custom.retain(|plugin| plugin.id != id);
    state.enabled.remove(&id);
    store
        .save_plugin_state(&state)
        .map_err(|error| error.to_string())
}

/// Re-read every installed plugin from disk.
///
/// ## Why this is a command rather than a frontend refresh
///
/// The frontend can call `plugin_list_installed` again, but "reload" also has to
/// reconcile the *backend* plugin state: a package the user deleted by hand, or
/// one whose manifest changed on disk, would otherwise keep contributing to
/// launches from the stale database row. Doing both in one call means the two
/// halves cannot disagree about what is installed.
///
/// Returns the full list so the caller does not need a second round trip.
#[tauri::command]
pub fn plugin_reload(
    app: tauri::AppHandle,
    store: State<'_, LauncherStore>,
) -> Result<Vec<InstalledPluginRecord>, String> {
    let root = plugin_root(&app)?;
    let installed = plugin_package::scan_installed(&root);
    let ids: Vec<String> = installed.iter().map(|plugin| plugin.id.clone()).collect();

    let mut state = store
        .load_plugin_state()
        .map_err(|error| error.to_string())?;
    // Remove contributions for plugins that are gone. Enabling state is kept only
    // for ids that still exist — same reasoning as the single-plugin removal.
    state.custom.retain(|plugin| ids.contains(&plugin.id));
    let known: std::collections::HashSet<&String> = ids.iter().collect();
    state.enabled.retain(|id, _| known.contains(id));
    store
        .save_plugin_state(&state)
        .map_err(|error| error.to_string())?;

    let mut records = Vec::new();
    for plugin in installed {
        if let Some(record) = record(&app, &root, &plugin) {
            records.push(record);
        }
    }
    Ok(records)
}

// ---------------------------------------------------------------------------
// Plugin sources
// ---------------------------------------------------------------------------

/// One source index entry, as delivered by a source's `index.json`.
///
/// Deliberately smaller than the on-disk manifest: an index describes *where to
/// get* a plugin, not what the plugin is. The authoritative manifest is the one
/// inside the package the user installs, so a compromised index cannot change
/// what a plugin is allowed to do — only what it points at, which is why the URL
/// is validated again on download.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IndexEntry {
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub author: String,
    pub version: String,
    /// HTTPS URL of the package archive.
    pub download: String,
    #[serde(default)]
    pub homepage: Option<String>,
    /// SHA-256 of the exact package, lowercase hex.
    ///
    /// ## Why this is checked during install rather than at index time
    ///
    /// The index is delivered over TLS but is not signed. A hash *inside* the same
    /// document as the URL does not stop a compromised index from pointing at a
    /// different file — but it does stop the far more common failure: a release
    /// asset that was replaced, a URL that now serves a newer build, a proxy that
    /// injected an HTML error page. Those all produce bytes that do not match, and
    /// the install refuses instead of unpacking them.
    ///
    /// Optional so an index without pins still works; when absent the install is
    /// as trustworthy as the transport.
    #[serde(default)]
    pub sha256: Option<String>,
    #[serde(default)]
    pub permissions: Vec<String>,
    #[serde(default)]
    pub hooks: Vec<String>,
    /// Launcher version below which this plugin should not be offered.
    ///
    /// Advisory only: the launcher compares it against its own version to grey out
    /// the entry with an explanation. It is not a security control, and treating
    /// it as one would be wrong — the package still installs if the user insists,
    /// because the entries here are pointers, not policy.
    #[serde(default)]
    pub min_launcher_version: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceIndex {
    /// Format marker. Refusing an unknown schema is what lets the format evolve
    /// without silently misreading an old index.
    #[serde(default)]
    pub schema: u32,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub plugins: Vec<IndexEntry>,
}

/// Ceiling on an index document. It is a list of plugins, and a few hundred
/// entries is already implausible for a curated source.
const MAX_INDEX_BYTES: u64 = 2 * 1024 * 1024;

/// Fetch and validate a source index.
///
/// ## What is checked, and why each check exists
///
///   - **HTTPS on the index URL**, before the request.
///   - **HTTPS after redirects**, so a redirect cannot downgrade the transport.
///   - **Bounded body**, while streaming, so a hostile source cannot make the
///     launcher buffer an unbounded document.
///   - **`schema` must be 1**, so a future format is rejected with a clear
///     message instead of being half-parsed.
///   - **Every `download` must be HTTPS**, at index time so the user sees the
///     dead entry before clicking install rather than after.
///
/// The index is data, not code: nothing here executes anything from it.
#[tauri::command]
pub async fn plugin_fetch_index(url: String) -> Result<SourceIndex, String> {
    let parsed = reqwest::Url::parse(&url).map_err(|_| "插件源地址格式无效。".to_string())?;
    if parsed.scheme() != "https" {
        return Err("插件源必须使用 HTTPS 地址。".into());
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err("插件源地址不能包含账号信息。".into());
    }

    let client = http_client()?;
    let response = client
        .get(parsed)
        .send()
        .await
        .map_err(|error| format!("无法连接插件源：{error}"))?
        .error_for_status()
        .map_err(|error| format!("插件源返回错误：{error}"))?;

    if response.url().scheme() != "https" {
        return Err("插件源被重定向到非 HTTPS 地址，已拒绝。".into());
    }
    if let Some(length) = response.content_length()
        && length > MAX_INDEX_BYTES
    {
        return Err("插件源索引过大。".into());
    }

    let mut buffer: Vec<u8> = Vec::new();
    let mut response = response;
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|error| format!("读取插件源索引中断：{error}"))?
    {
        if buffer.len() as u64 + chunk.len() as u64 > MAX_INDEX_BYTES {
            return Err("插件源索引过大。".into());
        }
        buffer.extend_from_slice(&chunk);
    }

    let index: SourceIndex =
        serde_json::from_slice(&buffer).map_err(|error| format!("插件源索引格式错误：{error}"))?;
    if index.schema != 1 {
        return Err(format!(
            "不支持的插件源格式版本 {}（本启动器支持 1）。",
            index.schema
        ));
    }
    for entry in &index.plugins {
        plugin_package::validate_entry(entry)
            .map_err(|error| format!("插件源条目「{}」无效：{error}", entry.id))?;
    }
    Ok(index)
}

/// Absolute path of a plugin's directory, for "show me the files".
#[tauri::command]
pub fn plugin_directory(app: tauri::AppHandle) -> Result<String, String> {
    let root = plugin_root(&app)?;
    Ok(root.to_string_lossy().into_owned())
}
