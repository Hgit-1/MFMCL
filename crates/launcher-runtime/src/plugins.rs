//! Built-in plugin catalogue and validation, without the command layer.
//!
//! See `src-tauri/src/plugin_runtime.rs` for the reasoning behind keeping the
//! catalogue minimal. The same rule applies here: a plugin is only listed if
//! some line of code actually reads its enable switch.

use crate::Result;
use launcher_core::plugins::{Contribution, PluginDefinition, PluginState};

/// Built-in plugin definitions.
///
/// `launch-logger` has no contributions but does control one thing: whether
/// launch history is written. `download-mirror` likewise contributes nothing at
/// launch time — its switch maps to the download source setting, which the
/// installer reads. Listing anything else would advertise a capability that does
/// not exist.
fn builtin_definitions(_state: &PluginState) -> Vec<PluginDefinition> {
    vec![
        PluginDefinition {
            id: "launch-logger".into(),
            version: "1.0.0".into(),
            hooks: vec!["launch".into()],
            permissions: vec!["launchHook".into(), "storage".into()],
            contributions: Vec::new(),
            incompatible: false,
        },
        PluginDefinition {
            id: "download-mirror".into(),
            version: "2.0.1".into(),
            hooks: vec!["launch".into()],
            permissions: vec!["network".into(), "storage".into()],
            contributions: Vec::new(),
            incompatible: false,
        },
    ]
}

/// Whether a plugin is enabled.
///
/// Missing ids default to enabled, matching the UI's switch. Incompatible custom
/// plugins are always off.
pub fn is_enabled(state: &PluginState, plugin_id: &str) -> bool {
    if let Some(false) = state.enabled.get(plugin_id) {
        return false;
    }
    if let Some(plugin) = state.custom.iter().find(|item| item.id == plugin_id) {
        return !plugin.incompatible;
    }
    true
}

/// Built-ins followed by user-installed plugins.
pub fn catalog(state: &PluginState) -> Vec<PluginDefinition> {
    let mut all = builtin_definitions(state);
    all.extend(state.custom.iter().cloned());
    all
}

/// Validate a state before saving it.
///
/// A rejected save is the right moment to report a bad contribution: the user is
/// looking at the form that produced it. Storing it and skipping it at launch
/// time means the failure surfaces days later with no visible connection to its
/// cause.
pub fn validate_state(state: &PluginState) -> Result<()> {
    for plugin in &state.custom {
        if plugin.id.trim().is_empty() {
            return Err(crate::RuntimeError::message("插件 id 不能为空。"));
        }
        for contribution in &plugin.contributions {
            launcher_core::plugins::validate_contribution(contribution).map_err(|reason| {
                crate::RuntimeError::message(format!(
                    "插件「{}」的一条贡献无效：{reason}",
                    plugin.id
                ))
            })?;
        }
    }
    Ok(())
}

/// What one plugin would contribute, and what would be skipped.
pub struct Preview {
    pub applied: Vec<launcher_core::AppliedContribution>,
    pub skipped: Vec<launcher_core::SkippedContribution>,
}

/// Explain what a plugin will actually do at launch.
pub fn preview(state: &PluginState, plugin_id: &str) -> Result<Preview> {
    let plugins = catalog(state);
    let plugin = plugins
        .iter()
        .find(|item| item.id == plugin_id)
        .ok_or_else(|| crate::RuntimeError::message(format!("找不到插件 {plugin_id}。")))?;
    let enabled = state.enabled.get(&plugin.id).copied().unwrap_or(true);
    let mut applied = Vec::new();
    let mut skipped = Vec::new();
    for contribution in &plugin.contributions {
        match launcher_core::plugins::contribution_verdict(plugin, enabled, contribution) {
            Ok(()) => applied.push(launcher_core::AppliedContribution {
                plugin_id: plugin.id.clone(),
                kind: contribution.kind,
                summary: contribution.value.trim().to_owned(),
            }),
            Err(reason) => skipped.push(launcher_core::SkippedContribution {
                plugin_id: plugin.id.clone(),
                kind: contribution.kind,
                reason,
            }),
        }
    }
    Ok(Preview { applied, skipped })
}

/// A user plugin definition from the CLI's simple `add` form.
pub fn definition(
    id: &str,
    hooks: Vec<String>,
    permissions: Vec<String>,
    contributions: Vec<Contribution>,
) -> PluginDefinition {
    PluginDefinition {
        id: id.to_owned(),
        version: "1.0.0".into(),
        hooks,
        permissions,
        contributions,
        incompatible: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_catalogue_is_exactly_the_plugins_with_real_switches() {
        let state = PluginState::default();
        let ids: Vec<String> = catalog(&state).into_iter().map(|p| p.id).collect();
        assert_eq!(ids, vec!["launch-logger", "download-mirror"]);
    }

    #[test]
    fn no_builtin_plugin_claims_a_contribution_it_cannot_execute() {
        let state = PluginState::default();
        for plugin in catalog(&state) {
            assert!(
                plugin.contributions.is_empty(),
                "{} advertises contributions that nothing executes",
                plugin.id
            );
        }
    }

    #[test]
    fn enabled_defaults_to_true_and_respects_an_explicit_false() {
        let mut state = PluginState::default();
        assert!(is_enabled(&state, "anything"));
        state.enabled.insert("launch-logger".into(), false);
        assert!(!is_enabled(&state, "launch-logger"));
    }

    #[test]
    fn incompatible_plugins_are_off() {
        let mut state = PluginState::default();
        state.custom.push(PluginDefinition {
            id: "broken".into(),
            version: "1".into(),
            hooks: vec![],
            permissions: vec![],
            contributions: vec![],
            incompatible: true,
        });
        assert!(!is_enabled(&state, "broken"));
    }

    #[test]
    fn a_blank_plugin_id_is_rejected() {
        let mut state = PluginState::default();
        state.custom.push(definition("   ", vec![], vec![], vec![]));
        assert!(validate_state(&state).is_err());
    }

    #[test]
    fn a_forbidden_contribution_is_rejected_before_saving() {
        let mut state = PluginState::default();
        state.custom.push(definition(
            "sneaky",
            vec!["launch".into()],
            vec![],
            vec![Contribution {
                kind: launcher_core::ContributionKind::JvmArgs,
                value: "-javaagent:/tmp/evil.jar".into(),
            }],
        ));
        // No `launchHook` permission, and a JVM agent is not an allowed argument.
        assert!(validate_state(&state).is_err());
    }

    #[test]
    fn preview_lists_a_skipped_contribution_with_its_reason() {
        let mut state = PluginState::default();
        state.custom.push(definition(
            "sneaky",
            vec!["launch".into()],
            vec![],
            vec![Contribution {
                kind: launcher_core::ContributionKind::JvmArgs,
                value: "-javaagent:/tmp/evil.jar".into(),
            }],
        ));
        let preview = preview(&state, "sneaky").unwrap();
        assert!(preview.applied.is_empty());
        assert_eq!(preview.skipped.len(), 1);
        assert!(!preview.skipped[0].reason.is_empty());
    }

    #[test]
    fn preview_of_an_unknown_plugin_is_an_error() {
        let state = PluginState::default();
        assert!(preview(&state, "nope").is_err());
    }
}
