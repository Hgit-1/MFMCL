# Local validation record — 2026-10-21

This record distinguishes implemented code, automated checks, and unverified release requirements. It is not a certification of Minecraft compatibility or a signed release.

## Automated checks performed

- Core Rust suites: `mc-metadata`, `download-engine`, `launcher-core` (including backups, mod JAR inspection, update-manifest parsing/version ordering, and account-removal store tests), and `launcher-auth`. Coverage includes source trust, bounded retry and cancellation, validated Range resume, corrupt hash rejection, authentication state/PKCE, Microsoft entitlement checks, Yggdrasil profile selection, launch placeholder expansion, secret redaction, interrupted task recovery, mod toggles, ZIP stream limits and import rollback.
- Frontend: TypeScript/Vite production build, Oxlint, and Playwright browser tests for HTTPS skin-server consent/role selection, Microsoft command wiring and error display, repeated offline confirmation, installation pause/resume/cancel/retry, backup flows, saved-account removal with credential warnings, launcher update check/download/policy, Modrinth mod-update disclosure/warning/verified replacement, client update checks, invalid settings, theme contrast, and narrow-screen navigation. Native APIs in these tests are mocked; no live Modrinth/Mojang request is made.
- Native Windows backend: `cargo check --target x86_64-pc-windows-gnu -p mfmcl-desktop --tests`, using real MinGW tools. This type-checks desktop commands/tests and the Windows credential adapter; it does not link or execute a Windows installer.
- Linux credential adapter: `cargo check -p launcher-auth --features native-credentials`. The OS credential service itself has not been exercised with a real account.
- Development entry point: the documented Tauri npm command now finds the project and starts Vite on `127.0.0.1:1420`. Native Linux compilation remains blocked by missing GTK/WebKit development packages in this container.
- Browser visual inspection: the current dark overview page was inspected in a Chromium viewport. This is not a complete accessibility or all-platform visual audit.
- Opt-in JVM smoke test: `crates/launcher-core/tests/jvm_smoke.rs` launches a real JVM against an original fixture (not Minecraft) when a JDK is present. It is not run in CI.

## Launcher self-update status

The PCL2-inspired update feature is implemented as check/prompt/verified-download: a background startup check when the policy is "notify" (quiet failures), a manual check with changelog in settings, and a SHA-256-verified installer download into a user-chosen folder. The running binary is never replaced in place: without update-signing keys, automatic self-replacement would be unsafe, so the user runs the installer explicitly. Builds must set `MFMCL_UPDATE_MANIFEST` at compile time; without it, checks honestly report that no update source is configured. No production update endpoint exists yet.

## Mod and client update status

Mod update checks use Modrinth's documented hash-lookup and project-version endpoints against locally mocked responses only; no live Modrinth query or real mod replacement has been executed in this environment. Client update checks compare a fingerprint of artifact-bearing metadata (client JAR, asset index, libraries, logging) against the locally stored official profile; both fingerprints and republish detection are unit-tested, but no live Mojang republish scenario has been verified.

## CI configured, not locally proven

`ci.yml` defines frontend tests, Rust suites, and desktop compilation on Windows/macOS/Linux. `desktop-artifacts.yml` is a manual workflow that builds unsigned validation bundles and retains them for seven days. These workflow definitions have not been executed remotely in this session. Signing, notarization, publishing and automatic updater credentials are intentionally absent.

## Outstanding plan requirements

- aria2: `scripts/aria2/` now contains a hash-locked source acquisition/reproducible-build/packaging toolchain for the pinned 1.37.0 release. Actually building per-platform binaries, verifying them against `aria2-manifest.json`, and completing GPL source/notice/distribution review remain outstanding; the manifest still pins nothing and the native downloader remains the fallback.
- Forge/NeoForge installer adapters, explicit mod conflict guidance, and an online content catalogue. Current modded support is Fabric/Quilt, supported Modrinth pack import/export, and local hash-checked backup/restore into fresh instances.
- Real Microsoft login with the project's registered/approved public application ID, system credential persistence/refresh on each OS, trusted live skin-server login and Authlib-Injector launch.
- Real vanilla/modded game launches with recorded Java/game/loader combinations, plus runtime process-stop and crash-summary verification.
- Native install/upgrade/uninstall, signing/notarization, disk-full and migration failure drills, physical Windows/macOS/Linux verification, and documented supported architectures.

Mobile execution remains documentation-only in `FUTURE.md`; no Android/iOS/HarmonyOS runtime code has been added.
