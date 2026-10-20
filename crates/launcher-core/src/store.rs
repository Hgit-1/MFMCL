use crate::model::{
    AccountKind, AccountProfile, CreateInstanceInput, InstanceRecord, InstanceState,
    LauncherSettings,
};
use chrono::Utc;
use rusqlite::{Connection, OptionalExtension, params};
use serde_json;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
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

pub struct LauncherStore {
    connection: Mutex<Connection>,
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
        Ok(Self {
            connection: Mutex::new(connection),
        })
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
