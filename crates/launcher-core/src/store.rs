use crate::model::{
    AccountKind, AccountProfile, CreateInstanceInput, DownloadTask, DownloadTaskState,
    InstanceRecord, InstanceState, LauncherSettings,
};
use chrono::Utc;
use rusqlite::{Connection, OptionalExtension, params};
use serde_json;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use thiserror::Error;
use uuid::Uuid;

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("database operation failed: {0}")]
    Database(#[from] rusqlite::Error),
    #[error("stored data could not be encoded or decoded: {0}")]
    Serialization(#[from] serde_json::Error),
    #[error("invalid data: {0}")]
    Invalid(String),
    #[error("database connection is unavailable")]
    Poisoned,
}

#[derive(Clone)]
pub struct LauncherStore {
    connection: Arc<Mutex<Connection>>,
}

impl LauncherStore {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, StoreError> {
        let database_path = path.as_ref();
        if database_path != Path::new(":memory:") {
            if let Some(parent) = database_path
                .parent()
                .filter(|parent| !parent.as_os_str().is_empty())
            {
                std::fs::create_dir_all(parent).map_err(|error| {
                    StoreError::Invalid(format!("could not create app data directory: {error}"))
                })?;
            }
        }
        let connection = Connection::open(database_path)?;
        connection.pragma_update(None, "foreign_keys", "ON")?;
        connection.busy_timeout(std::time::Duration::from_secs(5))?;
        connection.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS app_settings (
                id INTEGER PRIMARY KEY CHECK (id = 1),
                schema_version INTEGER NOT NULL,
                json TEXT NOT NULL,
                updated_at TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS instances (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                game_version TEXT NOT NULL,
                loader TEXT NOT NULL,
                instance_dir TEXT NOT NULL,
                game_dir TEXT NOT NULL,
                isolated INTEGER NOT NULL,
                state TEXT NOT NULL,
                created_at TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS accounts (
                id TEXT PRIMARY KEY,
                display_name TEXT NOT NULL,
                kind TEXT NOT NULL,
                auth_server TEXT,
                credential_ref TEXT,
                selected INTEGER NOT NULL DEFAULT 0,
                created_at TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS download_tasks (
                id TEXT PRIMARY KEY,
                label TEXT NOT NULL,
                source TEXT,
                state TEXT NOT NULL,
                completed_bytes INTEGER NOT NULL DEFAULT 0,
                total_bytes INTEGER,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS schema_migrations (
                version INTEGER PRIMARY KEY,
                applied_at TEXT NOT NULL
            );
            INSERT OR IGNORE INTO schema_migrations(version, applied_at)
                VALUES (1, datetime('now'));
            ",
        )?;
        let has_instance_id = {
            let mut columns = connection.prepare("PRAGMA table_info(download_tasks)")?;
            let names = columns
                .query_map([], |row| row.get::<_, String>(1))?
                .collect::<Result<Vec<_>, _>>()?;
            names.iter().any(|name| name == "instance_id")
        };
        if !has_instance_id {
            connection.execute(
                "ALTER TABLE download_tasks ADD COLUMN instance_id TEXT NOT NULL DEFAULT ''",
                [],
            )?;
            connection.execute("INSERT OR IGNORE INTO schema_migrations(version, applied_at) VALUES (2, datetime('now'))", [])?;
        }
        // A previous process may have been interrupted. Never keep an instance or task falsely running.
        connection.execute(
            "UPDATE instances SET state = 'failed' WHERE state = 'installing'",
            [],
        )?;
        connection.execute("UPDATE download_tasks SET state = 'interrupted', updated_at = datetime('now') WHERE state IN ('running', 'paused')", [])?;
        Ok(Self {
            connection: Arc::new(Mutex::new(connection)),
        })
    }

    pub fn save_download_task(&self, task: &DownloadTask) -> Result<(), StoreError> {
        let state = serde_json::to_value(&task.state)?;
        let connection = self.connection.lock().map_err(|_| StoreError::Poisoned)?;
        connection.execute(
            "INSERT INTO download_tasks(id, instance_id, label, source, state, completed_bytes, total_bytes, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
             ON CONFLICT(id) DO UPDATE SET instance_id = excluded.instance_id, label = excluded.label, source = excluded.source,
             state = excluded.state, completed_bytes = excluded.completed_bytes,
             total_bytes = excluded.total_bytes, updated_at = excluded.updated_at",
            params![task.id, task.instance_id, task.label, task.source, state.as_str(), task.completed_bytes.min(i64::MAX as u64) as i64,
                task.total_bytes.map(|v| v.min(i64::MAX as u64) as i64), task.created_at, task.updated_at],
        )?;
        Ok(())
    }

    pub fn list_download_tasks(&self) -> Result<Vec<DownloadTask>, StoreError> {
        let connection = self.connection.lock().map_err(|_| StoreError::Poisoned)?;
        let mut statement = connection.prepare("SELECT id, instance_id, label, source, state, completed_bytes, total_bytes, created_at, updated_at FROM download_tasks ORDER BY updated_at DESC LIMIT 100")?;
        let rows = statement.query_map([], |row| {
            let state: String = row.get(4)?;
            let state = match state.as_str() {
                "running" => DownloadTaskState::Running,
                "paused" => DownloadTaskState::Paused,
                "completed" => DownloadTaskState::Completed,
                "cancelled" => DownloadTaskState::Cancelled,
                "failed" => DownloadTaskState::Failed,
                "interrupted" => DownloadTaskState::Interrupted,
                _ => DownloadTaskState::Interrupted,
            };
            Ok(DownloadTask {
                id: row.get(0)?,
                instance_id: row.get(1)?,
                label: row.get(2)?,
                source: row.get(3)?,
                state,
                completed_bytes: row.get::<_, i64>(5)?.max(0) as u64,
                total_bytes: row.get::<_, Option<i64>>(6)?.map(|v| v.max(0) as u64),
                created_at: row.get(7)?,
                updated_at: row.get(8)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(StoreError::Database)
    }

    pub fn save_settings(&self, settings: &LauncherSettings) -> Result<(), StoreError> {
        let json = serde_json::to_string(settings)?;
        let connection = self.connection.lock().map_err(|_| StoreError::Poisoned)?;
        connection.execute(
            "INSERT INTO app_settings(id, schema_version, json, updated_at)
             VALUES (1, ?1, ?2, ?3)
             ON CONFLICT(id) DO UPDATE SET
                schema_version = excluded.schema_version,
                json = excluded.json,
                updated_at = excluded.updated_at",
            params![settings.schema_version, json, Utc::now().to_rfc3339()],
        )?;
        Ok(())
    }

    pub fn load_settings(&self) -> Result<LauncherSettings, StoreError> {
        let connection = self.connection.lock().map_err(|_| StoreError::Poisoned)?;
        let json: Option<String> = connection
            .query_row("SELECT json FROM app_settings WHERE id = 1", [], |row| {
                row.get(0)
            })
            .optional()?;
        match json {
            Some(json) => Ok(serde_json::from_str(&json)?),
            None => Ok(LauncherSettings::default()),
        }
    }

    pub fn create_instance(
        &self,
        input: &CreateInstanceInput,
        data_root: &Path,
    ) -> Result<InstanceRecord, StoreError> {
        input.validate().map_err(StoreError::Invalid)?;
        let id = Uuid::new_v4().to_string();
        let instance_dir = data_root.join("instances").join(&id);
        let game_dir = instance_dir.join("game");
        let now = Utc::now().to_rfc3339();
        let instance = InstanceRecord {
            id: id.clone(),
            name: input.name.trim().to_owned(),
            game_version: input.game_version.trim().to_owned(),
            loader: "vanilla".to_owned(),
            instance_dir: instance_dir.clone(),
            game_dir: game_dir.clone(),
            isolated: true,
            state: InstanceState::NotInstalled,
            created_at: now.clone(),
        };
        let connection = self.connection.lock().map_err(|_| StoreError::Poisoned)?;
        connection.execute(
            "INSERT INTO instances(id, name, game_version, loader, instance_dir, game_dir, isolated, state, created_at)
             VALUES (?1, ?2, ?3, 'vanilla', ?4, ?5, 1, 'not_installed', ?6)",
            params![
                instance.id,
                instance.name,
                instance.game_version,
                path_to_string(&instance_dir),
                path_to_string(&game_dir),
                now,
            ],
        )?;
        Ok(instance)
    }

    pub fn list_instances(&self) -> Result<Vec<InstanceRecord>, StoreError> {
        let connection = self.connection.lock().map_err(|_| StoreError::Poisoned)?;
        let mut statement = connection.prepare(
            "SELECT id, name, game_version, loader, instance_dir, game_dir, isolated, state, created_at
             FROM instances ORDER BY created_at DESC",
        )?;
        let rows = statement.query_map([], |row| {
            let state: String = row.get(7)?;
            let state = match state.as_str() {
                "installing" => InstanceState::Installing,
                "ready" => InstanceState::Ready,
                "failed" => InstanceState::Failed,
                _ => InstanceState::NotInstalled,
            };
            Ok(InstanceRecord {
                id: row.get(0)?,
                name: row.get(1)?,
                game_version: row.get(2)?,
                loader: row.get(3)?,
                instance_dir: PathBuf::from(row.get::<_, String>(4)?),
                game_dir: PathBuf::from(row.get::<_, String>(5)?),
                isolated: row.get::<_, i64>(6)? != 0,
                state,
                created_at: row.get(8)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(StoreError::Database)
    }

    pub fn set_instance_state(
        &self,
        instance_id: &str,
        state: InstanceState,
    ) -> Result<(), StoreError> {
        let state_value = match state {
            InstanceState::NotInstalled => "not_installed",
            InstanceState::Installing => "installing",
            InstanceState::Ready => "ready",
            InstanceState::Failed => "failed",
        };
        let connection = self.connection.lock().map_err(|_| StoreError::Poisoned)?;
        let changed = connection.execute(
            "UPDATE instances SET state = ?1 WHERE id = ?2",
            params![state_value, instance_id],
        )?;
        if changed == 0 {
            return Err(StoreError::Invalid(format!(
                "instance {instance_id} does not exist"
            )));
        }
        Ok(())
    }

    pub fn discard_uninstalled_instance(&self, id: &str) -> Result<(), StoreError> {
        let connection = self.connection.lock().map_err(|_| StoreError::Poisoned)?;
        connection.execute(
            "DELETE FROM instances WHERE id = ?1 AND state = 'not_installed'",
            [id],
        )?;
        Ok(())
    }

    /// Delete an instance record. The directory removal is the host's decision:
    /// the store never deletes files, it only drops the row (any state but
    /// `installing`, which may be mid-write in another process).
    pub fn delete_instance(&self, id: &str) -> Result<InstanceRecord, StoreError> {
        let connection = self.connection.lock().map_err(|_| StoreError::Poisoned)?;
        let removed = connection.query_row(
            "DELETE FROM instances WHERE id = ?1 AND state != 'installing' RETURNING id, name, game_version, loader, instance_dir, game_dir, isolated, state, created_at",
            [id],
            |row| {
                let state: String = row.get(7)?;
                Ok(InstanceRecord {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    game_version: row.get(2)?,
                    loader: row.get(3)?,
                    instance_dir: PathBuf::from(row.get::<_, String>(4)?),
                    game_dir: PathBuf::from(row.get::<_, String>(5)?),
                    isolated: row.get::<_, i64>(6)? != 0,
                    state: match state.as_str() {
                        "ready" => InstanceState::Ready,
                        "failed" => InstanceState::Failed,
                        "not_installed" => InstanceState::NotInstalled,
                        _ => InstanceState::Installing,
                    },
                    created_at: row.get(8)?,
                })
            },
        )
        .optional()?;
        removed.ok_or_else(|| StoreError::Invalid(format!("instance {id} cannot be deleted")))
    }

    pub fn set_instance_loader(&self, id: &str, loader: &str) -> Result<(), StoreError> {
        let connection = self.connection.lock().map_err(|_| StoreError::Poisoned)?;
        if connection.execute(
            "UPDATE instances SET loader = ?1 WHERE id = ?2",
            params![loader, id],
        )? == 0
        {
            return Err(StoreError::Invalid("Instance not found".into()));
        }
        Ok(())
    }

    pub fn add_account(&self, account: &AccountProfile) -> Result<(), StoreError> {
        let connection = self.connection.lock().map_err(|_| StoreError::Poisoned)?;
        let tx = connection.unchecked_transaction()?;
        if account.selected {
            tx.execute("UPDATE accounts SET selected = 0", [])?;
        }
        tx.execute(
            "INSERT INTO accounts(id, display_name, kind, auth_server, credential_ref, selected, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                account.id,
                account.display_name,
                serde_json::to_string(&account.kind)?,
                account.auth_server,
                account.credential_ref,
                i64::from(account.selected),
                account.created_at,
            ],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn list_accounts(&self) -> Result<Vec<AccountProfile>, StoreError> {
        let connection = self.connection.lock().map_err(|_| StoreError::Poisoned)?;
        let mut statement = connection.prepare(
            "SELECT id, display_name, kind, auth_server, credential_ref, selected, created_at
             FROM accounts ORDER BY selected DESC, created_at DESC",
        )?;
        let rows = statement.query_map([], |row| {
            let kind_json: String = row.get(2)?;
            let kind = serde_json::from_str::<AccountKind>(&kind_json).map_err(|error| {
                rusqlite::Error::FromSqlConversionFailure(
                    2,
                    rusqlite::types::Type::Text,
                    Box::new(error),
                )
            })?;
            Ok(AccountProfile {
                id: row.get(0)?,
                display_name: row.get(1)?,
                kind,
                auth_server: row.get(3)?,
                credential_ref: row.get(4)?,
                selected: row.get::<_, i64>(5)? != 0,
                created_at: row.get(6)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(StoreError::Database)
    }

    /// Delete a saved account. Its keyring entry must be removed separately by the host.
    /// A deleted selected account leaves no account selected.
    pub fn remove_account(&self, account_id: &str) -> Result<(), StoreError> {
        let connection = self.connection.lock().map_err(|_| StoreError::Poisoned)?;
        let changed = connection.execute("DELETE FROM accounts WHERE id = ?1", [account_id])?;
        if changed == 0 {
            return Err(StoreError::Invalid(format!(
                "account {account_id} does not exist"
            )));
        }
        Ok(())
    }

    pub fn select_account(&self, account_id: &str) -> Result<(), StoreError> {
        let connection = self.connection.lock().map_err(|_| StoreError::Poisoned)?;
        let tx = connection.unchecked_transaction()?;
        let changed = tx.execute("UPDATE accounts SET selected = 0", [])?;
        let selected = tx.execute(
            "UPDATE accounts SET selected = 1 WHERE id = ?1",
            [account_id],
        )?;
        if selected == 0 {
            tx.rollback()?;
            return Err(StoreError::Invalid(format!(
                "account {account_id} does not exist"
            )));
        }
        let _ = changed;
        tx.commit()?;
        Ok(())
    }
}

fn path_to_string(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn download_tasks_persist_and_interrupted_work_is_marked_truthfully() {
        let path = std::env::temp_dir().join(format!("mfmcl-{}.sqlite", Uuid::new_v4()));
        let store = LauncherStore::open(&path).unwrap();
        let now = Utc::now().to_rfc3339();
        let task = DownloadTask {
            id: Uuid::new_v4().to_string(),
            instance_id: "test-instance".into(),
            label: "Minecraft 1.21.4".into(),
            source: Some("official".into()),
            state: DownloadTaskState::Running,
            completed_bytes: 128,
            total_bytes: Some(512),
            created_at: now.clone(),
            updated_at: now,
        };
        store.save_download_task(&task).unwrap();
        assert_eq!(store.list_download_tasks().unwrap()[0].completed_bytes, 128);
        drop(store);
        let recovered = LauncherStore::open(&path).unwrap();
        assert_eq!(
            recovered.list_download_tasks().unwrap()[0].state,
            DownloadTaskState::Interrupted
        );
        drop(recovered);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn account_ipc_serialization_hides_the_keyring_reference() {
        let account = AccountProfile {
            id: "account-id".into(),
            display_name: "Player".into(),
            kind: AccountKind::Microsoft,
            auth_server: None,
            credential_ref: Some("opaque-keyring-entry".into()),
            selected: true,
            created_at: "2026-10-21T00:00:00Z".into(),
        };
        let value = serde_json::to_value(account).unwrap();
        assert!(value.get("credentialRef").is_none());
        assert!(!value.to_string().contains("opaque-keyring-entry"));
    }

    #[test]
    fn removing_an_account_deletes_it_and_clears_selection() {
        let store = LauncherStore::open(":memory:").unwrap();
        let account = AccountProfile {
            id: "ms-1".to_owned(),
            display_name: "Player".to_owned(),
            kind: AccountKind::Microsoft,
            auth_server: None,
            credential_ref: Some("ms-1".to_owned()),
            selected: true,
            created_at: "2026-10-21T00:00:00Z".to_owned(),
        };
        store.add_account(&account).unwrap();
        assert!(store.remove_account("ms-1").is_ok());
        assert!(store.list_accounts().unwrap().is_empty());
        assert!(store.remove_account("ms-1").is_err());
    }

    #[test]
    fn deleting_an_instance_returns_its_record_and_blocks_installing_rows() {
        let root = std::env::temp_dir().join(format!("mfmcl-del-{}", Uuid::new_v4()));
        let store = LauncherStore::open(":memory:").unwrap();
        let created = store
            .create_instance(
                &CreateInstanceInput {
                    name: "Doomed".into(),
                    game_version: "1.21.4".into(),
                },
                &root,
            )
            .unwrap();
        store
            .set_instance_state(&created.id, InstanceState::Ready)
            .unwrap();
        let removed = store.delete_instance(&created.id).unwrap();
        assert_eq!(removed.id, created.id);
        assert!(store.list_instances().unwrap().is_empty());
        assert!(store.delete_instance(&created.id).is_err());

        let installing = store
            .create_instance(
                &CreateInstanceInput {
                    name: "Busy".into(),
                    game_version: "1.21.4".into(),
                },
                &root,
            )
            .unwrap();
        store
            .set_instance_state(&installing.id, InstanceState::Installing)
            .unwrap();
        assert!(store.delete_instance(&installing.id).is_err());
        assert_eq!(store.list_instances().unwrap().len(), 1);
    }

    #[test]
    fn legacy_settings_rows_keep_loading_with_defaults_for_newer_fields() {
        let store = LauncherStore::open(":memory:").unwrap();
        // The exact schema-1 JSON persisted by the first release, before
        // javaPath/memoryMb/updatePolicy existed. Future fields must keep
        // #[serde(default)] so old rows load instead of bricking settings.
        let legacy = r##"{"schemaVersion":1,"themeMode":"dark","themePreset":"moss","customAccent":"#a6d991","customBackground":null,"beginnerMode":true,"showGameLog":true,"download":{"source":"mixed","maxConcurrentDownloads":6,"connectionsPerFile":2,"aria2Enabled":false}}"##;
        let connection = store.connection.lock().unwrap();
        connection
            .execute(
                "INSERT INTO app_settings(id, schema_version, json, updated_at) VALUES (1, 1, ?1, '2026-01-01')",
                [legacy],
            )
            .unwrap();
        drop(connection);
        let loaded = store.load_settings().unwrap();
        assert_eq!(loaded.download.source, mc_metadata::SourcePreference::Mixed);
        assert_eq!(loaded.download.max_concurrent_downloads, 6);
        assert_eq!(loaded.java_path, None);
        assert_eq!(loaded.memory_mb, 2048);
        assert_eq!(loaded.update_policy, crate::model::UpdatePolicy::Notify);
        // Round-tripping modernizes the row without losing legacy values.
        store.save_settings(&loaded).unwrap();
        let reloaded = store.load_settings().unwrap();
        assert_eq!(reloaded.download.source, mc_metadata::SourcePreference::Mixed);
        assert_eq!(reloaded.update_policy, crate::model::UpdatePolicy::Notify);
    }

    #[test]
    fn settings_round_trip_and_instances_are_isolated() {
        let store = LauncherStore::open(":memory:").unwrap();
        let mut settings = LauncherSettings::default();
        settings.download.source = crate::model::DownloadSettings::default().source;
        store.save_settings(&settings).unwrap();
        assert_eq!(store.load_settings().unwrap().schema_version, 1);

        let input = CreateInstanceInput {
            name: "Creative test".to_owned(),
            game_version: "1.21.4".to_owned(),
        };
        let created = store
            .create_instance(&input, Path::new("/tmp/mfmcl-test"))
            .unwrap();
        assert_eq!(created.state, InstanceState::NotInstalled);
        assert!(created.isolated);
        assert_eq!(store.list_instances().unwrap().len(), 1);
    }
}
