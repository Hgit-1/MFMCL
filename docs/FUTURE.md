# Future platform exploration

This document records research only. The initial implementation targets a complete desktop launcher for Windows, macOS, and Linux. No Android, iOS, or HarmonyOS application code is in the initial scope.

## Android: local Java Edition execution

### Candidate areas to investigate

- Fold Craft Launcher (FCL) is an Android Java Edition launcher. Its published README describes an HMCL-derived core and a PojavLauncher backend. Treat this as an architecture/use-case reference, not a code source.
- PojavLauncher provides Android-oriented Java runtime and rendering integration patterns worth studying separately.
- MobileGlues describes a GL implementation for Minecraft Java Edition on host OpenGL ES 3.x, with OpenGL ES 3.0 as a stated minimum and 3.2 recommended. It is a renderer candidate, not a universal compatibility guarantee.
- Zalith Launcher documents another Android Java Edition experience based on the PojavLauncher family. Compare its device support, renderer interfaces, controls, and compatibility reporting.

### Proof-of-concept gates

1. Select a small, named set of ARM64 devices and record SoC, GPU, Android version, available memory, and OpenGL ES version.
2. Establish a lawful Microsoft login and entitlement flow on Android; never collect Microsoft passwords in the launcher UI.
3. Launch one vanilla version on-device, then run a Fabric smoke test. Record installation, startup, controls, suspension/resume, and crash behavior.
4. Evaluate renderer choices independently: system/vendor driver, ANGLE where appropriate, and MobileGlues only on devices meeting its requirements. Keep renderer selection reversible and diagnosable.
5. Maintain a device × game version × loader × renderer matrix. Mark combinations as tested, partially working, failing, or untested; do not claim universal Android support from one device.
6. Before bundling any runtime, renderer, or plugin, review its current license, binary redistribution conditions, notices, and update policy.

### Entry criteria for implementation

Start Android code only after the proof of concept can repeatedly install and launch vanilla on at least two representative physical devices, can explain failures from logs, and has an approved dependency/redistribution review. Modded support requires a separate compatibility gate.

## iOS IPA

Initially consider only a companion/management client and a development or ad-hoc IPA build. Validate current Apple distribution rules, signing, device capabilities, and review requirements before advertising any functionality. Local Java Edition execution, downloaded executable game content, and runtime/JIT requirements are a separate unresolved feasibility and policy investigation; no full local launcher is promised.

## HarmonyOS HAP

Initially consider only a native HarmonyOS companion client and HAP packaging experiment. Confirm the target HarmonyOS generation, public SDK APIs, signing/deployment access, and device availability. A HAP build does not imply compatibility with Android APK/JVM/rendering components or local Java Edition execution.

## Shared mobile product direction

If a mobile client becomes viable before local game execution, prioritize safe instance browsing and optional, user-authorized desktop pairing. Pairing must be off by default, limited to a trusted local connection, use an explicit one-time pairing secret, and expose only documented launcher actions.

## Aria with You: seasonal artwork plugin

**Shipped.** This section previously described the plugin as planned. It is now installed
by default and listed in `BUNDLED_PLUGINS`, so the gates below are historical. What
changed: the three hooks it needed (`theme`, `background`, `sidebar`) are now triggered by
`App.tsx`, and its artwork is generated, audited and packed by `design/tools/aria/`
(`gen.py`, `audit.py`, `seg.py`, `pack.py`).

The original reasoning is kept below because it is the template for the next plugin that
needs a capability the launcher does not have.

### Why it could not ship as a normal plugin at the time

The declared hooks were `theme`, `background` and `sidebar`. Only the `launch` hook was
triggered at the time; the other three were accepted in a manifest and reported honestly as
"declared only, not triggered yet". The plugin therefore needed an entire capability the
launcher did not have, not just a switch:

1. A theme override channel that lets a plugin substitute visual variables (accent, surface
   tint, radius) without breaking contrast in any of the four presets or in high-contrast
   mode.
2. A background source channel: plugin-supplied images, with decoding limits, size caps and
   a documented cache location, so a plugin cannot exhaust memory or disk.
3. A sidebar slot with a fixed footprint and a documented opt-out, so a plugin cannot push
   navigation off-screen on small windows.
4. A way to bundle and verify artwork assets inside a plugin package. The plugin asks for
   `theme`, `uiSlot` and `storage` only - deliberately no `network` (artwork must ship with
   the plugin, not be fetched at runtime) and no `filesystem` (it must not touch game files).

### Gates that were met

1. The theme-override contract is `utils/aria.ts::applySeasonTheme`, which routes the accent
   through `applyAccent` and never writes `--color-bg-*` or `--color-text-*`, so contrast
   stays the launcher's responsibility rather than the plugin's.
2. Artwork limits are enforced by `plugin_package.rs` on the whole package (32 MiB extracted,
   8 MiB per entry, 256 entries) and by the thumbnail step, which caps side art at 384 px.
3. Assets ship inside the `.mflp`; the index pins a SHA-256 and the installer verifies it
   before unpacking. Redistribution rights remain the author's responsibility, stated in the
   plugin's `author` field.
4. It moved out of `PLANNED_PLUGINS` into `BUNDLED_PLUGINS` and can now be disabled.

## Distribution through application stores

Recorded as research, not as a plan with a date. The question is whether MFMCL can be
installed by people who do not read a README, and the answer depends more on store policy
than on packaging work.

### What blocks it today

**The plugin system, structurally.** Users install plugin packages at runtime from an HTTPS
index; those packages may contain a `main.js` that runs inside the WebView. That is
"download and execute code which introduces or changes features" in the exact words of two
store policies:

- **Apple, App Review Guidelines 2.5.2**: an app must be self-contained and "may not
  download, install, or execute code which introduces or changes features or functionality
  of the app". A runtime plugin loader fails this by design, not by implementation detail.
  For reference, none of Prism Launcher, MultiMC or comparable Java Edition launchers are on
  the Mac App Store.
- **Microsoft Store, policy 10.2.5**: the same prohibition for products that fetch remote
  code changing core functionality after certification.

Disabling the plugin system for a store build is technically possible and would be the only
honest way to satisfy those policies. It is a product decision, not an engineering one:
a store build without plugins is a different product from the one described in
[`PLUGIN.md`](../PLUGIN.md).

**No code signing.** `tauri.conf.json` declares no `signingIdentity`, and
`.github/workflows/desktop-artifacts.yml` builds with `--no-sign` on purpose. Without
signing, macOS Gatekeeper blocks the app and Windows SmartScreen warns on first run. This
is a prerequisite for *any* distribution to non-technical users, store or not.

**Flathub requires a network-free build.** Its builder enforces the sandbox with networking
disabled, so `npm ci` and `cargo fetch` fail. Building there means vendoring the whole
dependency closure into the repository and committing lockfiles that no longer resolve at
build time. Feasible, but it changes how releases are produced.

### Candidate channels, ordered by reachability

| Channel | Blocking requirement | Verdict |
| --- | --- | --- |
| GitHub Releases + in-app updater | Code signing (Windows and macOS) | Closest to done; the only work is certificates, notarisation and `tauri-plugin-updater` |
| winget | A stable installer URL and SHA-256; signing strongly recommended but not enforced | Straightforward once releases are signed |
| Flathub | Network-free reproducible build, vendored dependencies | Possible; needs a separate build pipeline |
| Mac App Store | Removing or sandboxing the plugin system | Contradicts the current design |
| Microsoft Store | Same as Apple, policy 10.2.5 | Same |

### Why a store presence is probably the wrong goal anyway

Minecraft Java Edition players find launchers through communities and videos, not through
app-store search. The measurable goal behind "available to everyone" is *install without
friction*, and a signed downloadable installer plus an in-app updater achieves that without
asking a store to accept a plugin loader. A landing page with download links, version
history and published checksums reaches the same audience with no review queue.

### Gates before any store work starts

1. Windows and macOS releases are signed, notarised, and verified on physical machines;
   `--no-sign` disappears from the release workflow.
2. An install / upgrade / uninstall cycle is verified on all three platforms, including the
   path where an update arrives while the game is running.
3. A store build has an explicit answer to the plugin question: either plugins are compiled
   out (and the UI says so), or the store channel is dropped as incompatible.
4. For Flathub specifically: a reproducible offline build is proven to produce the same
   binary as the CI build, so the store is not shipping something different from what was
   tested.

Until 1 and 2 are done, no store submission should be attempted: an unsigned app would be
rejected or flagged regardless of how the plugin question is resolved, and a rejection
teaches nothing about the parts that actually need deciding.
