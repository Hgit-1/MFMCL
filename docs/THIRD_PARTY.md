# Third-party dependency review

This project starts with original code and documented protocols. It does not copy implementations or assets from PCL, HMCL, Fold Craft Launcher, Zalith Launcher, or MobileGlues.

## aria2

aria2 is optional and starts as a separate local process with a randomly generated RPC secret bound to IPv4 loopback. The application passes trusted download jobs through JSON-RPC and re-verifies the completed file before installation. RPC is not exposed to the network. If the process fails to start or download, the Rust HTTP implementation is used as fallback.

A reviewed 1.37.0 binary can now be produced and staged locally with
`scripts/aria2/build.sh` + `distribute.py`; on 2026-10-29 that path was exercised end to end
(hash-verified official source, static-feature build, stripped binary, seven-check RPC smoke
test passed, staged into `src-tauri/resources/aria2/<os>/<arch>/` with a matching SHA-256 pin
in `aria2-manifest.json`). The staged binary and its provenance are `.gitignore`d: staging is a
local build step, not a redistribution. **Committing or redistributing the binary still
requires** the GPL corresponding-source/notice review described below.

Previously: no aria2 binary was included in the repository. Before distributing one, pin the source revision and build configuration, verify the release checksum, include the applicable GPL license and corresponding source/rebuild instructions, and review how that distribution interacts with the selected license for the MFMCL application. Do not redistribute a binary until this review is complete.

## Authlib-Injector

The desktop skin-server launch adapter fetches release metadata and a SHA-256-pinned Authlib-Injector JAR from the project's HTTPS artifact service. It checks bounded metadata and download sizes and uses the JAR as an explicit Java agent for the trusted root chosen by the user. No JAR is included in this repository or bundled with the launcher. Review the upstream project's license, notices, release provenance and terms before any redistribution; runtime use of a third-party authentication service does not establish Microsoft ownership or authorize copying Minecraft files.

## BMCLAPI

BMCLAPI is an optional third-party mirror, not an official Mojang service. Users select it explicitly. Its source identity is retained in task events; downloaded bytes are verified against metadata from the official version index before publication.

## Minecraft content

The application downloads game files for the user's local installation and does not bundle or host Minecraft game assets, libraries, or client JARs.

## CurseForge API

CurseForge is not called today: the store searches Modrinth through the Rust backend, and no CurseForge client exists in `crates/` or `src-tauri/`. If it is added, the API requires a per-developer key in the `x-api-key` header, issued from the CurseForge for Studios console. Such a key is a credential: it must not be compiled into the launcher or committed, because every copy of the client would share the same quota and identity. The API also enforces terms of use on how content may be surfaced and downloaded; review them before shipping the integration. Verified request shapes, key format, limits, and the distribution-restricted-project case are recorded in [`CURSEFORGE.md`](CURSEFORGE.md).

## Dependency update process

For every release, record direct and transitive dependencies, review licenses, update security-sensitive libraries, and preserve required notices. Revisit upstream terms before adding a mod catalog or another content provider.
