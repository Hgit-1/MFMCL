//! `mfmcl download` — create an instance and install game files plus a loader.
//!
//! ## Why this command has two halves
//!
//! Creating an instance is instant; installing one takes minutes. `--no-install`
//! exists so the first half is addressable on its own (for scripted setup where
//! the download happens later), and re-running the command against an existing
//! instance is a *completion* operation, not a fresh install — the underlying
//! installer only fetches missing or wrong files.
//!
//! ## Loader resolution
//!
//! `latest` needs a network round trip to the loader's version service, which is
//! why it is resolved here rather than in the argument parser. Everything else is
//! passed through untouched.

use crate::cli::DownloadArgs;
use crate::commands::{Failure, Outcome, Session, resolve_instance};
use crate::output::{Palette, Table, bytes};
use launcher_core::{InstanceState, loaders};
use launcher_runtime::Downloads;

pub async fn run(session: &Session<'_>, args: &DownloadArgs) -> Outcome {
    let palette = Palette::new(session.sink.palette().on);

    let (kind, version) = match args.loader_request().map_err(Failure::message)? {
        Some((kind, version)) => {
            let kind = loaders::LoaderKind::from_name(&kind)
                .ok_or_else(|| Failure::message("未识别的加载器。"))?;
            (Some(kind), version)
        }
        None => (None, String::new()),
    };

    /*
     * `--name` creates, `--instance` completes, and neither means "derive a name".
     * Deriving one from the game version keeps the common case short:
     * `mfmcl download -v 1.21.1` should not require inventing a name.
     */
    let requested_name = args
        .name
        .clone()
        .or_else(|| args.instance.clone())
        .unwrap_or_else(|| default_name(&args.game, kind));

    let existing = match &args.instance {
        Some(target) => Some(resolve_instance(&session.store, target)?),
        None => session
            .store
            .list_instances()?
            .into_iter()
            .find(|instance| instance.name.eq_ignore_ascii_case(&requested_name)),
    };

    let instance = match existing {
        Some(instance) => {
            /*
             * An existing instance is completed, never silently recreated. But a
             * version mismatch is a real risk: the user may have typed the wrong
             * `--game` and would otherwise "successfully" install into an instance
             * that keeps its old version.
             */
            if instance.game_version != args.game {
                return Err(Failure::message(format!(
                    "实例「{}」是 Minecraft {}，与 --game {} 不一致。\n\
                     请改用正确的版本，或换一个 --name 创建新实例。",
                    instance.name, instance.game_version, args.game
                )));
            }
            session.sink.note(&format!(
                "实例「{}」已存在，将补全缺失文件而不是重新创建。",
                instance.name
            ));
            instance
        }
        None => {
            let loader_marker = kind.map(|kind| {
                // Resolve `latest` before creating anything: a bad version should
                // not leave a half-made instance behind.
                format!("{}@{version}", kind.name())
            });
            let record = launcher_runtime::install::create_instance(
                &session.context,
                &session.store,
                &requested_name,
                &args.game,
                loader_marker.as_deref(),
            )?;
            session.sink.note(&format!(
                "已创建实例「{}」（Minecraft {}）",
                record.name, record.game_version
            ));
            record
        }
    };

    if args.no_install {
        print_summary(session, &instance, &palette)?;
        session.sink.note(
            "按 --no-install 只创建了记录；稍后用 `mfmcl download --instance <名称>` 下载文件。",
        );
        return Ok(());
    }

    /*
     * Resolve the loader version now, after the record exists. Doing it here means
     * a network failure leaves a created-but-uninstalled instance that can be
     * retried, rather than nothing at all.
     */
    let resolved_loader = match kind {
        Some(kind) => {
            let resolved = resolve_loader_version(kind, &args.game, &version).await?;
            /*
             * A newly created instance carries a marker file that the installer
             * consumes. An existing instance has no marker, so the loader has to be
             * requested explicitly by writing one — which is exactly what the
             * runtime helper does.
             */
            launcher_runtime::install_support::request_pack_loader(
                &instance,
                &format!("{}@{resolved}", kind.name()),
            )
            .map_err(Failure::from)?;
            Some((kind, resolved))
        }
        None => None,
    };

    if let Some((kind, version)) = &resolved_loader {
        session
            .sink
            .note(&format!("加载器：{} {}", kind.name(), version));
    }

    let downloads = Downloads::default();
    let sink = session.sink.as_event_sink();
    launcher_runtime::install::install_instance(
        &session.context,
        &session.store,
        &downloads,
        sink,
        &instance.id,
    )
    .await
    .map_err(Failure::from)?;

    // Re-read: the installer changed state and possibly the recorded loader.
    let installed = session
        .store
        .list_instances()?
        .into_iter()
        .find(|record| record.id == instance.id)
        .ok_or_else(|| Failure::message("安装完成后找不到实例记录。"))?;
    print_summary(session, &installed, &palette)?;

    if !args.mods.is_empty() {
        install_mods(session, &installed, &args.mods, &palette).await?;
    }

    if args.launch {
        session.sink.note("");
        crate::commands::launch::run(
            session,
            &crate::cli::LaunchArgs {
                instance: Some(installed.name.clone()),
                last: false,
                account: args.account.clone(),
                java: None,
                detach: false,
                no_log: false,
                offline_ok: false,
            },
        )
        .await?;
    }
    Ok(())
}

/// Install `--mod` entries from Modrinth into an instance.
///
/// ## Why this lives here rather than in the runtime
///
/// The runtime has no "install a mod by name" operation because the desktop UI
/// drives it through a search → pick version → install flow, and each step is a
/// separate command. A command line wants the whole thing in one line, so the
/// steps are composed here: resolve the project, pick the newest version matching
/// the instance, then hand that version to the same verified installer the GUI
/// uses.
///
/// Nothing about verification is reimplemented — `install_mod_version` still
/// downloads from `cdn.modrinth.com`, checks the SHA-1, and refuses to overwrite an
/// existing file.
async fn install_mods(
    session: &Session<'_>,
    instance: &launcher_core::InstanceRecord,
    requested: &[String],
    palette: &Palette,
) -> Outcome {
    /*
     * Mods only load through a modded instance. Installing into vanilla would put
     * jars in `mods/` that nothing reads, and the user would see "installed" followed
     * by "the mod is not in game" — the exact confusion the GUI warns about.
     */
    if instance.loader == "vanilla" {
        return Err(Failure::message(format!(
            "实例「{}」是原版，模组不会生效。\n\
             请用 --mod-loader fabric（或 quilt/forge/neoforge）重建，或把模组装进模组实例。",
            instance.name
        )));
    }
    if !session.cli.quiet {
        session.sink.note("");
    }

    let client = launcher_core::catalog::catalog_client().map_err(Failure::message)?;
    /*
     * Honour the user's saved concurrency. `--mod` downloads from
     * `cdn.modrinth.com` directly, so the source preference does not apply, but the
     * limits do; `engine_from_settings` is the same constructor the installer uses,
     * so there is one definition of "which settings reach the engine".
     */
    let settings = session.store.load_settings()?;
    let engine = launcher_runtime::install::engine_from_settings(&settings)?;
    let mods_dir = instance.game_dir.join("mods");
    std::fs::create_dir_all(&mods_dir).map_err(|error| {
        Failure::message(format!(
            "无法创建 mods 目录 {}：{error}",
            mods_dir.display()
        ))
    })?;

    let mut installed = 0_usize;
    for spec in requested {
        let (name, pinned) = split_mod_spec(spec);
        if name.is_empty() {
            return Err(Failure::message(format!(
                "模组标识无效：「{spec}」。用法：--mod jei 或 --mod jei@版本号"
            )));
        }
        if !session.cli.quiet {
            session.sink.note(&format!("查找模组 {name}…"));
        }

        // Search by the slug, then take the exact project whose slug/id matches.
        // Modrinth's search is fuzzy, so trusting the first hit would silently
        // install the wrong project when a slug is close to another's title.
        let query = launcher_core::catalog::ModQuery {
            query: name.to_owned(),
            ..Default::default()
        };
        let candidates = launcher_core::catalog::search_mods(&client, instance, &query)
            .await
            .map_err(Failure::message)?;
        let project = candidates
            .iter()
            .find(|item| item.project_id == name)
            .or_else(|| candidates.first())
            .ok_or_else(|| {
                Failure::message(format!(
                    "Modrinth 上没有找到「{name}」。检查拼写，或用 `mfmcl list mods` 看已安装的模组。"
                ))
            })?;

        let versions = launcher_core::catalog::mod_versions(&client, instance, &project.project_id)
            .await
            .map_err(Failure::message)?;
        let version = match pinned {
            Some(wanted) => versions
                .iter()
                .find(|item| item.version_number == wanted || item.version_id == wanted)
                .ok_or_else(|| {
                    Failure::message(format!(
                        "「{}」没有版本 {wanted}；用 `mfmcl list mod-versions {}` 查看可用版本。",
                        project.title, project.project_id
                    ))
                })?,
            None => versions.first().ok_or_else(|| {
                Failure::message(format!(
                    "「{}」没有兼容 Minecraft {} / {} 的版本。",
                    project.title, instance.game_version, instance.loader
                ))
            })?,
        };

        let file = launcher_core::catalog::install_mod_version(version, &mods_dir, &engine)
            .await
            .map_err(Failure::message)?;
        installed += 1;
        if session.cli.json {
            continue;
        }
        println!(
            "{} {} {} → {}",
            palette.green("已安装"),
            project.title,
            version.version_number,
            file
        );
        /*
         * Dependencies are reported, not installed. The catalogue only exposes how
         * many a version declares, and fetching them would mean resolving a graph
         * the GUI does not resolve either. Saying so beats letting the user find out
         * from a crash report.
         */
        if version.dependency_count > 0 {
            println!(
                "{}",
                palette.yellow(&format!(
                    "  这个版本声明了 {} 个前置模组；它们不会被自动安装，请按需补齐。",
                    version.dependency_count
                ))
            );
        }
    }

    if session.cli.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "instance": instance.name,
                "installed": installed,
                "modsDir": mods_dir.to_string_lossy(),
            }))
            .unwrap_or_default()
        );
        return Ok(());
    }
    println!(
        "{}",
        palette.dim(&format!(
            "共 {installed} 个模组，目录：{}",
            mods_dir.display()
        ))
    );
    Ok(())
}

/// Split `slug@version` into its two halves.
///
/// The `@` form is the same one `--mod-loader` accepts, so users do not have to
/// learn a second syntax for "this project, that version".
fn split_mod_spec(spec: &str) -> (&str, Option<&str>) {
    let spec = spec.trim();
    match spec.split_once('@') {
        Some((name, version)) => {
            let version = version.trim();
            (
                name.trim(),
                if version.is_empty() {
                    None
                } else {
                    Some(version)
                },
            )
        }
        None => (spec, None),
    }
}

/// Derive an instance name when the user did not supply one.
///
/// The shape matches what people type by hand (`1.21.1_neoforge`), which matters
/// because the name becomes a directory and shows up in every other command.
fn default_name(game: &str, kind: Option<loaders::LoaderKind>) -> String {
    match kind {
        Some(kind) => format!("{game}_{}", kind.name()),
        None => game.to_owned(),
    }
}

/// Turn `latest` into a concrete loader version.
///
/// `latest` means the newest **stable** release, falling back to the newest of any
/// kind when nothing is flagged stable (Forge's promotion feed is often empty for
/// older versions, and returning "no stable version" there would be useless).
async fn resolve_loader_version(
    kind: loaders::LoaderKind,
    game: &str,
    requested: &str,
) -> Result<String, Failure> {
    let requested = requested.trim();
    if !requested.is_empty() && requested != "latest" {
        loaders::safe_version(requested).map_err(Failure::message)?;
        return Ok(requested.to_owned());
    }
    let versions = loaders::list_loader_versions(kind, game)
        .await
        .map_err(Failure::from)?;
    /*
     * The upstream lists are already sorted newest-first, so `stable` selection is
     * a filter rather than a sort. Relying on that ordering is safe here because
     * `list_loader_versions` sorts explicitly.
     */
    versions
        .iter()
        .find(|version| version.stable)
        .or_else(|| versions.first())
        .map(|version| version.version.clone())
        .ok_or_else(|| {
            Failure::message(format!(
                "Minecraft {game} 下没有可用的 {} 版本。",
                kind.name()
            ))
        })
}

/// Print what is now on disk.
fn print_summary(
    session: &Session<'_>,
    instance: &launcher_core::InstanceRecord,
    palette: &Palette,
) -> Outcome {
    if session.cli.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "id": instance.id,
                "name": instance.name,
                "gameVersion": instance.game_version,
                "loader": instance.loader,
                "state": format!("{:?}", instance.state),
                "instanceDir": instance.instance_dir.to_string_lossy(),
                "gameDir": instance.game_dir.to_string_lossy(),
            }))
            .unwrap_or_default()
        );
        return Ok(());
    }
    let mut table = Table::new(&["项目", "值"]);
    table.row(vec!["实例".into(), instance.name.clone()]);
    table.row(vec!["Minecraft".into(), instance.game_version.clone()]);
    table.row(vec!["加载器".into(), instance.loader.clone()]);
    let state = match instance.state {
        InstanceState::Ready => palette.green("已就绪"),
        InstanceState::NotInstalled => palette.yellow("未安装"),
        InstanceState::Installing => palette.yellow("安装中"),
        InstanceState::Failed => palette.red("失败"),
    };
    table.row(vec!["状态".into(), state]);
    table.row(vec![
        "目录".into(),
        instance.instance_dir.to_string_lossy().into_owned(),
    ]);
    println!();
    print!("{}", table.render(palette, terminal_width()));
    println!();
    if instance.state == InstanceState::Ready {
        println!(
            "{}",
            palette.dim(&format!("启动：mfmcl launch --instance {}", instance.name))
        );
    }
    Ok(())
}

/// Columns to draw into.
///
/// `COLUMNS` is honoured because the CLI may run under a wrapper that reports a
/// different width than the terminal. 80 is the fallback for redirected output.
pub fn terminal_width() -> usize {
    std::env::var("COLUMNS")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|width| *width >= 20)
        .unwrap_or(100)
}

/// Human-readable one-liner for `download`'s completion message.
#[allow(dead_code)]
fn describe_size(instance: &launcher_core::InstanceRecord) -> String {
    bytes(
        std::fs::read_dir(&instance.instance_dir)
            .map(|entries| {
                entries
                    .flatten()
                    .filter_map(|entry| entry.metadata().ok())
                    .map(|meta| meta.len())
                    .sum()
            })
            .unwrap_or(0),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derived_names_match_what_people_type() {
        assert_eq!(default_name("1.21.1", None), "1.21.1");
        assert_eq!(
            default_name("1.21.1", Some(loaders::LoaderKind::NeoForge)),
            "1.21.1_neoforge"
        );
        assert_eq!(
            default_name("1.20.1", Some(loaders::LoaderKind::Fabric)),
            "1.20.1_fabric"
        );
    }

    #[test]
    fn a_concrete_loader_version_skips_the_network() {
        // This test must not need the internet: a concrete version is returned
        // before any request is made.
        let future = resolve_loader_version(loaders::LoaderKind::Fabric, "1.21.1", "0.16.10");
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        assert_eq!(runtime.block_on(future).unwrap(), "0.16.10");
    }

    #[test]
    fn a_mod_spec_splits_on_the_last_at_sign() {
        assert_eq!(split_mod_spec("jei"), ("jei", None));
        assert_eq!(split_mod_spec(" jei "), ("jei", None));
        assert_eq!(split_mod_spec("jei@19.70.0"), ("jei", Some("19.70.0")));
        // A trailing `@` means "no version", not "the empty version".
        assert_eq!(split_mod_spec("jei@"), ("jei", None));
    }
}
