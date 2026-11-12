//! `mfmcl list` — every "show me what exists" query in one place.
//!
//! ## Why one command instead of five
//!
//! `mfmcl list instances`, `list versions`, `list accounts`, `list tasks`, and
//! `list mods` answer the same kind of question, and grouping them means one `--json`
//! behaviour, one table style, and one place to look. Separate top-level commands
//! (`mfmcl instances`, `mfmcl versions`) would triple the help surface for no gain.

use crate::cli::{ListArgs, ListWhat};
use crate::commands::download::terminal_width;
use crate::commands::{Failure, Outcome, Session, resolve_instance};
use crate::output::{Palette, Table, bytes, timestamp, truncate};
use launcher_core::InstanceState;
use mc_metadata::{SourcePreference, fetch_manifest};

pub async fn run(session: &Session<'_>, args: &ListArgs) -> Outcome {
    let palette = Palette::new(session.sink.palette().on);
    let json = session.cli.json;

    match &args.what {
        ListWhat::Instances => instances(session, &palette, json),
        ListWhat::Accounts => accounts(session, &palette, json),
        ListWhat::Tasks => tasks(session, &palette, json),
        ListWhat::Mods { instance } => mods(session, &palette, json, instance),
        ListWhat::ModVersions {
            project,
            instance,
            limit,
        } => mod_versions(session, &palette, json, project, instance, *limit).await,
        ListWhat::Search {
            query,
            instance,
            project_type,
            limit,
        } => {
            search(
                session,
                &palette,
                json,
                query.as_deref(),
                instance.as_deref(),
                project_type,
                *limit,
            )
            .await
        }
        ListWhat::Versions { limit, snapshots } => {
            versions(session, &palette, json, *limit, *snapshots).await
        }
        ListWhat::Loaders {
            loader,
            game,
            limit,
        } => loaders(session, &palette, json, loader, game, *limit).await,
    }
}

fn instances(session: &Session<'_>, palette: &Palette, json: bool) -> Outcome {
    let instances = session.store.list_instances()?;
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&instances).unwrap_or_default()
        );
        return Ok(());
    }
    if instances.is_empty() {
        println!("还没有任何实例。");
        println!(
            "{}",
            palette.dim("创建：mfmcl download --game 1.21.1 --mod-loader fabric latest")
        );
        return Ok(());
    }
    let mut table =
        Table::new(&["实例", "Minecraft", "加载器", "状态", "大小", "上次启动"]).numeric(&[4]);
    for instance in &instances {
        let state = match instance.state {
            InstanceState::Ready => palette.green("已就绪"),
            InstanceState::NotInstalled => palette.yellow("未安装"),
            InstanceState::Installing => palette.yellow("安装中"),
            InstanceState::Failed => palette.red("失败"),
        };
        table.row(vec![
            instance.name.clone(),
            instance.game_version.clone(),
            instance.loader.clone(),
            state,
            bytes(directory_size(&instance.instance_dir)),
            instance
                .last_launched_at
                .as_deref()
                .map(timestamp)
                .unwrap_or_else(|| "从未".to_owned()),
        ]);
    }
    print!("{}", table.render(palette, terminal_width()));
    println!();
    println!("{}", palette.dim(&format!("共 {} 个实例", instances.len())));
    Ok(())
}

fn accounts(session: &Session<'_>, palette: &Palette, json: bool) -> Outcome {
    let accounts = session.store.list_accounts()?;
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&accounts).unwrap_or_default()
        );
        return Ok(());
    }
    if accounts.is_empty() {
        println!("还没有任何账户。");
        println!(
            "{}",
            palette.dim("添加离线档案：mfmcl account add-offline YourName")
        );
        return Ok(());
    }
    let mut table = Table::new(&["", "名称", "类型", "id"]);
    for account in &accounts {
        let kind = match account.kind {
            launcher_core::AccountKind::Offline => "离线",
            launcher_core::AccountKind::Microsoft => "微软",
            launcher_core::AccountKind::SkinServer => "皮肤站",
        };
        table.row(vec![
            if account.selected {
                palette.green("●")
            } else {
                " ".to_owned()
            },
            account.display_player_name().to_owned(),
            kind.to_owned(),
            account.id.clone(),
        ]);
    }
    print!("{}", table.render(palette, terminal_width()));
    println!();
    println!("{}", palette.dim("● 表示当前选中的账户"));
    Ok(())
}

fn tasks(session: &Session<'_>, palette: &Palette, json: bool) -> Outcome {
    let tasks = session.store.list_download_tasks()?;
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&tasks).unwrap_or_default()
        );
        return Ok(());
    }
    if tasks.is_empty() {
        println!("没有下载任务记录。");
        return Ok(());
    }
    let mut table = Table::new(&["任务", "状态", "进度", "更新时间"]);
    for task in tasks.iter().take(20) {
        let state = match task.state {
            launcher_core::DownloadTaskState::Running => palette.cyan("进行中"),
            launcher_core::DownloadTaskState::Paused => palette.yellow("已暂停"),
            launcher_core::DownloadTaskState::Completed => palette.green("已完成"),
            launcher_core::DownloadTaskState::Cancelled => palette.dim("已取消"),
            launcher_core::DownloadTaskState::Failed => palette.red("失败"),
            launcher_core::DownloadTaskState::Interrupted => palette.yellow("已中断"),
        };
        table.row(vec![
            truncate(&task.label, 40),
            state,
            crate::output::progress_text(task.completed_bytes, task.total_bytes),
            timestamp(&task.updated_at),
        ]);
    }
    print!("{}", table.render(palette, terminal_width()));
    Ok(())
}

fn mods(session: &Session<'_>, palette: &Palette, json: bool, target: &str) -> Outcome {
    let instance = resolve_instance(&session.store, target)?;
    let mods = launcher_core::mods::list_mods(&instance).map_err(Failure::message)?;
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&mods).unwrap_or_default()
        );
        return Ok(());
    }
    if mods.is_empty() {
        println!("实例「{}」的 mods 目录里没有模组。", instance.name);
        return Ok(());
    }
    let mut table = Table::new(&["文件", "启用", "大小"]);
    for item in &mods {
        table.row(vec![
            item.file_name.clone(),
            if item.enabled {
                palette.green("是")
            } else {
                palette.dim("否")
            },
            bytes(item.size),
        ]);
    }
    print!("{}", table.render(palette, terminal_width()));
    println!();
    let enabled = mods.iter().filter(|item| item.enabled).count();
    println!(
        "{}",
        palette.dim(&format!("{} 个模组，其中 {enabled} 个已启用", mods.len()))
    );
    Ok(())
}

/// Columns to draw into, mirroring `download`'s rule. See `download::terminal_width`.
fn width() -> usize {
    crate::commands::download::terminal_width()
}

/// List the versions of one Modrinth project that fit an instance.
///
/// Answers the question `download --mod project@version` raises: "which versions
/// are there?" Without it, a pinned install that fails leaves the user guessing at
/// version numbers.
async fn mod_versions(
    session: &Session<'_>,
    palette: &Palette,
    json: bool,
    project: &str,
    target: &str,
    limit: usize,
) -> Outcome {
    let instance = resolve_instance(&session.store, target)?;
    if instance.loader == "vanilla" {
        return Err(Failure::message(format!(
            "实例「{}」是原版，模组版本列表需要加载器决定兼容性。\n\
             改用一个带加载器的实例，或先 `mfmcl download --mod-loader fabric`。",
            instance.name
        )));
    }
    let client = launcher_core::catalog::catalog_client().map_err(Failure::message)?;
    let versions = launcher_core::catalog::mod_versions(&client, &instance, project)
        .await
        .map_err(Failure::message)?;
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&versions).unwrap_or_default()
        );
        return Ok(());
    }
    if versions.is_empty() {
        println!(
            "「{project}」没有兼容 Minecraft {} / {} 的版本。",
            instance.game_version, instance.loader
        );
        return Ok(());
    }
    let mut table = Table::new(&["版本", "类型", "文件名", "大小", "前置"]).numeric(&[3, 4]);
    for version in versions.iter().take(limit) {
        table.row(vec![
            version.version_number.clone(),
            if version.version_type == "release" {
                palette.green("正式")
            } else {
                palette.dim(&version.version_type)
            },
            truncate(&version.file_name, 40),
            bytes(version.size),
            version.dependency_count.to_string(),
        ]);
    }
    print!("{}", table.render(palette, width()));
    println!();
    println!(
        "{}",
        palette.dim(&format!(
            "共 {} 个兼容版本；安装指定版本：mfmcl download --instance {} --game {} --mod {project}@<版本>",
            versions.len(),
            instance.name,
            instance.game_version
        ))
    );
    Ok(())
}

/// Search Modrinth, optionally scoped to an instance.
///
/// Scoping matters: an unscoped search returns mods for every loader and version,
/// and picking one of those is how a user installs a Forge jar into a Fabric
/// instance.
async fn search(
    session: &Session<'_>,
    palette: &Palette,
    json: bool,
    query: Option<&str>,
    target: Option<&str>,
    project_type: &str,
    limit: usize,
) -> Outcome {
    let client = launcher_core::catalog::catalog_client().map_err(Failure::message)?;
    let request = launcher_core::catalog::ModQuery {
        query: query.unwrap_or_default().to_owned(),
        project_type: project_type.to_owned(),
        ..Default::default()
    };
    let results = match target {
        Some(target) => {
            let instance = resolve_instance(&session.store, target)?;
            launcher_core::catalog::search_mods(&client, &instance, &request)
                .await
                .map_err(Failure::message)?
        }
        None => launcher_core::catalog::search_mods_unscoped(&client, &request)
            .await
            .map_err(Failure::message)?,
    };
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&results).unwrap_or_default()
        );
        return Ok(());
    }
    if results.is_empty() {
        println!("没有匹配的结果。");
        return Ok(());
    }
    let mut table = Table::new(&["项目", "标题", "作者", "下载量"]).numeric(&[3]);
    for item in results.iter().take(limit) {
        table.row(vec![
            item.project_id.clone(),
            truncate(&item.title, 32),
            truncate(&item.author, 18),
            item.downloads.to_string(),
        ]);
    }
    print!("{}", table.render(palette, width()));
    println!();
    println!(
        "{}",
        palette.dim("安装：mfmcl download --mod <项目>，或用 list mod-versions 查看版本")
    );
    Ok(())
}

async fn versions(
    _session: &Session<'_>,
    palette: &Palette,
    json: bool,
    limit: usize,
    snapshots: bool,
) -> Outcome {
    // The manifest is the trust root, so it is always fetched from the official
    // source regardless of the user's download mirror preference.
    let client = reqwest::Client::builder()
        .user_agent(concat!("MFMCL-cli/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|error| Failure::message(format!("无法创建网络客户端：{error}")))?;
    let manifest = fetch_manifest(&client, SourcePreference::Official)
        .await
        .map_err(|error| Failure::message(format!("无法获取版本清单：{error}")))?;

    let selected: Vec<&mc_metadata::VersionSummary> = manifest
        .versions
        .iter()
        .filter(|version| snapshots || version.version_type == "release")
        .take(limit)
        .collect();

    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&manifest).unwrap_or_default()
        );
        return Ok(());
    }
    let mut table = Table::new(&["版本", "类型", "发布时间"]);
    for version in &selected {
        let kind = if version.version_type == "release" {
            palette.green("正式版")
        } else if version.version_type == "snapshot" {
            palette.yellow("快照")
        } else {
            palette.dim(&version.version_type)
        };
        table.row(vec![
            version.id.clone(),
            kind,
            timestamp(&version.release_time),
        ]);
    }
    print!("{}", table.render(palette, terminal_width()));
    println!();
    println!(
        "{}",
        palette.dim(&format!(
            "最新正式版：{}　最新快照：{}",
            manifest.latest.release, manifest.latest.snapshot
        ))
    );
    Ok(())
}

async fn loaders(
    session: &Session<'_>,
    palette: &Palette,
    json: bool,
    loader: &str,
    game: &str,
    limit: usize,
) -> Outcome {
    let kind = launcher_core::loaders::LoaderKind::from_name(&loader.to_ascii_lowercase())
        .ok_or_else(|| {
            Failure::message(format!(
                "未识别的加载器 {loader}；可用：fabric、quilt、forge、neoforge。"
            ))
        })?;
    let download = session.store.load_settings()?.download;
    let versions = launcher_core::loaders::list_loader_versions_cached(
        kind,
        game,
        &download,
        Some(session.context.data_root()),
    )
    .await
    .map_err(Failure::from)?;
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&versions).unwrap_or_default()
        );
        return Ok(());
    }
    let mut table = Table::new(&["版本", "稳定"]);
    for version in versions.iter().take(limit) {
        table.row(vec![
            version.version.clone(),
            if version.stable {
                palette.green("是")
            } else {
                palette.dim("否")
            },
        ]);
    }
    print!("{}", table.render(palette, terminal_width()));
    println!();
    println!(
        "{}",
        palette.dim(&format!(
            "Minecraft {game} 共 {} 个 {} 版本",
            versions.len(),
            kind.name()
        ))
    );
    Ok(())
}

/// Sum of file sizes below a directory, bounded so listing stays fast.
///
/// A deep `du` on a 5 GB instance is slow enough to be annoying, and the number is
/// only a sanity hint. Stopping after a few thousand entries keeps `mfmcl list` fast
/// while still being honest about what it measured.
fn directory_size(root: &std::path::Path) -> u64 {
    const MAX_ENTRIES: usize = 5000;
    let mut total = 0_u64;
    let mut seen = 0_usize;
    let mut stack = vec![root.to_path_buf()];
    while let Some(directory) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&directory) else {
            continue;
        };
        for entry in entries.flatten() {
            seen += 1;
            if seen > MAX_ENTRIES {
                return total;
            }
            let Ok(metadata) = entry.metadata() else {
                continue;
            };
            if metadata.is_dir() {
                stack.push(entry.path());
            } else {
                total += metadata.len();
            }
        }
    }
    total
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn directory_size_sums_files_recursively() {
        let root = std::env::temp_dir().join(format!("mfmcl-size-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(root.join("nested")).unwrap();
        std::fs::write(root.join("a.bin"), vec![0_u8; 100]).unwrap();
        std::fs::write(root.join("nested/b.bin"), vec![0_u8; 50]).unwrap();
        assert_eq!(directory_size(&root), 150);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn directory_size_of_a_missing_path_is_zero() {
        let missing = std::env::temp_dir().join("mfmcl-definitely-missing-dir");
        assert_eq!(directory_size(&missing), 0);
    }
}
