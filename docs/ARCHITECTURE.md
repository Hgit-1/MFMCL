# Architecture notes

## Workspace packages

- `crates/mc-metadata`: official Minecraft version manifest, version profile, libraries, launch arguments, and asset index data models. The official version manifest is treated as the trust root; mirror-sourced version profiles are accepted only after matching their official SHA-1 entry.
- `crates/download-engine`: source candidates, BMCLAPI URL adapters for known host/path families, bounded parallel downloads, temporary-file writes, SHA-1/SHA-256/size verification, atomic publication, and optional aria2 RPC sidecar integration.
- `crates/launcher-core`: SQLite app settings, account references, isolated instance records, vanilla installation, native library extraction, offline launch argument preparation, and local crash diagnostics.
- `src-tauri`: desktop commands, OS app-data discovery, Java child-process management, live game output, native directory picker, and frontend event transport.
- `ui`: React/TypeScript interface and accessible theme, instance, account prompt, download, and diagnostics screens.

## Trust and file flow

```mermaid
flowchart LR
  OfficialIndex[Official version index] --> VerifiedMetadata[Version metadata verified against index]
  VerifiedMetadata --> ArtifactPlan[Artifact plan with expected hashes and sizes]
  ArtifactPlan --> SourceAdapter[Official or approved BMCLAPI candidate]
  SourceAdapter --> TemporaryFile[Temporary download file]
  TemporaryFile --> IntegrityCheck[Size and hash verification]
  IntegrityCheck --> AtomicInstall[Atomic cache or instance placement]
  AtomicInstall --> JavaProcess[Java process in isolated instance directory]
```

The official index is not mirror-selected. Each metadata/artifact download keeps its original official URL and an optional explicit mirror URL. The downloader never applies a blanket host rewrite. A mirror is a candidate only when official metadata provides an expected hash; unverified bytes are not committed to the final destination.

## Storage boundaries

- The SQLite database records settings, selected profiles, instance indexes, and task status; it must not contain passwords or bearer tokens.
- Instance and game directories hold version files and user data. Each new instance uses a unique instance root and its own game directory.
- OS credentials belong in a future platform credential-store adapter. Until that adapter is present, the UI must not claim Microsoft or skin-server tokens are persisted.
- Diagnostic export operates only after the user chooses a directory. It limits file count and size, redacts common credentials, and leaves original logs in place.

## Current implementation boundaries

The first native slice currently installs and prepares vanilla profiles and supports explicit offline launch confirmation. Microsoft account exchange, Yggdrasil login/token lifecycle, loader installation, modpack import, task database history, and cross-platform release signing require subsequent implementation. UI controls indicate a preview or pending integration rather than simulating those operations.

Android, iOS, and HarmonyOS research is isolated in `docs/FUTURE.md` and is not part of the native workspace build targets.
