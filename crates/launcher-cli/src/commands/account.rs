//! `mfmcl account` — offline profiles and account removal.
//!
//! ## What is deliberately absent
//!
//! Interactive Microsoft sign-in is not here. It needs a browser, a loopback
//! callback page, and a dialog to pick a Minecraft profile from the account's
//! entitlements — none of which a command line can present well. The protocol lives
//! in `launcher-auth`; a future `mfmcl account login microsoft` would drive it with
//! a printed URL, and is not implemented yet.
//!
//! Adding an offline profile *is* here, because it is a single validated string and
//! is what makes the CLI usable without any login at all.

use crate::cli::{AccountAction, AccountArgs};
use crate::commands::download::terminal_width;
use crate::commands::{Failure, Outcome, Session};
use crate::output::{Palette, Table};

pub async fn run(session: &Session<'_>, args: &AccountArgs) -> Outcome {
    let palette = Palette::new(session.sink.palette().on);
    match &args.action {
        AccountAction::List => list(session, &palette),
        AccountAction::AddOffline { name } => add_offline(session, &palette, name),
        AccountAction::Select { account } => select(session, &palette, account),
        AccountAction::Remove { account } => remove(session, &palette, account).await,
    }
}

fn list(session: &Session<'_>, palette: &Palette) -> Outcome {
    let accounts = session.store.list_accounts()?;
    if session.cli.json {
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
            account.display_name.clone(),
            kind.to_owned(),
            account.id.clone(),
        ]);
    }
    print!("{}", table.render(palette, terminal_width()));
    Ok(())
}

fn add_offline(session: &Session<'_>, palette: &Palette, name: &str) -> Outcome {
    let account = launcher_runtime::account::offline_profile(name).map_err(Failure::from)?;
    /*
     * Offline profiles carry no secret, so there is nothing to store in the
     * keyring; the SQLite row is the whole account. `add_account` marks it selected.
     */
    session.store.add_account(&account)?;
    if session.cli.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&account).unwrap_or_default()
        );
        return Ok(());
    }
    println!(
        "{} 已添加离线档案「{}」",
        palette.green("完成"),
        account.display_name
    );
    println!(
        "{}",
        palette.dim("离线档案不验证正版所有权；启动时需要确认。")
    );
    Ok(())
}

fn select(session: &Session<'_>, palette: &Palette, target: &str) -> Outcome {
    let account = crate::commands::resolve_account(&session.store, Some(target))?;
    session.store.select_account(&account.id)?;
    if session.cli.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "id": account.id,
                "name": account.display_name,
            }))
            .unwrap_or_default()
        );
        return Ok(());
    }
    println!(
        "{} 当前账户：{}",
        palette.green("已切换"),
        account.display_name
    );
    Ok(())
}

async fn remove(session: &Session<'_>, palette: &Palette, target: &str) -> Outcome {
    let account = crate::commands::resolve_account(&session.store, Some(target))?;
    if !session.confirm(&format!(
        "删除账户「{}」？删除后需要重新登录才能恢复。",
        account.display_name
    )) {
        return Err(Failure::Declined("已取消删除。".to_owned()));
    }
    /*
     * Clear the keyring entry *before* removing the database row. If the keyring
     * fails, the account record still exists and the user can retry; the other order
     * would leave an unreachable token in the OS credential store with nothing
     * pointing at it.
     */
    if let Some(reference) = account.credential_ref.clone() {
        launcher_runtime::account::delete_credential(reference)
            .await
            .map_err(|error| {
                Failure::message(format!(
                    "账户记录未删除：无法清除系统凭据库中的令牌（{error}）。"
                ))
            })?;
    }
    session.store.remove_account(&account.id)?;
    if session.cli.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({ "removed": account.id }))
                .unwrap_or_default()
        );
        return Ok(());
    }
    println!(
        "{} 已删除账户「{}」",
        palette.green("完成"),
        account.display_name
    );
    Ok(())
}
