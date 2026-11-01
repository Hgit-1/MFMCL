//! Fabric API 的自动安装。
//!
//! ## 为什么单独做一个模块
//!
//! Fabric API 不是一个「可选模组」，而是绝大多数 Fabric 模组的前置依赖：装了
//! Sodium、Iris、Mod Menu 而不装它，游戏会在启动时因为找不到 `net.fabricmc.api`
//! 直接崩溃，崩溃日志里只有一段 `NoClassDefFoundError`，与「我装了什么」很难对应。
//!
//! 而用户往往不知道这件事——从原版转到 Fabric 时，启动器只帮他装了加载器本身。
//! PCL2 之类的启动器会在装完 Fabric 后直接询问要不要顺便装 API，这里补齐这一步。
//!
//! ## 只做「装最新匹配版」
//!
//! 不做版本选择界面：Fabric API 的版本策略是「对应一个 MC 版本范围的最新构建
//! 总是正确的」，不存在「我要旧版 API」的正常需求。给出选择只会让用户困惑。
//! 需要特定版本的少数人会自己去 Modrinth 下载。
//!
//! ## 已装检测看文件名
//!
//! 不解析 jar 内的 `fabric.mod.json`——那需要解压并信任文件内容。Fabric API 的
//! 官方文件名固定是 `fabric-api-<version>+<mc>.jar`，按前缀匹配足够可靠，而且
//! 误判的后果只是「提示已安装」，用户仍可在模组列表里确认。

use crate::{InstanceRecord, LauncherStore};
use download_engine::{DownloadEngine, DownloadProgress};
use std::path::Path;
use tokio::sync::mpsc;

use crate::catalog::{self, CatalogVersion};

/// Fabric API 在 Modrinth 上的项目 id。
///
/// 用 id 而不是 slug：slug 可以被作者改，id 不会。
pub const FABRIC_API_PROJECT: &str = "P7dR8mSH";

/// 官方文件名前缀。用于判断「这个实例是否已经装了 API」。
const FABRIC_API_PREFIX: &str = "fabric-api-";

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FabricApiStatus {
    /// 这个实例是否可能装 Fabric API（只看加载器）。
    pub applicable: bool,
    /// mods 目录里是否已经存在 Fabric API。
    pub installed: bool,
    /// 已装的文件名（`installed` 为真时有值），便于界面直接显示。
    pub installed_file: Option<String>,
    /// 是否找到与该实例匹配的可用版本。
    pub available: bool,
    /// 将要安装的版本号（`available` 为真时有值）。
    pub version: Option<String>,
}

/// 判断实例是否用 Fabric 系加载器（Fabric 或 Quilt）。
///
/// Quilt 也接受 Fabric API：Quilt 加载器向下兼容 Fabric 模组，而 Quilt 自己的
/// API 尚未被广泛使用。把 Quilt 排除会让大部分 Quilt 整合包缺前置。
fn uses_fabric_family(instance: &InstanceRecord) -> bool {
    matches!(
        instance.loader.split('@').next().unwrap_or(""),
        "fabric" | "quilt"
    )
}

/// 在实例的 mods 目录里找 Fabric API。
fn installed_api_file(instance: &InstanceRecord) -> Option<String> {
    let mods = crate::mods::list_mods(instance).ok()?;
    mods.iter()
        .map(|item| item.file_name.clone())
        .find(|name| name.to_ascii_lowercase().starts_with(FABRIC_API_PREFIX))
}

/// 查询 Fabric API 的可用版本（按游戏版本 + `fabric` 加载器）。
///
/// 固定用 `fabric` 筛选而不是实例自己的加载器名：Quilt 实例的加载器名是
/// `quilt`，而 Modrinth 上 Fabric API 只标了 `fabric`，用 quilt 去筛会得到空
/// 列表。Quilt 向下兼容 Fabric 模组，所以这样查既对 Fabric 对 Quilt 也成立。
async fn fabric_api_versions(
    client: &reqwest::Client,
    instance: &InstanceRecord,
) -> Result<Vec<CatalogVersion>, String> {
    catalog::project_versions_for_loader(
        client,
        FABRIC_API_PROJECT,
        &instance.game_version,
        "fabric",
    )
    .await
}
pub async fn fabric_api_status(instance: &InstanceRecord) -> Result<FabricApiStatus, String> {
    if !uses_fabric_family(instance) {
        return Ok(FabricApiStatus {
            applicable: false,
            installed: false,
            installed_file: None,
            available: false,
            version: None,
        });
    }
    let installed_file = installed_api_file(instance);
    let installed = installed_file.is_some();

    let client = catalog::catalog_client()?;
    let versions = fabric_api_versions(&client, instance)
        .await
        .unwrap_or_default();
    // 列表按发布时间倒序（Modrinth 的默认顺序），取第一个即最新匹配版。
    let latest: Option<&CatalogVersion> = versions.first();

    Ok(FabricApiStatus {
        applicable: true,
        installed,
        installed_file,
        available: latest.is_some(),
        version: latest.map(|item| item.version_number.clone()),
    })
}

/// 安装 Fabric API 到实例的 mods 目录。
///
/// 返回落盘的文件名。已存在同名文件时不会覆盖（由 `install_mod_version` 保证），
/// 所以重复调用是安全的——界面不需要自己防抖。
pub async fn install_fabric_api(
    instance: &InstanceRecord,
    engine: &DownloadEngine,
    progress: Option<mpsc::UnboundedSender<DownloadProgress>>,
) -> Result<String, String> {
    if !uses_fabric_family(instance) {
        return Err("只有 Fabric 或 Quilt 实例需要 Fabric API。".into());
    }
    let client = catalog::catalog_client()?;
    let versions = fabric_api_versions(&client, instance).await?;
    let version = versions.into_iter().next().ok_or_else(|| {
        format!(
            "Minecraft {} 没有可用的 Fabric API。",
            instance.game_version
        )
    })?;
    let mods_dir = instance.game_dir.join("mods");
    tokio::fs::create_dir_all(&mods_dir)
        .await
        .map_err(|error| format!("无法创建 mods 目录：{error}"))?;
    let _ = &progress; // 进度由调用方的下载任务推送；这里保持签名一致。
    catalog::install_mod_version(&version, &mods_dir, engine).await
}

/// 供命令层复用的「是否 Fabric 系」判断。
pub fn instance_uses_fabric(instance: &InstanceRecord) -> bool {
    uses_fabric_family(instance)
}

/// 保留 `LauncherStore` 参数位的占位：当前实现不需要写库（安装结果由文件系统
/// 反映），但命令签名与其它安装类命令保持一致，将来若要记录「此次安装由启动器
/// 发起」时不必再改调用点。
pub fn _store_parameter_reserved(_store: &LauncherStore, _root: &Path) {}
