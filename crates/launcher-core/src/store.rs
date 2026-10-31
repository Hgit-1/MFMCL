use crate::model::{
    AccountKind, AccountProfile, CreateInstanceInput, DownloadTask, DownloadTaskState,
    InstanceOverrides, InstanceRecord, InstanceState, LauncherSettings,
};
use chrono::Utc;
use mc_metadata::SourcePreference;
use rusqlite::{Connection, OptionalExtension, params};
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

/// 把旧版本的设置升级到当前 schema。
///
/// ## 为什么需要它
///
/// `load_settings` 原本只是反序列化。这意味着旧版本写进数据库的值会**永久冻结**：
/// 即便新版本改了默认值（例如把下载源默认从 `Official` 改成 `Mixed`），老用户
/// 读到的仍是旧默认值，永远享受不到改进。实测 2026-10-27 写入的库读出来是
/// `schemaVersion=2` + `source=Official`，而当前默认是 3 + `Mixed`。
///
/// ## 为什么不一概重置
///
/// 不能简单地把所有字段覆盖成新默认值——那会抹掉用户真实的偏好（主题、内存、
/// Java 路径等）。所以只迁移「旧默认值 → 新默认值」这一种情况：只有当某个字段
/// **等于它所在版本的默认值**时才替换。
///
/// 这样做的代价是无法区分「用户主动选了 official」与「用户从未选过」。这是可接受
/// 的：旧版本里 `Official` 是唯一默认，绝大多数用户没有主动改过；而愿意主动选
/// 官方源的用户，在设置页里能一眼看到当前值并改回去。
fn migrate_settings(settings: &mut LauncherSettings) {
    /*
     * schema 1/2 → 3：下载源默认值由 Official 改为 Mixed（镜像优先、官方回退）。
     * 只在本字段仍是旧默认值时改写，用户明确选过 Bmclapi 的不受影响。
     */
    if settings.schema_version < 3 && settings.download.source == SourcePreference::Official {
        settings.download.source = SourcePreference::Mixed;
    }
    // 迁移完成后标记为当前版本，避免下次重复处理。
    settings.schema_version = LauncherSettings::default().schema_version;
}

impl LauncherStore {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, StoreError> {
        let database_path = path.as_ref();
        if database_path != Path::new(":memory:")
            && let Some(parent) = database_path
                .parent()
                .filter(|parent| !parent.as_os_str().is_empty())
        {
            std::fs::create_dir_all(parent).map_err(|error| {
                StoreError::Invalid(format!("could not create app data directory: {error}"))
            })?;
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
            /*
             * Per-instance overrides live in their own table rather than in the
             * instances row. Two reasons: the JSON shape is free to grow without
             * a column-per-setting migration, and an instance with no overrides
             * stores no row at all, so inheriting everything is the physical
             * default instead of an all-NULL row that has to be interpreted.
             */
            CREATE TABLE IF NOT EXISTS instance_overrides (
                instance_id TEXT PRIMARY KEY REFERENCES instances(id) ON DELETE CASCADE,
                json TEXT NOT NULL,
                updated_at TEXT NOT NULL
            );
            /*
             * 插件状态。
             *
             * 单行保存整份 JSON，而不是给每个插件建一行：插件清单的结构会随版本
             * 增长（新增贡献类型、新增权限），per-column 迁移的代价远高于整份
             * JSON 加逐字段校验。这里存的只是**用户状态**（装了哪些、启用了哪些），
             * 内置清单在代码里，不进库。
             */
            CREATE TABLE IF NOT EXISTS plugin_state (
                id INTEGER PRIMARY KEY CHECK (id = 1),
                json TEXT NOT NULL,
                updated_at TEXT NOT NULL
            );
            /*
             * 启动历史。
             *
             * 为什么记在库里而不是只写日志文件：用户要看的是「哪次启动、哪个实例、
             * 插件做了什么」，这是结构化数据。日志文件是给人排查崩溃用的，塞进去
             * 之后既难查询也难展示。
             *
             * `contributions` 存 JSON：一次启动里插件贡献的条数不定，建成独立表会
             * 让「取最近 N 次启动」变成 N 次 join。
             */
            CREATE TABLE IF NOT EXISTS launch_history (
                id TEXT PRIMARY KEY,
                instance_id TEXT NOT NULL,
                instance_name TEXT NOT NULL,
                started_at TEXT NOT NULL,
                finished_at TEXT,
                exit_code INTEGER,
                outcome TEXT NOT NULL,
                contributions TEXT NOT NULL DEFAULT '{}'
            );
            CREATE INDEX IF NOT EXISTS launch_history_started_at
                ON launch_history(started_at DESC);
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
        /*
         * 记录上次启动时间，供内容商店的「目标实例」默认为最近玩过的实例。
         *
         * 为什么加列而不是复用 `created_at`：创建时间和游玩时间无关，用创建顺序
         * 猜「上次启动」会在用户新建实例但没启动时给出错误答案。
         *
         * 为什么可空而不是填默认值：老库里的实例确实没有启动记录，填一个假时间
         * 会让排序看起来可信却是编的。NULL 表示「从未启动」，排序时排到最后。
         */
        let has_last_launched = {
            let mut columns = connection.prepare("PRAGMA table_info(instances)")?;
            let names = columns
                .query_map([], |row| row.get::<_, String>(1))?
                .collect::<Result<Vec<_>, _>>()?;
            names.iter().any(|name| name == "last_launched_at")
        };
        if !has_last_launched {
            connection.execute("ALTER TABLE instances ADD COLUMN last_launched_at TEXT", [])?;
            connection.execute("INSERT OR IGNORE INTO schema_migrations(version, applied_at) VALUES (3, datetime('now'))", [])?;
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
        crate::validate_settings(settings, None).map_err(StoreError::Invalid)?;
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
            Some(json) => {
                let mut settings: LauncherSettings = serde_json::from_str(&json)?;
                let before = settings.schema_version;
                migrate_settings(&mut settings);
                /*
                 * 迁移后立即写回。
                 *
                 * 只在内存里迁移、不落盘的话，数据库会一直停在旧格式：每次启动都要
                 * 重新迁移一次，而且任何直接读库的地方（诊断导出、外部工具）看到的
                 * 都是过期值。实测迁移逻辑生效后库里的 `schemaVersion` 仍是 2，
                 * 就是因为漏了这一步。
                 */
                if settings.schema_version != before {
                    let updated = chrono::Utc::now().to_rfc3339();
                    connection.execute(
                        "UPDATE app_settings SET json = ?1, schema_version = ?2, updated_at = ?3 WHERE id = 1",
                        rusqlite::params![
                            serde_json::to_string(&settings)?,
                            settings.schema_version as i64,
                            updated
                        ],
                    )?;
                }
                Ok(settings)
            }
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
            // 刚创建的实例还没启动过。
            last_launched_at: None,
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
        /*
         * 排序：最近启动过的在前，从未启动过的按创建时间倒序。
         *
         * 用 `IS NULL` 先分组而不是直接 `ORDER BY last_launched_at DESC`：SQLite 里
         * NULL 在 DESC 时会排在最前，那样从未启动过的新实例反而置顶，「上次启动的
         * 实例」这个默认就失效了。
         */
        let mut statement = connection.prepare(
            "SELECT id, name, game_version, loader, instance_dir, game_dir, isolated, state, created_at, last_launched_at
             FROM instances
             ORDER BY last_launched_at IS NULL, last_launched_at DESC, created_at DESC",
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
                last_launched_at: row.get(9)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(StoreError::Database)
    }

    /// 记录一次成功启动，用于「默认目标实例」与「最近游玩」排序。
    ///
    /// 只在启动成功后调用：启动失败不该改变「上次玩的是哪个实例」，否则用户看到
    /// 的目标实例会是他刚刚启动失败的那个。
    pub fn mark_instance_launched(&self, instance_id: &str) -> Result<(), StoreError> {
        let connection = self.connection.lock().map_err(|_| StoreError::Poisoned)?;
        connection.execute(
            "UPDATE instances SET last_launched_at = ? WHERE id = ?",
            params![Utc::now().to_rfc3339(), instance_id],
        )?;
        Ok(())
    }

    // -----------------------------------------------------------------------
    // 插件状态
    // -----------------------------------------------------------------------

    /// 读取插件用户状态。没有保存过时返回默认值（无自定义插件、无覆盖）。
    pub fn load_plugin_state(&self) -> Result<crate::plugins::PluginState, StoreError> {
        let connection = self.connection.lock().map_err(|_| StoreError::Poisoned)?;
        let json: Option<String> = connection
            .query_row("SELECT json FROM plugin_state WHERE id = 1", [], |row| {
                row.get(0)
            })
            .optional()?;
        let Some(json) = json else {
            return Ok(crate::plugins::PluginState::default());
        };
        /*
         * 存档损坏时回落到默认值而不是报错。
         *
         * 插件状态是「便利配置」，一个字段写坏不该让插件页整页打不开。逐字段
         * 校验由 serde 的默认值完成（`#[serde(default)]`），解析失败只丢内容。
         */
        Ok(serde_json::from_str(&json).unwrap_or_default())
    }

    pub fn save_plugin_state(&self, state: &crate::plugins::PluginState) -> Result<(), StoreError> {
        let json = serde_json::to_string(state)?;
        let connection = self.connection.lock().map_err(|_| StoreError::Poisoned)?;
        connection.execute(
            "INSERT INTO plugin_state(id, json, updated_at) VALUES (1, ?1, ?2)
             ON CONFLICT(id) DO UPDATE SET json = excluded.json, updated_at = excluded.updated_at",
            params![json, Utc::now().to_rfc3339()],
        )?;
        Ok(())
    }

    // -----------------------------------------------------------------------
    // 启动历史
    // -----------------------------------------------------------------------

    /**
     * 登记一次启动。
     *
     * 在 JVM 进程真正起来之后调用，与 `mark_instance_launched` 同一时机：起不来
     * 的那次不算「玩过」，也不该出现在历史里。返回记录 id，供进程结束时回填结果。
     */
    pub fn record_launch_start(
        &self,
        entry: &crate::plugins::LaunchHistoryEntry,
    ) -> Result<(), StoreError> {
        let contributions = serde_json::to_string(&entry.contributions)?;
        let connection = self.connection.lock().map_err(|_| StoreError::Poisoned)?;
        connection.execute(
            "INSERT INTO launch_history(id, instance_id, instance_name, started_at, exit_code, outcome, contributions)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                entry.id,
                entry.instance_id,
                entry.instance_name,
                entry.started_at,
                entry.exit_code,
                entry.outcome,
                contributions
            ],
        )?;
        Ok(())
    }

    /** 进程结束时回填结果。记录不存在时静默失败——历史不该影响启动流程。 */
    pub fn finish_launch(
        &self,
        id: &str,
        outcome: &str,
        exit_code: Option<i32>,
    ) -> Result<(), StoreError> {
        let connection = self.connection.lock().map_err(|_| StoreError::Poisoned)?;
        connection.execute(
            "UPDATE launch_history SET outcome = ?1, exit_code = ?2, finished_at = ?3 WHERE id = ?4",
            params![outcome, exit_code, Utc::now().to_rfc3339(), id],
        )?;
        Ok(())
    }

    /**
     * 最近的启动记录。
     *
     * 只取 200 条：历史是给人回看「刚才那次启动发生了什么」的，不是审计日志。
     * 无上限会让数据库随时间无限增长，而用户永远不会翻到第 500 条。
     */
    pub fn list_launch_history(&self) -> Result<Vec<crate::plugins::LaunchHistoryEntry>, StoreError> {
        let connection = self.connection.lock().map_err(|_| StoreError::Poisoned)?;
        let mut statement = connection.prepare(
            "SELECT id, instance_id, instance_name, started_at, exit_code, outcome, contributions
             FROM launch_history ORDER BY started_at DESC LIMIT 200",
        )?;
        let rows = statement.query_map([], |row| {
            let contributions: String = row.get(6)?;
            Ok(crate::plugins::LaunchHistoryEntry {
                id: row.get(0)?,
                instance_id: row.get(1)?,
                instance_name: row.get(2)?,
                started_at: row.get(3)?,
                exit_code: row.get(4)?,
                outcome: row.get(5)?,
                // 解析失败时给空报告：能读到「启动过」比整条记录消失重要。
                contributions: serde_json::from_str(&contributions).unwrap_or_default(),
            })
        })?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    /** 清空启动历史。用户在插件页点「清空记录」时调用。 */
    pub fn clear_launch_history(&self) -> Result<usize, StoreError> {
        let connection = self.connection.lock().map_err(|_| StoreError::Poisoned)?;
        Ok(connection.execute("DELETE FROM launch_history", [])?)
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
            "DELETE FROM instances WHERE id = ?1 AND state != 'installing' RETURNING id, name, game_version, loader, instance_dir, game_dir, isolated, state, created_at, last_launched_at",
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
                    last_launched_at: row.get(9)?,
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

    /// Read one instance's overrides. A missing row means "inherits everything",
    /// which is why this returns the type default rather than `None`: callers
    /// always want a usable value, and an empty override set resolves to exactly
    /// the global settings.
    pub fn load_instance_overrides(
        &self,
        instance_id: &str,
    ) -> Result<InstanceOverrides, StoreError> {
        let connection = self.connection.lock().map_err(|_| StoreError::Poisoned)?;
        let json: Option<String> = connection
            .query_row(
                "SELECT json FROM instance_overrides WHERE instance_id = ?1",
                [instance_id],
                |row| row.get(0),
            )
            .optional()?;
        let Some(json) = json else {
            return Ok(InstanceOverrides::default());
        };
        // A row that cannot be parsed is a corrupted store, not an empty one.
        // Surfacing the error is better than silently launching with settings the
        // user did not choose.
        serde_json::from_str(&json).map_err(StoreError::from)
    }

    /// Persist overrides, or delete the row when there is nothing to store.
    ///
    /// Writing an all-`None` payload therefore erases the row, so "跟随全局" on
    /// every row leaves no trace behind — the instance goes back to being the
    /// physical default instead of a record full of nulls.
    pub fn save_instance_overrides(
        &self,
        instance_id: &str,
        overrides: &InstanceOverrides,
    ) -> Result<(), StoreError> {
        let connection = self.connection.lock().map_err(|_| StoreError::Poisoned)?;
        if *overrides == InstanceOverrides::default() {
            connection.execute(
                "DELETE FROM instance_overrides WHERE instance_id = ?1",
                [instance_id],
            )?;
            return Ok(());
        }
        if connection
            .query_row(
                "SELECT 1 FROM instances WHERE id = ?1",
                [instance_id],
                |_| Ok(()),
            )
            .optional()?
            .is_none()
        {
            return Err(StoreError::Invalid("Instance not found".into()));
        }
        let json = serde_json::to_string(overrides)?;
        connection.execute(
            "INSERT INTO instance_overrides(instance_id, json, updated_at)
             VALUES (?1, ?2, datetime('now'))
             ON CONFLICT(instance_id) DO UPDATE SET json = excluded.json, updated_at = excluded.updated_at",
            params![instance_id, json],
        )?;
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
        assert_eq!(loaded.memory_mode, crate::model::MemoryMode::Custom);
        assert_eq!(
            loaded.garbage_collector,
            crate::model::GarbageCollector::Auto
        );
        assert_eq!(
            loaded.launcher_visibility,
            crate::model::LauncherVisibility::Keep
        );
        let encoded = serde_json::to_value(&loaded).unwrap();
        assert_eq!(encoded["memoryMode"], "custom");
        assert_eq!(encoded["garbageCollector"], "auto");
        assert_eq!(encoded["launcherVisibility"], "keep");
        // Round-tripping modernizes the row without losing legacy values.
        store.save_settings(&loaded).unwrap();
        let reloaded = store.load_settings().unwrap();
        assert_eq!(
            reloaded.download.source,
            mc_metadata::SourcePreference::Mixed
        );
        assert_eq!(reloaded.update_policy, crate::model::UpdatePolicy::Notify);
    }

    #[test]
    fn overrides_round_trip_and_clear_to_a_missing_row() {
        let root = std::env::temp_dir().join(format!("mfmcl-ovr-{}", Uuid::new_v4()));
        let store = LauncherStore::open(":memory:").unwrap();
        let created = store
            .create_instance(
                &CreateInstanceInput {
                    name: "Overrides".into(),
                    game_version: "1.21.4".into(),
                },
                &root,
            )
            .unwrap();

        // Never-written instance: no row, and reading yields the type default.
        assert_eq!(
            store.load_instance_overrides(&created.id).unwrap(),
            crate::InstanceOverrides::default()
        );

        let overrides = crate::InstanceOverrides {
            memory_mode: Some(crate::MemoryMode::Custom),
            memory_mb: Some(6144),
            fullscreen: Some(true),
            window_width: Some(1600),
            window_height: Some(900),
            extra_jvm_args: Some("-XX:+UseZGC".into()),
            ..crate::InstanceOverrides::default()
        };
        store
            .save_instance_overrides(&created.id, &overrides)
            .unwrap();
        assert_eq!(
            store.load_instance_overrides(&created.id).unwrap(),
            overrides
        );

        // Saving the default erases the row rather than storing nulls, so the
        // physical state matches the meaning of "inherits everything".
        store
            .save_instance_overrides(&created.id, &crate::InstanceOverrides::default())
            .unwrap();
        let rows: i64 = {
            let connection = store.connection.lock().unwrap();
            connection
                .query_row("SELECT COUNT(*) FROM instance_overrides", [], |row| {
                    row.get(0)
                })
                .unwrap()
        };
        assert_eq!(rows, 0);
        assert_eq!(
            store.load_instance_overrides(&created.id).unwrap(),
            crate::InstanceOverrides::default()
        );
    }

    #[test]
    fn overrides_for_unknown_instances_are_rejected_and_deletes_cascade() {
        let root = std::env::temp_dir().join(format!("mfmcl-ovr-{}", Uuid::new_v4()));
        let store = LauncherStore::open(":memory:").unwrap();
        // Writing overrides for an instance that does not exist must fail: a
        // dangling row would never be read and would hide the real mistake.
        assert!(
            store
                .save_instance_overrides(
                    "missing",
                    &crate::InstanceOverrides {
                        fullscreen: Some(true),
                        ..crate::InstanceOverrides::default()
                    }
                )
                .is_err()
        );

        let created = store
            .create_instance(
                &CreateInstanceInput {
                    name: "Cascade".into(),
                    game_version: "1.21.4".into(),
                },
                &root,
            )
            .unwrap();
        store
            .save_instance_overrides(
                &created.id,
                &crate::InstanceOverrides {
                    fullscreen: Some(false),
                    ..crate::InstanceOverrides::default()
                },
            )
            .unwrap();
        store.delete_instance(&created.id).unwrap();
        let rows: i64 = {
            let connection = store.connection.lock().unwrap();
            connection
                .query_row("SELECT COUNT(*) FROM instance_overrides", [], |row| {
                    row.get(0)
                })
                .unwrap()
        };
        // ON DELETE CASCADE keeps overrides from resurrecting onto a future
        // instance that happens to reuse the id.
        assert_eq!(rows, 0);
    }

    #[test]
    fn settings_round_trip_and_instances_are_isolated() {
        let store = LauncherStore::open(":memory:").unwrap();
        let mut settings = LauncherSettings::default();
        settings.download.source = crate::model::DownloadSettings::default().source;
        store.save_settings(&settings).unwrap();
        assert_eq!(store.load_settings().unwrap().schema_version, 3);

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
    #[test]
    fn last_launched_orders_instances_and_survives_reopen() {
        let path = std::env::temp_dir().join(format!("mfmcl-{}.sqlite", Uuid::new_v4()));
        let store = LauncherStore::open(&path).unwrap();

        let make = |name: &str| CreateInstanceInput {
            name: name.to_owned(),
            game_version: "1.21.4".to_owned(),
        };
        let first = store
            .create_instance(&make("first"), Path::new("/tmp/mfmcl-test"))
            .unwrap();
        let second = store
            .create_instance(&make("second"), Path::new("/tmp/mfmcl-test"))
            .unwrap();

        // 刚创建的实例都没有启动记录。
        assert!(first.last_launched_at.is_none());
        assert!(second.last_launched_at.is_none());

        // 启动较早创建的那个：它应当排到最前，成为「上次启动的实例」。
        store.mark_instance_launched(&first.id).unwrap();
        let listed = store.list_instances().unwrap();
        assert_eq!(listed[0].id, first.id, "最近启动的实例必须排第一");
        assert!(listed[0].last_launched_at.is_some());
        assert!(listed[1].last_launched_at.is_none());

        // 再启动另一个，顺序翻转。
        store.mark_instance_launched(&second.id).unwrap();
        assert_eq!(store.list_instances().unwrap()[0].id, second.id);

        // 重开数据库后仍然保留（列迁移 + 持久化都要对）。
        drop(store);
        let reopened = LauncherStore::open(&path).unwrap();
        let after = reopened.list_instances().unwrap();
        assert_eq!(after[0].id, second.id);
        assert!(after[0].last_launched_at.is_some());

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn never_launched_instances_sort_by_creation_not_by_null_first() {
        let store = LauncherStore::open(":memory:").unwrap();
        let make = |name: &str| CreateInstanceInput {
            name: name.to_owned(),
            game_version: "1.21.4".to_owned(),
        };
        let old = store
            .create_instance(&make("old"), Path::new("/tmp/mfmcl-test"))
            .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(1100));
        let new = store
            .create_instance(&make("new"), Path::new("/tmp/mfmcl-test"))
            .unwrap();

        // 都从未启动：按创建时间倒序，新的在前。
        // （SQLite 的 DESC 会把 NULL 排最前，所以查询里必须显式分组。）
        let listed = store.list_instances().unwrap();
        assert_eq!(listed[0].id, new.id);
        assert_eq!(listed[1].id, old.id);

        // 启动旧的那个之后，它必须排到最前，压过更新的创建时间。
        store.mark_instance_launched(&old.id).unwrap();
        assert_eq!(store.list_instances().unwrap()[0].id, old.id);
    }
    #[test]
    fn old_settings_are_migrated_without_losing_user_preferences() {
        let store = LauncherStore::open(":memory:").unwrap();
        /*
         * 模拟 2026-10-27 那一版写入的设置：schema 2，下载源是旧默认值 Official，
         * 但主题是用户改过的 Light。
         */
        let mut old = LauncherSettings::default();
        old.schema_version = 2;
        old.download.source = mc_metadata::SourcePreference::Official;
        old.theme_mode = crate::model::ThemeMode::Light;
        store.save_settings(&old).unwrap();

        let loaded = store.load_settings().unwrap();
        // 旧默认值被升级到新默认值。
        assert_eq!(loaded.download.source, mc_metadata::SourcePreference::Mixed);
        assert_eq!(loaded.schema_version, 3);
        // 用户真实偏好必须原样保留。
        assert_eq!(loaded.theme_mode, crate::model::ThemeMode::Light);
    }

    #[test]
    fn explicit_mirror_choice_survives_migration() {
        let store = LauncherStore::open(":memory:").unwrap();
        let mut old = LauncherSettings::default();
        old.schema_version = 2;
        // 用户明确选了 BMCLAPI，不该被迁移逻辑覆盖。
        old.download.source = mc_metadata::SourcePreference::Bmclapi;
        store.save_settings(&old).unwrap();

        assert_eq!(
            store.load_settings().unwrap().download.source,
            mc_metadata::SourcePreference::Bmclapi
        );
    }

    #[test]
    fn plugin_state_round_trips_and_defaults_when_absent() {
        let store = LauncherStore::open(":memory:").unwrap();
        // Never saved: no custom plugins, no overrides.
        let initial = store.load_plugin_state().unwrap();
        assert!(initial.custom.is_empty());
        assert!(initial.enabled.is_empty());

        let mut state = crate::plugins::PluginState::default();
        state.enabled.insert("auto-backup".into(), false);
        state.custom.push(crate::plugins::PluginDefinition {
            id: "third-party".into(),
            version: "2.0.0".into(),
            hooks: vec!["launch".into()],
            permissions: vec!["launchHook".into()],
            contributions: vec![crate::plugins::Contribution {
                kind: crate::plugins::ContributionKind::JvmArgs,
                value: "-Dfoo=bar".into(),
            }],
            incompatible: false,
        });
        store.save_plugin_state(&state).unwrap();

        let reloaded = store.load_plugin_state().unwrap();
        assert_eq!(reloaded.enabled.get("auto-backup"), Some(&false));
        assert_eq!(reloaded.custom.len(), 1);
        assert_eq!(reloaded.custom[0].id, "third-party");
        assert_eq!(reloaded.custom[0].contributions.len(), 1);
    }

    #[test]
    fn launch_history_records_outcome_and_reports() {
        let store = LauncherStore::open(":memory:").unwrap();
        let mut report = crate::plugins::LaunchContributionReport::default();
        report.applied.push(crate::plugins::AppliedContribution {
            plugin_id: "download-mirror".into(),
            kind: crate::plugins::ContributionKind::Env,
            summary: "MFMCL_PREFER_MIRROR".into(),
        });
        store
            .record_launch_start(&crate::plugins::LaunchHistoryEntry {
                id: "launch-1".into(),
                instance_id: "inst-1".into(),
                instance_name: "Survival".into(),
                started_at: chrono::Utc::now().to_rfc3339(),
                exit_code: None,
                outcome: "running".into(),
                contributions: report,
            })
            .unwrap();

        let running = store.list_launch_history().unwrap();
        assert_eq!(running.len(), 1);
        assert_eq!(running[0].outcome, "running");
        assert_eq!(running[0].contributions.applied.len(), 1);
        assert_eq!(running[0].exit_code, None);

        store.finish_launch("launch-1", "crashed", Some(1)).unwrap();
        let finished = store.list_launch_history().unwrap();
        assert_eq!(finished[0].outcome, "crashed");
        assert_eq!(finished[0].exit_code, Some(1));
        // 贡献记录不因回填结果而丢失。
        assert_eq!(finished[0].contributions.applied.len(), 1);

        assert_eq!(store.clear_launch_history().unwrap(), 1);
        assert!(store.list_launch_history().unwrap().is_empty());
    }
}
