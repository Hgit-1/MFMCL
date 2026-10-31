//! CurseForge API Key 的存放与命令。
//!
//! ## 为什么不把 key 存进数据库或前端
//!
//! key 是用户自己的凭据：泄漏会让他人消耗该用户的配额，也可能被用来冒充。所以
//! 这里沿用账户凭据的同一套做法——存进操作系统的凭据库（Windows Credential
//! Manager / macOS Keychain / Linux Secret Service），数据库只保存「有没有设置过」
//! 这个布尔事实。
//!
//! 前端永远拿不到 key 本身：`curseforge_key_status` 只回一个布尔值，所有需要 key
//! 的请求都由后端自己取出并附加。这样即使前端被注入脚本，也读不到凭据。
//!
//! ## 为什么每个命令都重新读一次 key
//!
//! 缓存 key 到内存会让「用户改了 key」和「后端在用旧 key」之间产生不一致，而这个
//! 不一致只会表现为莫名其妙的 403。凭据库读取是本地操作，代价可以接受。

use launcher_auth::{CredentialStore, Secret};
use launcher_core::{
    LauncherStore,
    curseforge::{self, ContentKind, CurseForgeMod, CurseForgeVersion},
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tauri::{AppHandle, State};

/// 凭据库里的服务名与账户键。与账户凭据同服务、不同账户键，互不干扰。
const SERVICE: &str = "dev.mfmcl.launcher";
const KEY_ACCOUNT: &str = "curseforge-api-key";

/**
 * 构建期注入的默认 CurseForge API Key。
 *
 * 用 `option_env!` 而不是写死在源码里：key 是凭据，写进源码就会永久留在 git
 * 历史中，任何拿到仓库的人都能用它消耗配额。这里只接受构建时通过环境变量
 * `MFMCL_CURSEFORGE_API_KEY` 提供的值（见 `.env.example` 与 `scripts/tauri-env.sh`），
 * 仓库本身不含任何 key。
 *
 * 运行时优先级：用户自己保存的 key > 这个默认 key。用户一旦在设置里填过 key，
 * 就完全以他的为准；「清除」也只是回到默认 key，而不是让 CurseForge 不可用。
 */
const DEFAULT_KEY: Option<&str> = option_env!("MFMCL_CURSEFORGE_API_KEY");

fn vault() -> Result<CredentialStore, String> {
    CredentialStore::new(SERVICE).map_err(|error| error.to_string())
}

/// 读取实际生效的 key：先看凭据库，再退回构建期默认值。
///
/// 返回 `None` 表示两者都没有，此时 CurseForge 功能应提示用户去设置。
async fn resolve_key() -> Option<String> {
    let stored = tauri::async_runtime::spawn_blocking(|| {
        let store = vault().ok()?;
        store
            .get(KEY_ACCOUNT)
            .ok()
            .flatten()
            .map(|secret| secret.expose().to_owned())
    })
    .await
    .ok()
    .flatten();
    stored
        .or_else(|| DEFAULT_KEY.map(str::to_owned))
        .filter(|key| !key.trim().is_empty())
}

/// 取一个必须存在的 key；没有时给出可操作的错误。
async fn read_key() -> Result<String, String> {
    resolve_key()
        .await
        .ok_or_else(|| "尚未设置 CurseForge API Key。".to_owned())
}

/// 供其他模块（整合包导入）读取 key。
///
/// key 必须留在后端：CurseForge 格式的整合包在导入时需要按 fileID 批量查询下载
/// 地址，这个动作发生在 `management::import_modpack_inner` 里。把 key 传给前端
/// 再传回来既多一次 IPC，又让凭据暴露在 web view 能读到的地方。
pub(crate) async fn read_key_for_internal() -> Option<String> {
    resolve_key().await
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KeyStatus {
    /// 是否已保存 key。前端只据此决定显示「设置」还是「已设置」。
    pub configured: bool,
    /// 未设置时的原因（凭据库不可用、读取出错）。
    pub problem: Option<String>,
    /**
     * key 的来源：`"user"`（用户自己保存的）、`"builtin"`（构建期注入的默认 key）
     * 或 `None`（都没有）。
     *
     * 界面需要知道这件事才能给出正确的提示：用默认 key 时不该显示「你已配置」，
     * 也不能让「清除」看起来像是把功能关掉了——它只是回到默认 key。
     */
    #[serde(default)]
    pub source: Option<KeySource>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum KeySource {
    User,
    Builtin,
}

/// 只回答「有没有 key」，不返回 key 本身。
#[tauri::command]
pub async fn curseforge_key_status() -> Result<KeyStatus, String> {
    let user_saved = tauri::async_runtime::spawn_blocking(|| {
        // 凭据库不可用（例如无 Secret Service 的 Linux 容器）要如实报告，
        // 而不是把它说成「没有设置过」。
        let store = match vault() {
            Ok(store) => store,
            Err(problem) => return (false, Some(problem)),
        };
        match store.get(KEY_ACCOUNT) {
            Ok(Some(_)) => (true, None),
            Ok(None) => (false, None),
            Err(error) => (false, Some(error.to_string())),
        }
    })
    .await
    .map_err(|error| error.to_string())?;

    let (configured, problem) = user_saved;
    if configured {
        return Ok(KeyStatus {
            configured: true,
            problem,
            source: Some(KeySource::User),
        });
    }
    // 没有用户 key 时看构建期默认值：有就等同「可用」，只是来源不同。
    match DEFAULT_KEY.filter(|key| !key.trim().is_empty()) {
        Some(_) => Ok(KeyStatus {
            configured: true,
            problem,
            source: Some(KeySource::Builtin),
        }),
        None => Ok(KeyStatus {
            configured: false,
            problem,
            source: None,
        }),
    }
}

/// 保存并验证 key。
///
/// 先验证再保存：存进一个必然失败的 key 只会让用户在之后每次搜索时遇到 403，
/// 却不知道问题出在 key 上。验证失败时不写入，凭据库里保持原状。
#[tauri::command]
pub async fn curseforge_save_key(key: String) -> Result<(), String> {
    let trimmed = key.trim().to_owned();
    curseforge::verify_key(&trimmed).await?;
    tauri::async_runtime::spawn_blocking(move || {
        vault()?
            .put(KEY_ACCOUNT, &Secret::new(trimmed))
            .map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
pub async fn curseforge_clear_key() -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(|| {
        vault()?
            .delete(KEY_ACCOUNT)
            .map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
pub async fn curseforge_search(
    kind: ContentKind,
    query: String,
    game_version: Option<String>,
    loader: Option<String>,
    index: Option<u32>,
    page_size: Option<u32>,
) -> Result<Vec<CurseForgeMod>, String> {
    let key = read_key().await?;
    curseforge::search(
        &key,
        kind,
        &query,
        game_version.as_deref(),
        loader.as_deref(),
        index.unwrap_or(0),
        page_size.unwrap_or(20),
    )
    .await
}

#[tauri::command]
pub async fn curseforge_files(
    project_id: String,
    game_version: String,
    loader: Option<String>,
) -> Result<Vec<CurseForgeVersion>, String> {
    let key = read_key().await?;
    curseforge::files(&key, &project_id, &game_version, loader.as_deref()).await
}

/// 安装一个 CurseForge 文件。
///
/// 与 Modrinth 的安装命令保持同一套任务上报：开始登记下载任务、推单文件进度、
/// 结束时标记成功或失败。这样下载页与侧栏徽章对两个来源的行为一致。
#[tauri::command]
pub async fn curseforge_install(
    instance_id: String,
    version: CurseForgeVersion,
    kind: ContentKind,
    app: AppHandle,
    store: State<'_, LauncherStore>,
    operations: State<'_, crate::management::Operations>,
    downloads: State<'_, Arc<crate::downloads::Downloads>>,
) -> Result<String, String> {
    let _guard = operations.acquire(&instance_id)?;
    let target = crate::management::instance(&store, &instance_id)?;
    let control = downloads.start(
        &instance_id,
        &format!("安装 {} {}", kind_label_cn(kind), version.file_name),
        &store,
        &app,
    )?;
    let engine = crate::management::engine(&store)?.with_control(control);
    let (tx, progress_task) =
        crate::management::task_progress(&instance_id, &app, store.inner(), downloads.inner());
    let result = curseforge::install(&engine, &target, &version, kind).await;
    drop(tx);
    let _ = progress_task.await;
    downloads.finish(&instance_id, result.is_ok(), &store, &app)?;
    result
}

/// 直接把文件下载到用户指定的目录，不装进实例。
///
/// 与 [`curseforge_install`] 共用任务上报与校验逻辑，区别只有落盘位置：
/// 这类下载不属于任何实例的安装，任务仍然记在实例下，便于在下载页看到来源。
#[tauri::command]
pub async fn curseforge_download(
    instance_id: String,
    version: CurseForgeVersion,
    kind: ContentKind,
    directory: String,
    app: AppHandle,
    store: State<'_, LauncherStore>,
    operations: State<'_, crate::management::Operations>,
    downloads: State<'_, Arc<crate::downloads::Downloads>>,
) -> Result<String, String> {
    let _guard = operations.acquire(&instance_id)?;
    let target = std::path::PathBuf::from(directory);
    if target.as_os_str().is_empty() {
        return Err("请选择下载目录。".into());
    }
    let control = downloads.start(
        &instance_id,
        &format!("下载 {} {}", kind_label_cn(kind), version.file_name),
        &store,
        &app,
    )?;
    let engine = crate::management::engine(&store)?.with_control(control);
    let (tx, progress_task) =
        crate::management::task_progress(&instance_id, &app, store.inner(), downloads.inner());
    let result = curseforge::download_to(&engine, &version, kind, &target).await;
    drop(tx);
    let _ = progress_task.await;
    downloads.finish(&instance_id, result.is_ok(), &store, &app)?;
    result
}

/// 任务标题用的中文类型名。与 `launcher-core` 里的文案保持一致，避免下载页
/// 出现「安装模组 xx.zip」这种与内容不符的标题。
fn kind_label_cn(kind: ContentKind) -> &'static str {
    match kind {
        ContentKind::Mod => "模组",
        ContentKind::Modpack => "整合包",
        ContentKind::ResourcePack => "资源包",
        ContentKind::World => "地图",
    }
}
