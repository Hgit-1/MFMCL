# Architecture notes

## Workspace packages

- `crates/mc-metadata`: official Minecraft version manifest, version profile, library, launch argument, and asset-index models. The official version manifest is the trust root; mirror-sourced version profiles are accepted only after matching the official SHA-1 entry.
- `crates/download-engine`: explicit official/BMCLAPI source adapters, verified fallback candidates, bounded concurrency, retries, safe Range resume, pause/cancel checkpoints, temporary-file writes, SHA-1/SHA-256/size validation, atomic publication, and optional aria2 RPC integration.
- `crates/launcher-auth`: Microsoft public-client PKCE authorization and Xbox/XSTS/Java entitlement checks; HTTPS Yggdrasil authentication; backend-only credential/session types and OS credential-store adapter.
- `crates/launcher-core`: SQLite settings, account references, isolated instances and download history; vanilla install; supported Fabric/Quilt profile merge; mod enable/disable and Modrinth pack import/export; Java launch preparation and local crash diagnostics.
- `src-tauri`: desktop commands, OS app-data paths, keyring and system-browser adapters, loopback Microsoft callback server, Java child-process/log events, task controls, and native file/directory dialogs.
- `ui`: Chinese-first React/TypeScript interface, login consent/profile dialogs, beginner-oriented setup, theme controls, tasks, loader/mod manager, and diagnostics.

## Trust and file flow

```mermaid
flowchart LR
  OfficialIndex[Official version index] --> VerifiedMetadata[Version metadata verified against index]
  VerifiedMetadata --> ArtifactPlan[Artifact plan with expected hashes and sizes]
  ArtifactPlan --> SourceAdapter[Explicit official or approved BMCLAPI candidate]
  SourceAdapter --> TemporaryFile[Temporary download file]
  TemporaryFile --> IntegrityCheck[Size and hash verification]
  IntegrityCheck --> AtomicInstall[Atomic cache or instance placement]
  AtomicInstall --> JavaProcess[Java process in isolated instance directory]
```

The official index is never mirror-selected. Each metadata/artifact download keeps its original official URL and optional explicit mirror URL. The downloader does not apply a blanket host rewrite. Unhashed content is not combined across sources; modpack downloads require a provider hash. Files are verified before publication.

## Storage and secret boundaries

- SQLite stores settings, public account profile metadata, opaque system-keyring references, instances, and bounded installation-task history. Passwords and bearer/refresh tokens do not enter SQLite or frontend IPC.
- Online account credentials are stored using the operating system credential provider. An unavailable or locked provider causes login to fail rather than falling back to plaintext storage.
- Microsoft login uses a user-provided or build-configured public client ID, the system browser, a loopback callback, state validation, and PKCE. The launcher has no Microsoft client secret and does not borrow another application's client ID.
- Skin-server login is HTTPS-only. The user must confirm trust for the shown server root. Passwords are used only for the authentication request; the keyring receives the returned Yggdrasil tokens.
- Instance and game directories hold version files and user data. Each new instance uses a unique instance root and its own game directory.
- Diagnostic export requires a user-chosen destination, bounds file count/size, removes likely credential values and common token fields, and leaves originals in place. Raw game commands are never logged. The launcher latest log is also bounded per process.

## Download and aria2 controls

The Rust downloader remains the authority for URLs, expected hashes, installation destinations, and publication. aria2 is optional and used only for pinned artifacts. It runs with a random local RPC secret bound to IPv4 loopback, bounded global and per-file connections, and a per-session configuration file with restricted Unix permissions. The binary is launched only when its platform SHA-256 is pinned in the release resource manifest and its version output matches. This repository's manifest intentionally has no binary pins yet; development and CI therefore use the native Rust downloader. No aria2 binary is being distributed until its corresponding-source and license review is complete.

Task controls operate on cooperative checkpoints, HTTP stream reads, and aria2 tasks. The interface displays the active file and its known bytes without inventing an overall install percentage. Task records recover as interrupted after an unexpected exit.

## Supported scope and limits

Fabric and Quilt metadata profiles are merged into a separate launch overlay; the official vanilla profile remains intact. Existing Forge/NeoForge installers run arbitrary processor programs and need dedicated, audited adapters. They are not implemented and are not advertised as supported. Pack import is limited to the supported Modrinth format and Vanilla/Fabric/Quilt dependencies; the importer does not run processors. Online catalogue sourcing has not been integrated.

Windows/macOS/Linux are the intended desktop targets. CI compiles each target, but an actual signed release/install/upgrade/uninstall and live Minecraft launch must still be verified on physical runners before claiming release readiness. Android, iOS, and HarmonyOS research is isolated in `docs/FUTURE.md`; mobile game execution is not part of the workspace.
