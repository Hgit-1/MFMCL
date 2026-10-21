# Local validation record — 2026-10-21

This record distinguishes implemented code, automated checks, and unverified release requirements. It is not a certification of Minecraft compatibility or a signed release.

## Automated checks performed

- Core Rust suites: `mc-metadata`, `download-engine`, `launcher-core`, and `launcher-auth`. Coverage includes source trust, bounded retry and cancellation, validated Range resume, corrupt hash rejection, authentication state/PKCE, Microsoft entitlement checks, Yggdrasil profile selection, launch placeholder expansion, secret redaction, interrupted task recovery, mod toggles, ZIP stream limits and import rollback.
- Frontend: TypeScript/Vite production build, Oxlint, and Playwright browser tests for HTTPS skin-server consent/role selection, Microsoft command wiring and error display, repeated offline confirmation, installation pause/resume/cancel/retry, invalid settings, theme contrast, and narrow-screen navigation. Native APIs in these tests are mocked.
- Native Windows backend: `cargo check --target x86_64-pc-windows-gnu -p mfmcl-desktop --tests`, using real MinGW tools. This type-checks desktop commands/tests and the Windows credential adapter; it does not link or execute a Windows installer.
- Linux credential adapter: `cargo check -p launcher-auth --features native-credentials`. The OS credential service itself has not been exercised with a real account.
- Development entry point: the documented Tauri npm command now finds the project and starts Vite on `127.0.0.1:1420`. Native Linux compilation remains blocked by missing GTK/WebKit development packages in this container.
- Browser visual inspection: the current dark overview page was inspected in a Chromium viewport. This is not a complete accessibility or all-platform visual audit.

## CI configured, not locally proven

`ci.yml` defines frontend tests, Rust suites, and desktop compilation on Windows/macOS/Linux. `desktop-artifacts.yml` is a manual workflow that builds unsigned validation bundles and retains them for seven days. These workflow definitions have not been executed remotely in this session. Signing, notarization, publishing and automatic updater credentials are intentionally absent.

## Outstanding plan requirements

- Complete license/corresponding-source review and reproducible distribution of fixed-version aria2 binaries. The current empty platform-pin manifest ensures native HTTP fallback; no bundled acceleration is claimed.
- Forge/NeoForge installer adapters, explicit mod conflict guidance, and a supported backup/restore user workflow. Current modded support is Fabric/Quilt and supported Modrinth pack import/export.
- Real Microsoft login with the project's registered/approved public application ID, system credential persistence/refresh on each OS, trusted live skin-server login and Authlib-Injector launch.
- Real vanilla/modded game launches with recorded Java/game/loader combinations, plus runtime process-stop and crash-summary verification.
- Native install/upgrade/uninstall, signing/notarization, disk-full and migration failure drills, physical Windows/macOS/Linux verification, and documented supported architectures.

Mobile execution remains documentation-only in `FUTURE.md`; no Android/iOS/HarmonyOS runtime code has been added.
