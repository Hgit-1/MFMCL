//! 插件运行时的命令层与内置目录。
//!
//! ## 这里定义什么、不定义什么
//!
//! 这个模块给前端提供三样东西：内置插件目录、用户状态（装了哪些、启用了哪些）、
//! 以及启动历史。**贡献的执行在 `launch_instance` 里**，不在这里——执行需要的
//! 上下文（实例、已解析设置、准备好的启动参数）只有那条路径才有。
//!
//! ## 内置插件为什么在这里而不是前端
//!
//! 内置插件的**贡献**必须由后端知道，否则启动时无从执行。但它们的文案、图标与
//! 品牌色属于展示，前端已经有完整的一份（`utils/plugins.ts`）。两边用 `id`
//! 对齐；同一份清单拆成两个视图，各自只带自己需要的字段。
//!
//! 为什么不同步成一份：前端那份要支持三语文案与图标字形，后端用不上，塞进来只会
//! 让后端多一份永远不会被读取的 i18n 数据；反之，后端的贡献定义前端也不需要理解。
//! 两者的共同契约是「id + 启用状态 + 贡献」。
//!
//! ## 目前内置插件真正做了什么
//!
//! 只给那些**能安全执行、且确实有意**的动作定义贡献：
//!
//!   - `auto-backup` → 启动前备份（需要用户配置目录，所以默认没有贡献条目，
//!     由设置里的备份目录驱动——见下面的说明）
//!   - `launch-logger` → 通过启动历史记录每次启动（不需要额外贡献，历史本身就是
//!     这个插件的产出）
//!   - `download-mirror` → 环境变量提示下载器使用镜像
//!
//! 其余内置插件（冲突检测、主题、性能监控）当前没有可执行的贡献：它们的功能要么
//! 已经在启动器内部实现（冲突检测在 `mods.rs`），要么需要界面侧的支持（主题、
//! 侧栏插槽）而不属于启动链路。它们**不应该**声明假的贡献，否则用户会看到一个
//! 永远显示「已应用」却什么也没做的插件——那正是这次要修的问题。

use launcher_core::plugins::{LaunchHistoryEntry, PluginDefinition, PluginState};
use launcher_core::plugins as runtime;
use launcher_core::LauncherStore;
use tauri::State;

/// 内置插件的贡献定义。
///
/// 返回 `Vec<PluginDefinition>` 而不是常量表：备份目录是用户配置项，必须在运行时
/// 读取。把它写死成常量会让「插件备份到哪个目录」无法被用户改变。
fn builtin_definitions(state: &PluginState) -> Vec<PluginDefinition> {
    /*
     * 启动记录。
     *
     * 这个插件不需要贡献就能工作——启动历史是启动器自己的能力，启用与否只决定
     * 是否写入。所以它没有 `contributions`，但它确实「生效」了：用户能在历史里
     * 看到记录。这一点很重要，否则「启用了但贡献为空」看起来像没实现。
     */
    /*
     * 启动记录。
     *
     * 这个插件没有贡献条目，但它确实控制一件事：**是否写入启动历史**。所以
     * 启动路径必须查它的启用状态——否则「关掉启动记录」不会有任何效果，而那正是
     * 用户关闭它的原因（比如不想留下启动痕迹）。
     */
    let launch_logger = PluginDefinition {
        id: "launch-logger".into(),
        version: "1.0.0".into(),
        hooks: vec!["launch".into()],
        permissions: vec!["launchHook".into(), "storage".into()],
        contributions: Vec::new(),
        incompatible: false,
    };

    /*
     * 自动备份。
     *
     * 贡献的目录来自用户在插件选项里配置的路径。`PluginState` 目前只存启用状态，
     * 不存选项值——选项值由前端保存在自己的偏好里（它们是纯界面配置）。所以这里
     * 不能凭空编一个目录：没有目录就**不产生贡献**，让启动路径里什么都不做。
     *
     * 这是有意的保守选择：静默挑一个默认目录（比如系统临时目录）会让插件在用户
     * 不知情的地方不断写入几十 MB 的归档。宁可什么都不做，也不要偷偷写盘。
     */
    let auto_backup = PluginDefinition {
        id: "auto-backup".into(),
        version: "1.2.0".into(),
        hooks: vec!["launch".into()],
        permissions: vec!["filesystem".into(), "launchHook".into()],
        contributions: Vec::new(),
        incompatible: false,
    };

    /*
     * 下载镜像助手。
     *
     * **不产生启动贡献。**
     *
     * 之前这里设置了一个 `MFMCL_PREFER_MIRROR` 环境变量，看起来像在做事，实际
     * 没有任何代码读取它——镜像选择由启动器自己的 `download.source` 设置在
     * `install_instance` 里决定，与游戏进程的环境无关。一个「已应用」但恒真的
     * 环境变量是纯粹的假象，比没有贡献更糟：它让用户以为插件在工作。
     *
     * 这个插件的真实作用是把用户的镜像偏好写进那个设置（界面侧完成）。所以它在
     * 启动链路上没有可执行的东西，这里如实留空。
     */
    let download_mirror = PluginDefinition {
        id: "download-mirror".into(),
        version: "2.0.1".into(),
        hooks: vec!["launch".into()],
        permissions: vec!["network".into(), "storage".into()],
        contributions: Vec::new(),
        incompatible: false,
    };

    /*
     * 模组冲突检测。
     *
     * 检测逻辑已经在启动器内部（安装前由 `mods.rs` 分析），插件的作用是决定
     * 「是否在安装前提示」。它没有启动链路贡献，但有实际效果——这里不伪造一条
     * 假的 jvmArgs 来证明自己存在。
     */
    let conflict_detector = PluginDefinition {
        id: "mod-conflict-detector".into(),
        version: "0.9.3".into(),
        hooks: vec!["modInstall".into()],
        permissions: vec!["network".into(), "uiSlot".into()],
        contributions: Vec::new(),
        incompatible: false,
    };

    // 主题与界面类插件：效果在界面侧，启动链路里没有可执行的东西。
    let presentation = |id: &str, version: &str, hooks: &[&str], permission: &str| {
        PluginDefinition {
            id: id.to_owned(),
            version: version.to_owned(),
            hooks: hooks.iter().map(|item| (*item).to_owned()).collect(),
            permissions: vec![permission.to_owned()],
            contributions: Vec::new(),
            incompatible: false,
        }
    };

    let _ = state;
    vec![
        launch_logger,
        auto_backup,
        conflict_detector,
        download_mirror,
        presentation("pink-theme", "1.0.0", &["theme"], "theme"),
        presentation("performance-monitor", "1.1.2", &["launch", "sidebar"], "launchHook"),
        /*
         * 旧版皮肤补丁：与当前启动器不兼容。
         *
         * `incompatible: true` 在这里是**权威判定**，不只是界面提示——启动路径会
         * 据此跳过它的全部贡献。之前这个字段只影响界面上开关的颜色，后端完全不知道，
         * 一个被标记为不兼容的插件仍然可以贡献参数。
         */
        PluginDefinition {
            incompatible: true,
            ..presentation("legacy-skin-patch", "0.3.1", &["launch"], "launchHook")
        },
    ]
}

/// 某个插件当前是否启用。
///
/// 与 `isPluginEnabled`（前端）同一套规则：未出现的 id 视为启用，不兼容恒为关闭。
/// 启动路径靠它决定「启动记录要不要写」这类没有贡献条目、但确实有开关效果的插件。
pub(crate) fn is_enabled(state: &PluginState, plugin_id: &str) -> bool {
    if let Some(false) = state.enabled.get(plugin_id) {
        return false;
    }
    // 不兼容的插件视为未启用，与界面上的开关状态一致。
    if let Some(plugin) = state.custom.iter().find(|item| item.id == plugin_id) {
        return !plugin.incompatible;
    }
    true
}

/// 全部插件：内置 + 用户安装。启动路径与命令都走这一个入口，保证两边一致。
pub(crate) fn catalog(state: &PluginState) -> Vec<PluginDefinition> {
    let mut all = builtin_definitions(state);
    all.extend(state.custom.iter().cloned());
    all
}

// ---------------------------------------------------------------------------
// 命令
// ---------------------------------------------------------------------------

/// 插件运行时可用的能力清单，供界面说明「这个插件会不会真的被执行」。
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginRuntimeInfo {
    /// 运行时是否可用。桌面版恒为 true。
    pub available: bool,
    /// 真正会被执行的贡献类型。界面上可以据此说明哪些声明是有效的。
    pub kinds: Vec<&'static str>,
}

#[tauri::command]
pub fn plugin_runtime_info() -> PluginRuntimeInfo {
    PluginRuntimeInfo {
        available: true,
        kinds: vec!["jvmArgs", "env", "preLaunch", "backupBefore"],
    }
}

/// 读取插件状态（自定义插件清单 + 启用覆盖）。
#[tauri::command]
pub fn plugin_state(store: State<'_, LauncherStore>) -> Result<PluginState, String> {
    store.load_plugin_state().map_err(|error| error.to_string())
}

/**
 * 保存插件状态。
 *
 * 保存前逐条校验贡献：非法贡献**拒绝整份保存**，而不是存下来等到启动时才在历史里
 * 悄悄跳过。理由：用户正在编辑这份清单，此刻告诉他「这条参数不允许加载 agent」
 * 是最有用的时机；等到下次启动才发现，他很难把失败和几周前填的表单联系起来。
 */
#[tauri::command]
pub fn plugin_save_state(
    state: PluginState,
    store: State<'_, LauncherStore>,
) -> Result<(), String> {
    for plugin in &state.custom {
        if plugin.id.trim().is_empty() {
            return Err("插件 id 不能为空。".into());
        }
        for contribution in &plugin.contributions {
            launcher_core::plugins::validate_contribution(contribution)
                .map_err(|reason| format!("插件「{}」的一条贡献无效：{reason}", plugin.id))?;
        }
    }
    store
        .save_plugin_state(&state)
        .map_err(|error| error.to_string())
}

/// 最近 200 次启动。
#[tauri::command]
pub fn launch_history(store: State<'_, LauncherStore>) -> Result<Vec<LaunchHistoryEntry>, String> {
    store.list_launch_history().map_err(|error| error.to_string())
}

#[tauri::command]
pub fn clear_launch_history(store: State<'_, LauncherStore>) -> Result<usize, String> {
    store.clear_launch_history().map_err(|error| error.to_string())
}

/**
 * 预览一份插件清单实际会贡献什么。
 *
 * 这是「插件能不能用」这个问题的直接答案：用户装完一个插件、打开详情，就能看到
 * 「这次启动会应用 2 条贡献」「这条被跳过，因为缺少 filesystem 权限」。没有它，
 * 用户只能通过启动游戏后观察副作用来猜。
 */
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginPreview {
    pub applied: Vec<launcher_core::plugins::AppliedContribution>,
    pub skipped: Vec<launcher_core::plugins::SkippedContribution>,
}

#[tauri::command]
pub fn plugin_preview(
    plugin_id: String,
    store: State<'_, LauncherStore>,
) -> Result<PluginPreview, String> {
    let state = store.load_plugin_state().map_err(|error| error.to_string())?;
    let plugins = catalog(&state);
    let plugin = plugins
        .iter()
        .find(|item| item.id == plugin_id)
        .ok_or_else(|| format!("找不到插件 {plugin_id}。"))?;
    let enabled = state.enabled.get(&plugin.id).copied().unwrap_or(true);

    let mut applied = Vec::new();
    let mut skipped = Vec::new();
    for contribution in &plugin.contributions {
        match runtime::contribution_verdict(plugin, enabled, contribution) {
            Ok(()) => applied.push(launcher_core::plugins::AppliedContribution {
                plugin_id: plugin.id.clone(),
                kind: contribution.kind,
                summary: contribution.value.trim().to_owned(),
            }),
            Err(reason) => skipped.push(launcher_core::plugins::SkippedContribution {
                plugin_id: plugin.id.clone(),
                kind: contribution.kind,
                reason,
            }),
        }
    }
    Ok(PluginPreview { applied, skipped })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_contains_every_builtin_plugin_the_ui_shows() {
        let state = PluginState::default();
        let plugins = catalog(&state);
        let ids: Vec<&str> = plugins.iter().map(|p| p.id.as_str()).collect();
        for expected in [
            "launch-logger",
            "auto-backup",
            "mod-conflict-detector",
            "download-mirror",
            "pink-theme",
            "performance-monitor",
            "legacy-skin-patch",
        ] {
            assert!(ids.contains(&expected), "{expected} missing from catalog");
        }
    }

    #[test]
    fn legacy_skin_patch_is_authoritatively_incompatible() {
        /*
         * 这个标记必须由后端给出，而不是只在前端提示：启动路径据此跳过它的全部
         * 贡献。之前它只影响界面开关的颜色，后端完全不知道。
         */
        let state = PluginState::default();
        let plugin = catalog(&state)
            .into_iter()
            .find(|p| p.id == "legacy-skin-patch")
            .expect("plugin must exist");
        assert!(plugin.incompatible);
    }

    #[test]
    fn no_builtin_plugin_fakes_a_contribution() {
        /*
         * 这条测试防的是「看起来在工作」。
         *
         * 曾经下载镜像助手设置了一个 MFMCL_PREFER_MIRROR 环境变量，而没有任何代码
         * 读取它——插件显示「已应用」，实际什么也没发生。比起没有贡献，假贡献更糟：
         * 它让用户相信一个不存在的功能。
         *
         * 现在只有真正被执行的东西才允许出现在这里。新增内置贡献时，必须先在启动
         * 链路里找到读取它的地方，再回来更新这条断言。
         */
        let state = PluginState::default();
        let plugins = catalog(&state);
        let with_contributions: Vec<String> = plugins
            .into_iter()
            .filter(|plugin| !plugin.contributions.is_empty())
            .map(|plugin| plugin.id)
            .collect();
        assert!(
            with_contributions.is_empty(),
            "内置插件当前都不应声明启动贡献，但发现了：{with_contributions:?}。\
             如果确实新增了会执行的贡献，请同步更新这条断言与 PLUGIN.md。"
        );
    }

    #[test]
    fn user_plugins_are_appended_after_builtin_ones() {
        let mut state = PluginState::default();
        state.custom.push(PluginDefinition {
            id: "mine".into(),
            version: "1.0.0".into(),
            hooks: vec!["launch".into()],
            permissions: vec!["launchHook".into()],
            contributions: Vec::new(),
            incompatible: false,
        });
        let all = catalog(&state);
        assert_eq!(all.last().map(|p| p.id.as_str()), Some("mine"));
    }

    #[test]
    fn is_enabled_defaults_to_true_and_respects_explicit_false() {
        let mut state = PluginState::default();
        assert!(is_enabled(&state, "anything"), "缺省必须视为启用");
        state.enabled.insert("pink-theme".into(), false);
        assert!(!is_enabled(&state, "pink-theme"));
    }

    #[test]
    fn is_enabled_treats_incompatible_custom_plugin_as_off() {
        let mut state = PluginState::default();
        state.custom.push(PluginDefinition {
            id: "broken".into(),
            version: "1.0.0".into(),
            hooks: vec![],
            permissions: vec![],
            contributions: vec![],
            incompatible: true,
        });
        assert!(!is_enabled(&state, "broken"));
    }
}
