# Building MFMCL

## Prerequisites

- Node.js 22 or newer and npm.
- Rust stable with the `rustfmt` component.
- Tauri 2 platform prerequisites: WebView2 and MSVC build tools on Windows; Xcode command-line tools on macOS; GTK 3, WebKitGTK 4.1, D-Bus, `pkg-config`, and the Tauri build packages on Linux.
- A system credential service for online login (Windows Credential Manager, macOS Keychain, or a running Linux Secret Service provider).

The Rust toolchain requirement applies to the desktop backend; frontend preview mode can be built independently with npm.

## Frontend preview

```sh
npm ci --prefix design/flowlaunch-ui
npm run dev --prefix design/flowlaunch-ui
```

The development server binds to `127.0.0.1:5173`; open that address in a browser. The preview supports theme settings and sample version metadata. Native login, download, install, and launch commands require the Tauri desktop app; preview mode does not simulate them as successful operations. UI changes follow the browser-preview-first workflow in [`UI_WORKFLOW.md`](../UI_WORKFLOW.md).

## Automated checks

```sh
cargo fmt --all -- --check
cargo test -p mc-metadata -p download-engine -p launcher-core -p launcher-auth
npm ci --prefix design/flowlaunch-ui
npm run typecheck --prefix design/flowlaunch-ui
npm run build --prefix design/flowlaunch-ui
```

Opt-in tests that need a real environment (skipped by default):

```sh
# Real JVM: compiles and runs an original tiny fixture; validates log draining and kill.
cargo test -p launcher-core --test jvm_smoke -- --ignored
# Real aria2 sidecar: full engine path with a locally built 1.37.0 binary.
MFMCL_ARIA2_BINARY=src-tauri/resources/aria2/linux/x86_64/aria2c \
  cargo test -p download-engine real_aria2 -- --ignored
```

The authentication tests use local mock services, so they do not prove a real account/game launch. Continuous integration runs these checks plus desktop-backend compile jobs for Linux, macOS, and Windows. The manual `desktop-artifacts.yml` workflow can also build unsigned short-retention artifacts on target runners; those files are test outputs, not supported or signed releases.

## Desktop app

Run from the repository root:

```sh
npm run tauri -- dev
npm run tauri -- build
```

The Tauri command changes to the repository root while the frontend hooks execute in `design/flowlaunch-ui`. The desktop development server binds to `127.0.0.1:5173` (see `src-tauri/tauri.conf.json`); port collisions fail instead of selecting an unexpected port. Build each native installer on its target operating system. The current project has not completed signed release packaging or a clean install/upgrade/uninstall run on all three systems.

## Native Linux build without root

On a locked-down Linux host (no sudo), GTK/WebKitGTK development files can be provided through a local sysroot instead of system installation:

```sh
# 1. Resolve the dependency closure and download the missing debs (no root needed).
mkdir -p /tmp/mfmcl-sysroot/debs && cd /tmp/mfmcl-sysroot
apt-cache depends --recurse --no-recommends --no-suggests --no-conflicts \
  --no-breaks --no-replaces --no-enhances --no-pre-depends \
  libgtk-3-dev libwebkit2gtk-4.1-dev libayatana-appindicator3-dev librsvg2-dev \
  libdbus-1-dev libssl-dev libxdo-dev pkg-config \
  | LC_ALL=C grep "^\w" | LC_ALL=C sort -u > pkgs.txt
dpkg-query -W -f='${Package}\n' | LC_ALL=C sort -u > installed.txt
LC_ALL=C comm -23 pkgs.txt installed.txt \
  | grep -vE ":i386$|aspell|hunspell|myspell|-doc$|-l10n$|locale" > final.list
(cd debs && while read -r p; do [ -n "$p" ] && apt-get download "$p" || true; done < ../final.list)

# 2. Extract and close dangling dev symlinks against the system's runtime libs.
mkdir -p root && for f in debs/*.deb; do dpkg-deb -x "$f" root/; done
cd root/usr/lib/x86_64-linux-gnu
for link in *.so; do [ -L "$link" ] || continue; t=$(readlink "$link")
  [ -e "$t" ] || cp "/usr/lib/x86_64-linux-gnu/$t" "$t" 2>/dev/null || true; done

# 3. Bridge Debian's multiarch OpenSSL headers for -dev builds.
ln -sf ../x86_64-linux-gnu/openssl/opensslconf.h root/usr/include/openssl/ 2>/dev/null || true

# 4. Compile (check links nothing; for `cargo build` also set LIBRARY_PATH
#    because rust-lld places RUSTFLAGS -L entries after -l arguments).
export SYS=/tmp/mfmcl-sysroot/root
export PKG_CONFIG_PATH="$SYS/usr/lib/x86_64-linux-gnu/pkgconfig:$SYS/usr/share/pkgconfig"
export PKG_CONFIG_SYSROOT_DIR="$SYS"
cargo check -p mfmcl-desktop --tests
LIBRARY_PATH="$SYS/usr/lib/x86_64-linux-gnu:$SYS/usr/lib:/usr/lib/x86_64-linux-gnu" \
  cargo build -p mfmcl-desktop
```

This produces a linked `target/debug/mfmcl-desktop` whose dynamic dependencies all resolve. Running the windowed app still needs a display/session; headless containers only support compile and unit verification. The same sysroot supplies OpenSSL development files for `scripts/aria2/build.sh` (pass the same `PKG_CONFIG_PATH`/`PKG_CONFIG_SYSROOT_DIR`, plus `-I` bridging for Debian's multiarch OpenSSL headers as shown in step 3).

## Building and staging aria2 locally

`scripts/aria2/build.sh WORKDIR` downloads the SHA-256-locked official 1.37.0 source, builds a stripped-feature static binary, verifies its version, and packages a provenance archive. `scripts/aria2/smoke.py BINARY` then exercises real loopback JSON-RPC behaviour (secret auth, checksum verification, cancellation, TLS rejection). Finally:

```sh
sha256sum WORKDIR/aria2-1.37.0-<os>-<arch>.tar.gz
python3 scripts/aria2/distribute.py stage \
  --archive WORKDIR/aria2-1.37.0-<os>-<arch>.tar.gz \
  --archive-sha256 <archive sha256> --tauri-dir src-tauri
```

stages the binary under `src-tauri/resources/aria2/<os>/<arch>/` and pins it in `aria2-manifest.json`. Staged binaries and pins are local build outputs (`.gitignore`d): committing or redistributing them awaits the GPL corresponding-source and license review in `docs/THIRD_PARTY.md`.

## Microsoft account configuration

Microsoft login requires an application public client ID registered and authorized for the relevant Xbox/Minecraft services. Supply it through the login dialog or set `MFMCL_MICROSOFT_CLIENT_ID` when building. Configure an appropriate desktop/loopback redirect URI in that registration. MFMCL contains no client secret and must not borrow another launcher's application ID. A denied/unavailable OS credential provider is reported as a login error; no plaintext fallback is used.

## CurseForge API key (optional)

CurseForge browsing needs an API key from <https://console.curseforge.com/>. There are two ways to supply one:

1. **At runtime by the user** (recommended for published builds). The key is validated once and then stored in the OS credential store; the database only records that it exists. The web view never sees it — every request is signed by the Rust backend.
2. **At build time** via `MFMCL_CURSEFORGE_API_KEY`. This is compiled into the binary with `option_env!` and acts as the fallback when the user has not saved a key of their own.

`src-tauri/build.rs` reads the variable from the environment **or** from `<repo>/.env` and forwards it with `cargo:rustc-env`. You no longer have to `source scripts/tauri-env.sh` first: with a `.env` present, a plain `cargo build` / `npm run tauri build` ships the default key. A real environment variable still wins, so CI can set — or blank — it externally.

Setting the variable to an **empty string** disables the built-in key even when `.env` has one. That is the supported way to produce a build for other people from a machine that also holds a local key:

```sh
MFMCL_CURSEFORGE_API_KEY= npm run tauri -- build
```

Local builds get the variable from `.env` at the repository root (`scripts/tauri-env.sh` loads it; `.env` is git-ignored). Copy `.env.example` to get the list of supported variables.

**Treat a compiled-in key as public.** `strings` on the produced binary prints it, and every copy of that build shares one quota and one identity. Therefore:

- Keep `MFMCL_CURSEFORGE_API_KEY` empty for anything you hand to other people.
- The GitHub artifact workflow does not set it, so those builds ship without a default key and prompt the user instead.
- A user-saved key always takes precedence over the compiled-in one, and "clear key" falls back to the built-in value rather than disabling CurseForge.

## Launcher updates (optional)

Set `MFMCL_UPDATE_MANIFEST` at build time to an HTTPS JSON manifest:

```json
{
  "version": "0.2.0",
  "notes": "…",
  "platforms": {
    "linux-x86_64": { "url": "https://…/MFMCL-0.2.0.AppImage", "sha256": "…" },
    "windows-x86_64": { "url": "https://…/MFMCL-0.2.0.exe", "sha256": "…" },
    "macos-aarch64": { "url": "https://…/MFMCL-0.2.0.dmg", "sha256": "…" }
  }
}
```

Version comparison is `major.minor.patch` with optional `-pre.N` suffixes (a same-core pre-release never updates another pre-release). The check enforces HTTPS, bounded manifest size, per-release SHA-256, and a bounded installer download size. Downloads are verified before publication into a user-chosen directory; the launcher does not replace its own running process. See `docs/REFERENCE_NOTES.md` for the PCL2 interaction review behind this design.

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
