# Third-party dependency review

This project starts with original code and documented protocols. It does not copy implementations or assets from PCL, HMCL, Fold Craft Launcher, Zalith Launcher, or MobileGlues.

## aria2

aria2 is optional and starts as a separate local process with a randomly generated RPC secret bound to IPv4 loopback. The application passes trusted download jobs through JSON-RPC and re-verifies the completed file before installation. RPC is not exposed to the network. If the process fails to start or download, the Rust HTTP implementation is used as fallback.

No aria2 binary is currently included in the repository. Before distributing one, pin the source revision and build configuration, verify the release checksum, include the applicable GPL license and corresponding source/rebuild instructions, and review how that distribution interacts with the selected license for the MFMCL application. Do not redistribute a binary until this review is complete.

## BMCLAPI

BMCLAPI is an optional third-party mirror, not an official Mojang service. Users select it explicitly. Its source identity is retained in task events; downloaded bytes are verified against metadata from the official version index before publication.

## Minecraft content

The application downloads game files for the user's local installation and does not bundle or host Minecraft game assets, libraries, or client JARs.

## Dependency update process

For every release, record direct and transitive dependencies, review licenses, update security-sensitive libraries, and preserve required notices. Revisit upstream terms before adding a mod catalog or another content provider.
