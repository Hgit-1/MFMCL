# Reference review notes

These notes capture product and configuration patterns to investigate, not code to copy. New implementation decisions must be based on documented requirements, official protocols, security review, and original implementation.

## Configuration and instance boundaries

- Model global preferences, per-instance overrides, and actual game-data directories as separate concepts.
- Preserve an explicit `inherit/automatic` setting separately from a user override; do not freeze an automatically inferred value as if the user chose it.
- Treat changing an instance's isolated/shared data directory as a data migration decision. Explain which saves/mods will be visible, back up first, and never move/delete files silently.
- Parse imported settings into a temporary validated model, show a preview, then commit the import atomically. Add a schema version and migration history.
- Keep secrets out of settings exports. Store long-lived credentials in the operating system credential store rather than custom reversible encryption.

## Downloads, startup, and diagnostics

- Represent official source and mirrors explicitly per artifact; preserve the selected source in task history.
- Verify reused files and completed downloads. Write to a temporary path, verify, then atomically publish to the cache/install directory.
- Separate download/install progress from JVM startup and game-ready detection. If the game provides no measurable percentage, show a phase or indeterminate progress rather than a fabricated 100%.
- Display a single-line live game log while retaining a full local log. Redact credentials before logging, not only when exporting.
- Treat crash analysis as heuristic: report confidence and evidence, keep the original logs, allow export to a user-selected directory, and never upload diagnostics without explicit consent.

## Product references reviewed

- HMCL's public repository describes broad cross-platform, mod-loader, modpack, and UI customization features. Use this as a checklist for capability categories, not an implementation template.
- Zalith Launcher and Fold Craft Launcher document Android-specific Java Edition flows; use them only to identify device, renderer, control, and compatibility questions for `FUTURE.md`.
- MobileGlues documents its OpenGL ES requirements and publishes a device/mod compatibility matrix; use that pattern for transparent support reporting.
- A limited read-only review of the local `PCL/` reference focused on settings separation, download fallback, progress, and diagnostics. Several implementations are legacy-specific; no source is imported or ported.

## PCL2 login and update interaction review (read-only, 2026-10-21)
A second limited read-only pass over the local `PCL/` checkout examined only the public login pages and the settings/about pages, plus the mod-update wording in `PageInstanceMod.xaml.vb`. The open-source build ships its launcher self-update implementation as stubs (`ModSecret.vb` shows `UpdateCheckByButton` returning "该版本中不包含更新功能……"), so nothing update-related could be taken even as reference; only the visible interaction wording and settings layout were noted. No code, assets, strings, or branding are reused; the principles below are re-implemented from scratch against our own architecture.

Login principles abstracted from PCL2's public pages:

- The Microsoft login page is account-centric, not flow-centric: a dropdown lists every cached account with per-account delete buttons plus a fixed "添加新账号" entry, and the previously used account is preselected.
- Silent re-login is the default: stored refresh tokens are exchanged without a browser; the system browser is only for adding a new account. The login button itself doubles as the progress indicator (percentage text) instead of a separate progress bar.
- Errors are classified into actionable Chinese hints (network vs SSL vs user-cancel vs unknown) instead of showing raw provider errors.
- All account kinds (offline/Microsoft/skin server) are reachable from one launch screen with the current kind always visible, so switching identity never hides what type is active.

Update principles abstracted from PCL2's settings/about wording (implementation absent from the public build):

- The user picks an update policy in settings (never check / notify only / auto-download), separate per channel (stable vs snapshot).
- Checks run in the background at startup; the about page also offers a manual "check update" button.
- When an update has been downloaded, it is applied on next restart rather than replacing a running binary.

Mod-update principles abstracted from PCL2's public mod page wording:

- Before replacing anything, the launcher warns that new mod versions may break saves or other mods, advises against self-updating inside modpacks, asks the user to back up saves and read changelogs, and promises the previous file is recoverable.
- The update flow shows a "可更新 (N)" style summary, updates files one by one, keeps the old file until the new one is in place, and reports how many mods were updated.
- Client file updates re-check against official metadata; the game data itself is never silently mutated while unrecoverable.

For MFMCL these translate to: manage saved accounts (list, preselect, delete, silent refresh — refresh already happens at launch); an update-check framework with an explicit user-chosen policy, manual check entry, changelog display, and hash-verified download to a user-chosen folder; and a Modrinth mod-update check that discloses hash sharing, confirms the risk in the same spirit (backup saves, modpack caution), verifies SHA-1 from Modrinth metadata, and keeps the previous JAR as a `.old` file. Without code signing and update-signing keys, in-place self-replacement must stay disabled; downloads are verified against a pinned SHA-256 in our own manifest and the user runs the installer explicitly.

## Sources

- HMCL: https://github.com/HMCL-dev/HMCL
- Zalith Launcher docs: https://docs.zalithlauncher.cn/en
- Fold Craft Launcher: https://github.com/FCL-Team/FoldCraftLauncher
- MobileGlues: https://github.com/MobileGL-Dev/MobileGlues-release
- aria2 manual: https://aria2.github.io/manual/en/html/aria2c.html
- BMCLAPI docs: https://bmclapidoc.bangbang93.com/
- Microsoft identity authorization code flow: https://learn.microsoft.com/en-us/entra/identity-platform/v2-oauth2-auth-code-flow
- Apple App Review Guidelines: https://developer.apple.com/app-store/review/guidelines/
