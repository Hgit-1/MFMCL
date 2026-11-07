//! `mfmcl plugin` — inspect and manage the plugin catalogue.
//!
//! ## Why `inspect` exists
//!
//! A plugin can be "enabled" and still do nothing, because a contribution without
//! the matching permission is skipped. The runtime records exactly what was applied
//! and what was skipped, with reasons. `inspect` surfaces that before a launch, so
//! "I turned it on but nothing happened" becomes answerable without reading logs.

use crate::cli::{PluginAction, PluginArgs};
use crate::commands::download::terminal_width;
use crate::commands::{Failure, Outcome, Session};
use crate::output::{Palette, Table, truncate};
use launcher_core::plugins::{Contribution, ContributionKind, PluginState};

pub fn run(session: &Session<'_>, args: &PluginArgs) -> Outcome {
    let palette = Palette::new(session.sink.palette().on);
    match &args.action {
        PluginAction::List => list(session, &palette),
        PluginAction::Enable { id } => set_enabled(session, &palette, id, true),
        PluginAction::Disable { id } => set_enabled(session, &palette, id, false),
        PluginAction::Inspect { id } => inspect(session, &palette, id),
        PluginAction::Add {
            id,
            hooks,
            permissions,
            jvm_args,
            env,
            pre_launch,
        } => add(
            session,
            &palette,
            id,
            hooks.clone(),
            permissions.clone(),
            jvm_args.clone(),
            env.clone(),
            pre_launch.clone(),
        ),
        PluginAction::Remove { id } => remove(session, &palette, id),
    }
}

/// Load the saved plugin state, or the default when nothing is stored.
fn load(session: &Session<'_>) -> Outcome<PluginState> {
    Ok(session.store.load_plugin_state()?)
}

fn list(session: &Session<'_>, palette: &Palette) -> Outcome {
    let state = load(session)?;
    let definitions = launcher_runtime::plugins::catalog(&state);
    if session.cli.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "plugins": definitions,
                "enabled": state.enabled,
            }))
            .unwrap_or_default()
        );
        return Ok(());
    }
    let mut table = Table::new(&["插件", "版本", "来源", "启用", "贡献"]);
    for definition in &definitions {
        let builtin = !state.custom.iter().any(|item| item.id == definition.id);
        let enabled = launcher_runtime::plugins::is_enabled(&state, &definition.id);
        table.row(vec![
            definition.id.clone(),
            definition.version.clone(),
            if builtin {
                "内置".to_owned()
            } else {
                "自定义".to_owned()
            },
            if enabled {
                palette.green("是")
            } else {
                palette.dim("否")
            },
            if definition.contributions.is_empty() {
                palette.dim("无")
            } else {
                definition.contributions.len().to_string()
            },
        ]);
    }
    print!("{}", table.render(palette, terminal_width()));
    println!();
    println!(
        "{}",
        palette.dim("查看某个插件实际会做什么：mfmcl plugin inspect <id>")
    );
    Ok(())
}

fn set_enabled(session: &Session<'_>, palette: &Palette, id: &str, enabled: bool) -> Outcome {
    let mut state = load(session)?;
    let known = launcher_runtime::plugins::catalog(&state)
        .iter()
        .any(|definition| definition.id == id);
    if !known {
        return Err(Failure::message(format!(
            "找不到插件「{id}」。用 `mfmcl plugin list` 查看已安装的插件。"
        )));
    }
    state.enabled.insert(id.to_owned(), enabled);
    session.store.save_plugin_state(&state)?;
    if session.cli.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({ "id": id, "enabled": enabled }))
                .unwrap_or_default()
        );
        return Ok(());
    }
    println!(
        "{} 插件「{id}」已{}",
        palette.green("完成"),
        if enabled { "启用" } else { "停用" }
    );
    Ok(())
}

fn inspect(session: &Session<'_>, palette: &Palette, id: &str) -> Outcome {
    let state = load(session)?;
    let preview = launcher_runtime::plugins::preview(&state, id).map_err(Failure::from)?;
    if session.cli.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "applied": preview.applied,
                "skipped": preview.skipped,
            }))
            .unwrap_or_default()
        );
        return Ok(());
    }
    println!(
        "插件 {id}（{}）",
        if launcher_runtime::plugins::is_enabled(&state, id) {
            "已启用"
        } else {
            "已停用"
        }
    );
    if preview.applied.is_empty() && preview.skipped.is_empty() {
        println!("{}", palette.dim("这个插件不向启动过程注入任何内容。"));
        return Ok(());
    }
    if !preview.applied.is_empty() {
        println!("\n{}", palette.bold("启动时会执行："));
        for item in &preview.applied {
            println!("  {} {}", palette.green("+"), truncate(&item.summary, 70));
        }
    }
    if !preview.skipped.is_empty() {
        println!("\n{}", palette.bold("会被跳过："));
        for item in &preview.skipped {
            println!(
                "  {} {} — {}",
                palette.red("-"),
                kind_label(item.kind),
                item.reason
            );
        }
    }
    Ok(())
}

/// Local names for contribution kinds.
///
/// `ContributionKind` intentionally has no `Display` impl: it is a wire value that
/// is serialized as `jvmArgs`/`preLaunch` for the frontend, and adding a display
/// string there would tempt callers to print the wire format at users. Mapping it
/// here keeps the terminal wording in the terminal crate.
fn kind_label(kind: ContributionKind) -> &'static str {
    match kind {
        ContributionKind::JvmArgs => "JVM 参数",
        ContributionKind::Env => "环境变量",
        ContributionKind::PreLaunch => "启动前命令",
        ContributionKind::BackupBefore => "启动前备份",
    }
}

#[allow(clippy::too_many_arguments)]
fn add(
    session: &Session<'_>,
    palette: &Palette,
    id: &str,
    hooks: Vec<String>,
    permissions: Vec<String>,
    jvm_args: Vec<String>,
    env: Vec<String>,
    pre_launch: Vec<String>,
) -> Outcome {
    let mut state = load(session)?;
    if state.custom.iter().any(|item| item.id == id) {
        return Err(Failure::message(format!(
            "自定义插件「{id}」已存在；如需修改请先 `mfmcl plugin remove {id}`。"
        )));
    }
    if launcher_runtime::plugins::catalog(&state)
        .iter()
        .any(|definition| definition.id == id)
    {
        return Err(Failure::message(format!(
            "「{id}」是内置插件，不能用 add 覆盖。"
        )));
    }

    let mut contributions = Vec::new();
    for value in jvm_args {
        contributions.push(Contribution {
            kind: ContributionKind::JvmArgs,
            value,
        });
    }
    for value in env {
        // Reject a malformed pair here rather than at launch time, when the user is
        // watching a game window instead of reading an error.
        if !value.contains('=') || value.starts_with('=') {
            return Err(Failure::message(format!(
                "环境变量贡献必须是 KEY=VALUE 形式（收到 {value}）。"
            )));
        }
        contributions.push(Contribution {
            kind: ContributionKind::Env,
            value,
        });
    }
    for value in pre_launch {
        contributions.push(Contribution {
            kind: ContributionKind::PreLaunch,
            value,
        });
    }

    state.custom.push(launcher_runtime::plugins::definition(
        id,
        hooks,
        permissions,
        contributions,
    ));
    /*
     * Validation happens on the candidate state, before it is written. A rejected
     * save means the plugin is never stored, so a bad contribution cannot sit in
     * the database waiting to be discovered at launch.
     */
    launcher_runtime::plugins::validate_state(&state).map_err(|error| {
        Failure::message(format!(
            "插件「{id}」未通过校验：{error}\n提示：注入 JVM 参数或启动前命令需要 --permission launchHook。"
        ))
    })?;
    session.store.save_plugin_state(&state)?;
    if session.cli.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({ "id": id, "added": true }))
                .unwrap_or_default()
        );
        return Ok(());
    }
    println!("{} 已添加插件「{id}」", palette.green("完成"));
    Ok(())
}

fn remove(session: &Session<'_>, palette: &Palette, id: &str) -> Outcome {
    let mut state = load(session)?;
    let before = state.custom.len();
    state.custom.retain(|item| item.id != id);
    if state.custom.len() == before {
        return Err(Failure::message(format!(
            "「{id}」不是自定义插件；内置插件不能删除，但可以用 `mfmcl plugin disable {id}` 停用。"
        )));
    }
    state.enabled.remove(id);
    session.store.save_plugin_state(&state)?;
    if session.cli.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({ "id": id, "removed": true }))
                .unwrap_or_default()
        );
        return Ok(());
    }
    println!("{} 已删除插件「{id}」", palette.green("完成"));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use launcher_core::plugins::ContributionKind;

    #[test]
    fn a_definition_carries_every_contribution_kind() {
        let definition = launcher_runtime::plugins::definition(
            "probe",
            vec!["launch".into()],
            vec!["launchHook".into()],
            vec![
                Contribution {
                    kind: ContributionKind::JvmArgs,
                    value: "-XX:+UseG1GC".into(),
                },
                Contribution {
                    kind: ContributionKind::Env,
                    value: "A=1".into(),
                },
            ],
        );
        assert_eq!(definition.contributions.len(), 2);
        assert_eq!(definition.id, "probe");
    }
}
