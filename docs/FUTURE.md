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

A planned third-party plugin, listed in the plugin workspace as `planned` (see
[`PLUGIN.md`](../PLUGIN.md)). It would switch the Aria character artwork between spring,
summer, autumn and winter based on the real date, and tint the launcher's surfaces to match
the current season.

### Why it cannot ship as a normal plugin

The declared hooks are `theme`, `background` and `sidebar`. Only the `launch` hook is
currently triggered; the other three are accepted in a manifest and reported honestly as
"declared only, not triggered yet". The plugin therefore needs an entire capability the
launcher does not have, not just a switch:

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

### Gates before it is implemented

1. Define the theme-override contract and prove it across all four presets, light/dark, and
   reduced-motion mode without regressing text contrast.
2. Set hard limits for plugin-supplied images (dimensions, bytes, decode memory) and test
   them with a deliberately oversized asset.
3. Decide how artwork assets are shipped and integrity-checked, and whether redistribution
   rights for each seasonal artwork set are cleared.
4. Only then move the manifest out of `PLANNED_PLUGINS` and let users install it normally.
