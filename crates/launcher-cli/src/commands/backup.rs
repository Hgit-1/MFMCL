//! `mfmcl backup` — create, inspect, and restore instance backups.
//!
//! ## Restore never overwrites
//!
//! Restoring always creates a **new** instance. Overwriting in place would mean a
//! failed restore destroys the save the user was trying to protect, and there is no
//! undo for that. The runtime enforces it; this module just explains it.
//!
//! ## The hash is the confirmation
//!
//! `preview` reports a SHA-256; `restore` requires it back. That binds the bytes
//! extracted to the bytes the user looked at, so a backup swapped between preview
//! and restore is refused instead of silently installed.

use crate::cli::{BackupAction, BackupArgs};
use crate::commands::download::terminal_width;
use crate::commands::{Failure, Outcome, Session, resolve_instance};
use crate::output::{Palette, Table, bytes, timestamp};

pub async fn run(session: &Session<'_>, args: &BackupArgs) -> Outcome {
    let palette = Palette::new(session.sink.palette().on);
    match &args.action {
        BackupAction::Create { instance, to } => create(session, &palette, instance, to).await,
        BackupAction::Preview { path } => preview(session, &palette, path).await,
        BackupAction::Restore { path, name } => {
            restore(session, &palette, path, name.as_deref()).await
        }
    }
}

async fn create(session: &Session<'_>, palette: &Palette, instance: &str, to: &str) -> Outcome {
    let record = resolve_instance(&session.store, instance)?;
    let directory = std::path::Path::new(to);
    if !directory.is_absolute() {
        return Err(Failure::message(format!(
            "备份目录必须是绝对路径（收到 {to}）。"
        )));
    }
    if !directory.is_dir() {
        return Err(Failure::message(format!(
            "备份目录不存在：{}\n请先创建它。",
            directory.display()
        )));
    }
    if !session.cli.quiet {
        session.sink.note(&format!(
            "正在备份「{}」到 {}…",
            record.name,
            directory.display()
        ));
    }
    let archive = launcher_runtime::backup::create(&session.store, &record.id, to)
        .await
        .map_err(Failure::from)?;
    if session.cli.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "archive": archive.to_string_lossy(),
                "instance": record.name,
            }))
            .unwrap_or_default()
        );
        return Ok(());
    }
    println!("{} 已备份到 {}", palette.green("完成"), archive.display());
    Ok(())
}

async fn preview(session: &Session<'_>, palette: &Palette, path: &str) -> Outcome {
    let preview = launcher_runtime::backup::preview(path)
        .await
        .map_err(Failure::from)?;
    if session.cli.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "name": preview.name,
                "minecraft": preview.minecraft,
                "loader": preview.loader,
                "createdAt": preview.created_at,
                "files": preview.file_count,
                "bytes": preview.total_bytes,
                "sha256": preview.archive_sha256,
            }))
            .unwrap_or_default()
        );
        return Ok(());
    }
    let mut table = Table::new(&["项目", "值"]);
    table.row(vec!["名称".into(), preview.name.clone()]);
    table.row(vec!["Minecraft".into(), preview.minecraft.clone()]);
    table.row(vec!["加载器".into(), preview.loader.clone()]);
    table.row(vec!["创建时间".into(), timestamp(&preview.created_at)]);
    table.row(vec!["文件数".into(), preview.file_count.to_string()]);
    table.row(vec!["总大小".into(), bytes(preview.total_bytes)]);
    print!("{}", table.render(palette, terminal_width()));
    println!();
    println!("{}", palette.dim("恢复："));
    println!(
        "{}",
        palette.dim(&format!(
            "  mfmcl backup restore \"{path}\" --name <新实例名>"
        ))
    );
    Ok(())
}

async fn restore(
    session: &Session<'_>,
    palette: &Palette,
    path: &str,
    requested_name: Option<&str>,
) -> Outcome {
    let preview = launcher_runtime::backup::preview(path)
        .await
        .map_err(Failure::from)?;

    /*
     * The name defaults to the backup's own name with a suffix, because restoring
     * twice must not collide and an instance name is a directory. The version is
     * fixed by the archive: restoring a 1.21.1 backup creates a 1.21.1 instance.
     */
    let name = match requested_name {
        Some(name) => name.to_owned(),
        None => {
            let stamp = chrono::Local::now().format("%m%d-%H%M");
            format!("{}-{stamp}", preview.name)
        }
    };

    if !session.cli.quiet {
        let mut table = Table::new(&["项目", "值"]);
        table.row(vec!["将创建实例".into(), name.clone()]);
        table.row(vec!["Minecraft".into(), preview.minecraft.clone()]);
        table.row(vec!["加载器".into(), preview.loader.clone()]);
        table.row(vec!["文件数".into(), preview.file_count.to_string()]);
        table.row(vec!["总大小".into(), bytes(preview.total_bytes)]);
        session.sink.note(&table.render(palette, terminal_width()));
    }
    if !session.confirm(&format!(
        "把备份恢复为新实例「{name}」？已有实例不会被修改。"
    )) {
        return Err(Failure::Declined("已取消恢复。".to_owned()));
    }

    let record = launcher_runtime::backup::restore(
        &session.context,
        &session.store,
        path,
        &preview.archive_sha256,
        &name,
    )
    .await
    .map_err(Failure::from)?;

    if session.cli.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&record).unwrap_or_default()
        );
        return Ok(());
    }
    println!(
        "{} 已恢复为实例「{}」（Minecraft {}）",
        palette.green("完成"),
        record.name,
        record.game_version
    );
    println!(
        "{}",
        palette.dim(&format!("启动：mfmcl launch --instance {}", record.name))
    );
    Ok(())
}
