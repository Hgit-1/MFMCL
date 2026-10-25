pub mod backups;
pub mod catalog;
mod diagnostics;
mod installer;
mod launch;
pub mod loaders;
pub mod logs;
pub mod mod_updates;
mod model;
pub mod modpacks;
pub mod mods;
pub mod settings;
mod store;
pub mod updates;

pub use catalog::{CatalogMod, CatalogVersion};
pub use diagnostics::{
    DiagnosticError, DiagnosticSummary, export_instance_diagnostics, summarize_instance,
};
pub use installer::{ClientUpdateStatus, InstallError, check_client_update, install_vanilla};
pub use launch::{
    JavaRuntimeInfo, LaunchError, LaunchIdentity, PreparedLaunch, SystemMemoryInfo,
    detect_java_runtime, discover_java_runtimes, inspect_java_path, prepare_authenticated_launch,
    prepare_offline_launch, system_memory_info,
};
pub use model::{
    AccountKind, AccountProfile, CreateInstanceInput, DownloadSettings, DownloadTask,
    DownloadTaskState, GarbageCollector, InstanceRecord, InstanceState, LauncherSettings,
    LauncherVisibility, MemoryMode, ProcessPriority, ThemeMode, ThemePreset, UpdatePolicy,
};
pub use settings::{LaunchSettingsError, apply_launch_settings, validate_settings};
pub use store::{LauncherStore, StoreError};
pub use updates::{PlatformRelease, UpdateError, UpdateManifest, Version};
