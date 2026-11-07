//! `mfmcl remove` — delete an instance record, and optionally its files.
//!
//! ## Why files are opt-in
//!
//! Deleting the record is reversible by re-downloading. Deleting the files is not,
//! and an instance directory holds saves. Defaulting to "record only" means the
//! destructive half always requires a second decision, which is the only thing that
//! makes an irreversible command safe in a terminal.

use crate::cli::RemoveArgs;
use crate::commands::{Failure, Outcome, Session, resolve_instance};
use crate::output::Palette;

pub fn run(session: &Session<'_>, args: &RemoveArgs) -> Outcome {
    let palette = Palette::new(session.sink.palette().on);
    let instance = resolve_instance(&session.store, &args.instance)?;

    let prompt = if args.files {
        format!(
            "删除实例「{}」以及它磁盘上的全部文件（含存档）？此操作无法撤销。",
            instance.name
        )
    } else {
        format!(
            "删除实例「{}」的记录？磁盘文件会保留在 {}。",
            instance.name,
            instance.instance_dir.display()
        )
    };
    if !session.confirm(&prompt) {
        return Err(Failure::Declined("已取消删除。".to_owned()));
    }

    session.store.delete_instance(&instance.id)?;

    /*
     * File removal is restricted to the launcher's instances root. The record's
     * directory could have been edited by hand, and `remove_dir_all` on an arbitrary
     * path is not something a launcher should ever do.
     */
    let mut files_removed = false;
    if args.files {
        let instances_root = session.context.data_root().join("instances");
        let directory = &instance.instance_dir;
        let within_root = directory
            .canonicalize()
            .ok()
            .zip(instances_root.canonicalize().ok())
            .is_some_and(|(target, root)| target.starts_with(&root));
        if !within_root {
            return Err(Failure::message(format!(
                "已删除实例记录，但拒绝删除 {}：它不在启动器的实例目录内。",
                directory.display()
            )));
        }
        std::fs::remove_dir_all(directory).map_err(|error| {
            Failure::message(format!(
                "已删除实例记录，但删除文件失败：{error}\n目录：{}",
                directory.display()
            ))
        })?;
        files_removed = true;
    }

    if session.cli.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "instance": instance.name,
                "filesRemoved": files_removed,
                "directory": instance.instance_dir.to_string_lossy(),
            }))
            .unwrap_or_default()
        );
        return Ok(());
    }
    println!(
        "{} 已删除实例「{}」{}",
        palette.green("完成"),
        instance.name,
        if files_removed {
            "及其文件"
        } else {
            "（文件已保留）"
        }
    );
    Ok(())
}
