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
