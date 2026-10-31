# MFMCL

MFMCL is an original Minecraft Java Edition launcher project for Windows, macOS, and Linux. Its Chinese-first interface focuses on guided setup and separate game instances.

## Implemented in the current development tree

- Official Minecraft version metadata, explicit official/BMCLAPI/mixed artifact sources, size/hash verification, verified fallback, safe partial downloads, bounded concurrency, retry, pause, resume, cancellation, and persistent task recovery.
- Vanilla instance installation and Java runtime detection, launch-argument expansion, memory configuration, live process/log events, diagnostic summaries, and local export.
- Offline profiles, Microsoft public-client browser login with PKCE and Java entitlement validation, and HTTPS Yggdrasil skin-server login with explicit trust confirmation and role selection. Online credentials are stored through the operating system credential provider; no launcher client secret or borrowed Microsoft client ID is included.
- Fabric and Quilt installation, mod enable/disable, static dependency/conflict inspection surfaced in the UI, and bounded Modrinth pack preview/import/export for supported Vanilla/Fabric/Quilt combinations.
- A Modrinth online mod catalogue: search scoped to the instance's game version and loader, per-project version lists, and SHA-1-verified installs that never overwrite existing files.
- Instance deletion with explicit confirmation; file removal is restricted to the launcher's instances root.
- Mod updates against Modrinth (hash-disclosed lookup, SHA-1-verified download, previous JAR kept as `.old`) and Minecraft client update checks that compare the installed official profile against the current Mojang manifest; applying a client update runs the existing verified repair install.
- Four theme presets, custom accent/background settings, settings validation, and saved-account management (list, preselect, silent refresh at launch, per-account removal that also deletes the keyring entry).
- Launcher self-update checks in the PCL2 spirit: a quiet startup check with reminders, a manual check with changelog, and a SHA-256-verified installer download into a user-chosen folder. Self-replacement of the running binary stays disabled until update signing exists; builds without a configured `MFMCL_UPDATE_MANIFEST` report that honestly.
- Hash-checked local game-data backups with preview and restore into a fresh instance.

## Deliberate release gates and limits

This is a development build, not a signed production release. The Linux container does not have the GTK/WebKit development libraries required to run the desktop shell, and no live Minecraft account/game launch or physical Windows/macOS run has been verified here. CI compiles the desktop backend on all three target operating systems; local Windows GNU cross-checks are compile checks, not packaged or executed Windows applications.

The aria2 integration is optional and guarded by a platform-specific SHA-256/version manifest. That manifest currently contains no binary pins, so this source tree falls back to the Rust downloader; it does not ship an aria2 executable. A release must complete the GPL source/notice review before adding any binary. Forge and NeoForge, online mod catalogues, signed installers, and a verified install/upgrade/uninstall cycle remain unimplemented. Fabric/Quilt and supported Modrinth packs are the only modded formats currently handled.

Android, iOS, and HarmonyOS local game execution remain documentation-only in [`docs/FUTURE.md`](docs/FUTURE.md).

## Development

See [`docs/BUILDING.md`](docs/BUILDING.md) for prerequisites, frontend preview, tests, and native desktop builds. See [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) for module and trust boundaries, and [`docs/THIRD_PARTY.md`](docs/THIRD_PARTY.md) for distribution constraints.

UI work starts with a browser preview: run `npm run dev` and open `http://127.0.0.1:5173/`, then adjust the visuals in Design mode. The full convention is in [`UI_WORKFLOW.md`](UI_WORKFLOW.md).

The root Git repository excludes the nested `/PCL/` reference checkout. Reference observations are abstracted in [`docs/REFERENCE_NOTES.md`](docs/REFERENCE_NOTES.md); no reference implementation or assets are copied.

## Accounts and game content

MFMCL does not bundle or host Minecraft game files. Users must use an account authorized for the content they install. An offline profile does not verify Microsoft ownership and is clearly identified; every attempt to launch with it requires a fresh confirmation. It does not bypass Microsoft entitlement checks or online-mode server authentication. Skin-server credentials are submitted only after explicit confirmation of the HTTPS authentication root.
