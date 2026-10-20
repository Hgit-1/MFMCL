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

## Sources

- HMCL: https://github.com/HMCL-dev/HMCL
- Zalith Launcher docs: https://docs.zalithlauncher.cn/en
- Fold Craft Launcher: https://github.com/FCL-Team/FoldCraftLauncher
- MobileGlues: https://github.com/MobileGL-Dev/MobileGlues-release
- aria2 manual: https://aria2.github.io/manual/en/html/aria2c.html
- BMCLAPI docs: https://bmclapidoc.bangbang93.com/
- Microsoft identity authorization code flow: https://learn.microsoft.com/en-us/entra/identity-platform/v2-oauth2-auth-code-flow
- Apple App Review Guidelines: https://developer.apple.com/app-store/review/guidelines/
