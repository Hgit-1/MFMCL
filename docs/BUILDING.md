# Building MFMCL

## Prerequisites

- Node.js 22 or newer and npm.
- Rust stable with the `rustfmt` component.
- Tauri 2 platform prerequisites: WebView2 and MSVC build tools on Windows; Xcode command-line tools on macOS; GTK 3, WebKitGTK 4.1, D-Bus, `pkg-config`, and the Tauri build packages on Linux.
- A system credential service for online login (Windows Credential Manager, macOS Keychain, or a running Linux Secret Service provider).

The Rust toolchain requirement applies to the desktop backend; frontend preview mode can be built independently with npm.

## Frontend preview

```sh
npm ci --prefix ui
npm run dev --prefix ui
```

The preview supports theme settings and sample version metadata. Native login, download, install, and launch commands require the Tauri desktop app; preview mode does not simulate them as successful operations.

## Automated checks

```sh
cargo fmt --all -- --check
cargo test -p mc-metadata -p download-engine -p launcher-core -p launcher-auth
npm ci --prefix ui
npm run lint --prefix ui
npm run build --prefix ui
npx --prefix ui playwright install chromium
npm run test --prefix ui
```

The authentication tests use local mock services, and browser tests use mocked native IPC; neither proves a real account/game launch. Continuous integration runs these checks plus desktop-backend compile jobs for Linux, macOS, and Windows. The manual `desktop-artifacts.yml` workflow can also build unsigned short-retention artifacts on target runners; those files are test outputs, not supported or signed releases.

## Desktop app

Run from the repository root:

```sh
npm run tauri --prefix ui -- dev
npm run tauri --prefix ui -- build
```

The Tauri command changes to the repository root while the frontend hooks execute in `ui`. The development server binds to `127.0.0.1:1420`; port collisions fail instead of selecting an unexpected port. Build each native installer on its target operating system. The current project has not completed signed release packaging or a clean install/upgrade/uninstall run on all three systems.

## Microsoft account configuration

Microsoft login requires an application public client ID registered and authorized for the relevant Xbox/Minecraft services. Supply it through the login dialog or set `MFMCL_MICROSOFT_CLIENT_ID` when building. Configure an appropriate desktop/loopback redirect URI in that registration. MFMCL contains no client secret and must not borrow another launcher's application ID. A denied/unavailable OS credential provider is reported as a login error; no plaintext fallback is used.

## Optional aria2 sidecar

The runtime expects a platform binary at:

```text
<application resource directory>/aria2/<os>/<architecture>/aria2c[.exe]
```

`src-tauri/resources/aria2-manifest.json` pins allowed version and per-platform SHA-256 values. It intentionally contains no binary pins yet, so the current build uses the Rust downloader even if aria2 is enabled in settings. Before a release includes aria2, pin the exact binary, verify the corresponding source/build, include the GPL license and required source/notices, and complete distribution review. aria2 is never discovered and executed from an arbitrary PATH location.

## Release checklist

- Build, install, run, upgrade, and uninstall on Windows, macOS, and Linux; verify signatures and native credential-store behavior on each system.
- Test clean profiles and real entitled Microsoft accounts, explicitly trusted skin servers, Java versions, vanilla launch, and the supported Fabric/Quilt game-version matrix.
- Test no network, mirror mismatch, hash tampering, disk full, pause/cancel/retry, interrupted task recovery, and verify no account token appears in frontend events, logs, SQLite, or diagnostic archives.
- Confirm packages include no Minecraft content, private credentials, saves, or user logs. Record architecture support and Linux system-library requirements.
- Complete aria2 source/license review before enabling any binary pins. Forge/NeoForge, online content catalogues, and mobile game execution are outside the current implemented scope.
