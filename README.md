# MFMCL

A cross-platform Minecraft Java Edition launcher, starting with Windows, macOS, and Linux.

The first release is planned to manage official game versions, isolated instances, Java runtimes, mod loaders, resilient downloads, and understandable startup/crash diagnostics. Android, iOS, and HarmonyOS local game execution are research-only and tracked in [`docs/FUTURE.md`](docs/FUTURE.md).

## Project status

This repository is at the initial planning and scaffold stage. Features are not yet implemented unless explicitly documented as available.

## Development direction

- UI: React + TypeScript
- Desktop shell and platform bridge: Tauri 2
- Launcher core: Rust workspace
- App metadata: SQLite; credentials: operating-system credential store
- Reference notes: [`docs/REFERENCE_NOTES.md`](docs/REFERENCE_NOTES.md)

## Licensing and distribution

The project will not bundle Minecraft game files. Users must authenticate with an account that has the required entitlement. Third-party mod and launcher dependencies require individual license and redistribution review before shipping.
