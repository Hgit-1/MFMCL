# MFMCL

A cross-platform Minecraft Java Edition launcher, beginning with Windows, macOS, and Linux.

## Current project state

The initial desktop foundation is implemented: React/Tauri workspace, settings and isolated-instance storage, official version metadata access, verified official/BMCLAPI download source selection, optional aria2 RPC support, vanilla installation core, offline account profiles and guarded launch preparation, live process/log event plumbing, theme customization, and local diagnostic summary/export. The desktop UI is still evolving and should be considered an early development build.

Microsoft identity and skin-server token login, loader installation, modpack import, packaged aria2 binaries, and platform installers are not complete. The interface does not simulate these operations as if they succeeded.

Android, iOS, and HarmonyOS local game execution remain research-only in [`docs/FUTURE.md`](docs/FUTURE.md).

## Development

See [`docs/BUILDING.md`](docs/BUILDING.md) for prerequisites, frontend preview, tests, and native desktop builds. See [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) for the module and trust boundaries.

The root Git repository excludes the nested `/PCL/` reference checkout. Reference observations and third-party distribution constraints are documented in [`docs/REFERENCE_NOTES.md`](docs/REFERENCE_NOTES.md) and [`docs/THIRD_PARTY.md`](docs/THIRD_PARTY.md).

## Content and accounts

MFMCL does not bundle Minecraft game files. Users must use an account authorized for the content they install. Offline profiles are explicitly identified and require a fresh confirmation each time launch is attempted; they do not bypass Microsoft entitlement checks or online-mode server authentication.
