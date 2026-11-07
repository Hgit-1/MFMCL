//! Command dispatch and the pieces every command needs.
//!
//! ## Error shape
//!
//! [`Failure`] separates "the command failed" from "the user declined". They map to
//! different exit codes, and a script that pipes output somewhere needs to tell
//! "this broke" from "this was cancelled on purpose".

pub mod account;
pub mod backup;
pub mod config;
pub mod doctor;
pub mod download;
pub mod help;
pub mod launch;
pub mod list;
pub mod plugin;
pub mod remove;

use launcher_core::{AccountProfile, InstanceRecord, LauncherStore};
use launcher_runtime::RuntimeContext;

use crate::cli::{Cli, Command};
use crate::sink::TerminalSink;

/// Why a command did not succeed.
#[derive(Debug)]
pub enum Failure {
    /// The command failed. The message is shown to the user as-is.
    Message(String),
    /// The user declined a confirmation, or a destructive action lacked `--yes`.
    Declined(String),
}

impl Failure {
    pub fn message(text: impl Into<String>) -> Self {
        Self::Message(text.into())
    }
}

impl From<String> for Failure {
    fn from(message: String) -> Self {
        Self::Message(message)
    }
}

impl From<launcher_runtime::RuntimeError> for Failure {
    fn from(error: launcher_runtime::RuntimeError) -> Self {
        Self::Message(error.to_string())
    }
}

impl From<launcher_core::StoreError> for Failure {
    fn from(error: launcher_core::StoreError) -> Self {
        Self::Message(error.to_string())
    }
}

pub type Outcome<T = ()> = Result<T, Failure>;

/// Everything a command needs that is the same for all of them.
pub struct Session<'a> {
    pub cli: &'a Cli,
    pub sink: &'a TerminalSink,
    pub context: RuntimeContext,
    pub store: LauncherStore,
}

impl<'a> Session<'a> {
    /// Whether confirmations should be assumed accepted.
    ///
    /// `--yes` is the scripted answer. Without it, a destructive command in a
    /// non-interactive shell has no way to ask, so it declines — silence is not
    /// consent.
    pub fn assume_yes(&self) -> bool {
        self.cli.yes
    }

    /// Ask for confirmation, honouring `--yes`.
    pub fn confirm(&self, question: &str) -> bool {
        if self.assume_yes() {
            return true;
        }
        crate::output::confirm(question, false)
    }
}

/// Resolve the data directory.
///
/// The desktop app uses the Tauri identifier (`dev.mfmcl.launcher`) under the OS's
/// roaming app-data directory. The CLI must land on the *same* path, or the two
/// frontends would each have their own list of instances and accounts — which is
/// the single most confusing thing a launcher can do.
pub fn default_data_dir() -> Result<std::path::PathBuf, Failure> {
    let base = if cfg!(windows) {
        std::env::var_os("APPDATA")
            .map(std::path::PathBuf::from)
            // APPDATA is unset in some service contexts; fall back to the profile.
            .or_else(|| {
                std::env::var_os("USERPROFILE")
                    .map(|home| std::path::PathBuf::from(home).join("AppData/Roaming"))
            })
    } else if cfg!(target_os = "macos") {
        std::env::var_os("HOME")
            .map(|home| std::path::PathBuf::from(home).join("Library/Application Support"))
    } else {
        std::env::var_os("XDG_DATA_HOME")
            .map(std::path::PathBuf::from)
            .or_else(|| {
                std::env::var_os("HOME")
                    .map(|home| std::path::PathBuf::from(home).join(".local/share"))
            })
    };
    let base = base.ok_or_else(|| {
        Failure::message("找不到用户数据目录；请用 --data-dir 明确指定启动器数据目录。")
    })?;
    Ok(base.join("dev.mfmcl.launcher"))
}

/// Find an instance by id or by name.
///
/// Ids are matched first because they are unique. Names are matched
/// case-insensitively, and a name that matches several instances is an error: the
/// alternative is silently acting on the wrong instance.
pub fn resolve_instance(store: &LauncherStore, needle: &str) -> Result<InstanceRecord, Failure> {
    let needle = needle.trim();
    if needle.is_empty() {
        return Err(Failure::message("实例名不能为空。"));
    }
    let instances = store.list_instances()?;
    if let Some(found) = instances.iter().find(|instance| instance.id == needle) {
        return Ok(found.clone());
    }
    let matches: Vec<&InstanceRecord> = instances
        .iter()
        .filter(|instance| instance.name.eq_ignore_ascii_case(needle))
        .collect();
    match matches.len() {
        1 => Ok(matches[0].clone()),
        0 => {
            let mut message = format!("找不到实例「{needle}」。");
            if !instances.is_empty() {
                message.push_str("\n现有实例：");
                for instance in instances.iter().take(10) {
                    message.push_str(&format!(
                        "\n  {} ({})",
                        instance.name, instance.game_version
                    ));
                }
            } else {
                message.push_str("\n还没有任何实例；先用 `mfmcl download` 创建一个。");
            }
            Err(Failure::Message(message))
        }
        _ => {
            let mut message = format!("实例名「{needle}」不唯一，请改用 id：");
            for instance in matches.iter().take(10) {
                message.push_str(&format!("\n  {}  {}", instance.id, instance.game_version));
            }
            Err(Failure::Message(message))
        }
    }
}

/// Resolve an account by name or id, defaulting to the selected one.
pub fn resolve_account(
    store: &LauncherStore,
    needle: Option<&str>,
) -> Result<AccountProfile, Failure> {
    let accounts = store.list_accounts()?;
    let Some(needle) = needle.map(str::trim).filter(|value| !value.is_empty()) else {
        return accounts
            .iter()
            .find(|account| account.selected)
            .cloned()
            .or_else(|| accounts.first().cloned())
            .ok_or_else(|| {
                Failure::message(
                    "还没有任何账户；先用 `mfmcl account add-offline <名称>` 或图形界面添加一个。",
                )
            });
    };
    if let Some(found) = accounts.iter().find(|account| account.id == needle) {
        return Ok(found.clone());
    }
    let matches: Vec<&AccountProfile> = accounts
        .iter()
        .filter(|account| account.display_name.eq_ignore_ascii_case(needle))
        .collect();
    match matches.len() {
        1 => Ok(matches[0].clone()),
        0 => Err(Failure::message(format!("找不到账户「{needle}」。"))),
        _ => Err(Failure::message(format!(
            "账户名「{needle}」不唯一，请改用 id。"
        ))),
    }
}

/// The instance used most recently, for `launch --last`.
pub fn last_instance(store: &LauncherStore) -> Result<InstanceRecord, Failure> {
    let instances = store.list_instances()?;
    instances
        .into_iter()
        .filter(|instance| instance.last_launched_at.is_some())
        .max_by(|left, right| left.last_launched_at.cmp(&right.last_launched_at))
        .ok_or_else(|| {
            Failure::message(
                "还没有启动过任何实例；请先用 `mfmcl launch --instance <名称>` 指定一个。",
            )
        })
}

/// Run the parsed command.
pub async fn dispatch(cli: &Cli, sink: &TerminalSink) -> Outcome {
    let data_root = match &cli.data_dir {
        Some(dir) => std::path::PathBuf::from(dir),
        None => default_data_dir()?,
    };
    let context = RuntimeContext::new(&data_root);
    context
        .prepare()
        .map_err(|error| Failure::message(error.to_string()))?;
    let store = LauncherStore::open(data_root.join("mfmcl.sqlite"))?;
    let session = Session {
        cli,
        sink,
        context,
        store,
    };

    match &cli.command {
        Command::Download(args) => download::run(&session, args).await,
        Command::Launch(args) => launch::run(&session, args).await,
        Command::List(args) => list::run(&session, args).await,
        Command::Backup(args) => backup::run(&session, args).await,
        Command::Plugin(args) => plugin::run(&session, args),
        Command::Config(args) => config::run(&session, args),
        Command::Account(args) => account::run(&session, args).await,
        Command::Doctor(args) => doctor::run(&session, args),
        Command::Remove(args) => remove::run(&session, args),
        // Handled in `main` before any I/O happens.
        Command::Help { .. } => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use launcher_core::{CreateInstanceInput, InstanceState};

    fn store_with(names: &[(&str, &str)]) -> (std::path::PathBuf, LauncherStore) {
        let root = std::env::temp_dir().join(format!("mfmcl-cli-{}", uuid::Uuid::new_v4()));
        let store = LauncherStore::open(root.join("mfmcl.sqlite")).unwrap();
        for (name, version) in names {
            store
                .create_instance_in(
                    &CreateInstanceInput {
                        name: (*name).into(),
                        game_version: (*version).into(),
                    },
                    &root,
                    None,
                )
                .unwrap();
        }
        (root, store)
    }

    #[test]
    fn instances_resolve_by_name_case_insensitively() {
        let (root, store) = store_with(&[("MyInstance", "1.21.1")]);
        let found = resolve_instance(&store, "myinstance").unwrap();
        assert_eq!(found.name, "MyInstance");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn instances_resolve_by_id() {
        let (root, store) = store_with(&[("a", "1.21.1")]);
        let id = store.list_instances().unwrap()[0].id.clone();
        assert_eq!(resolve_instance(&store, &id).unwrap().id, id);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_missing_instance_lists_the_available_ones() {
        let (root, store) = store_with(&[("existing", "1.21.1")]);
        let error = resolve_instance(&store, "nope").unwrap_err();
        let Failure::Message(text) = error else {
            panic!("expected a message");
        };
        assert!(text.contains("existing"), "got {text}");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn an_empty_installation_explains_how_to_start() {
        let (root, store) = store_with(&[]);
        let Failure::Message(text) = resolve_instance(&store, "x").unwrap_err() else {
            panic!("expected a message");
        };
        assert!(text.contains("mfmcl download"), "got {text}");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn the_default_data_dir_matches_the_tauri_identifier() {
        let path = default_data_dir().expect("a data dir in the test environment");
        assert!(
            path.ends_with("dev.mfmcl.launcher"),
            "the CLI must share state with the desktop app: {}",
            path.display()
        );
    }

    #[test]
    fn an_account_is_optional_only_when_one_exists() {
        let (root, store) = store_with(&[]);
        assert!(resolve_account(&store, None).is_err());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn last_instance_requires_a_previous_launch() {
        let (root, store) = store_with(&[("a", "1.21.1")]);
        assert!(last_instance(&store).is_err());
        // Marking it launched makes it eligible; state does not matter here.
        let id = store.list_instances().unwrap()[0].id.clone();
        store.set_instance_state(&id, InstanceState::Ready).unwrap();
        store.mark_instance_launched(&id).unwrap();
        assert_eq!(last_instance(&store).unwrap().id, id);
        let _ = std::fs::remove_dir_all(root);
    }
}
