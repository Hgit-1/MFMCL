# Building MFMCL

## Prerequisites

- Node.js 22 or newer and npm.
- Rust stable with the `rustfmt` component.
- Tauri 2 platform prerequisites: WebView2 and MSVC build tools on Windows; Xcode command-line tools on macOS; GTK 3, WebKitGTK 4.1, D-Bus, `pkg-config`, and the Tauri build packages on Linux.

The Rust toolchain requirement applies to the launcher desktop backend; frontend preview mode can be built independently with npm.

## Frontend preview

```sh
npm ci --prefix ui
npm run dev --prefix ui
```

The preview uses local browser storage for theme preferences and sample version metadata. It does not download or launch Minecraft. Use the Tauri desktop app to exercise native commands.

## Core tests

```sh
cargo fmt --all -- --check
cargo test -p mc-metadata -p download-engine -p launcher-core
```

## Desktop app

```sh
npm ci --prefix ui
npm run tauri --prefix ui dev
```

Tauri starts Vite at `http://127.0.0.1:1420` and connects the Rust backend. Production bundles are built using the platform-specific packaging command, for example:

```sh
npm run tauri --prefix ui build
```

Build on each target operating system to produce its native installer. Linux system-library dependencies are not bundled by the application.

## Optional aria2 sidecar

The runtime looks for aria2 at:

```text
<application resource directory>/aria2/<os>/<architecture>/aria2c[.exe]
```

The project does not currently check in or redistribute aria2 binaries. Before a release bundles the sidecar, pin its version, verify its checksum/signature, include the corresponding source and notices, and complete license/distribution review. If no compatible sidecar exists, the Rust HTTP downloader remains the fallback.

## Release checklist

- Build and launch on Windows, macOS, and Linux; verify platform signing and installer behavior.
- Verify official manifest metadata, file hashes, mirror fallback, Java detection, account prompts, logs, and crash export on each platform.
- Check all bundled dependency licenses and archive the exact source/binary versions used for the release.
- Confirm the application package does not contain Minecraft game files, account tokens, user saves, or crash logs.
