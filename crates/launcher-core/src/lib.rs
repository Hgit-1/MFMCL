pub mod backups;
pub mod catalog;
pub mod curseforge;
mod diagnostics;
pub mod forge_install;
mod installer;
mod launch;
pub mod loaders;
pub mod logs;
pub mod mod_updates;
mod model;
pub mod modpacks;
pub mod mods;
pub mod plugins;
pub mod settings;
mod store;
pub mod updates;

pub use catalog::{CatalogMod, CatalogVersion};
pub use diagnostics::{
    DiagnosticError, DiagnosticReason, DiagnosticSummary, export_instance_diagnostics,
    summarize_instance,
};
pub use installer::{
    ClientUpdateStatus, InstallError, check_client_update, install_vanilla, repair_instance,
};
pub use launch::{
    JavaRuntimeInfo, LaunchError, LaunchIdentity, PreparedLaunch, SystemMemoryInfo,
    detect_java_runtime, discover_java_runtimes, inspect_java_path, prepare_authenticated_launch,
    prepare_offline_launch, system_memory_info,
};
pub use model::{
    AccountKind, AccountProfile, CreateInstanceInput, DownloadSettings, DownloadTask,
    DownloadTaskState, GarbageCollector, InstanceOverrides, InstanceRecord, InstanceState,
    LauncherSettings, LauncherVisibility, MemoryMode, ProcessPriority, ThemeMode, ThemePreset,
    UpdatePolicy,
};
pub use settings::{
    LaunchSettingsError, apply_launch_settings, count_instance_overrides, resolve_settings,
    validate_instance_overrides, validate_settings,
};
pub use plugins::{
    AppliedContribution, Contribution, ContributionKind, ContributionPlan, LaunchContributionReport,
    LaunchHistoryEntry, PluginDefinition, PluginState, SkippedContribution, build_plan,
    contribution_verdict, insert_jvm_args, validate_contribution,
};
pub use store::{LauncherStore, StoreError};pub use updates::{PlatformRelease, UpdateError, UpdateManifest, Version};
