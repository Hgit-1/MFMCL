# launcher-auth

Backend-only Minecraft Java authentication. All production requests use HTTPS,
refuse redirects, and have a 30-second total timeout (10-second connect timeout).
Errors never include response bodies, submitted credentials, or reqwest URLs.
Responses are capped at 1 MiB.

## Yggdrasil

1. Parse the authentication base URL (including `/authserver/` if required by
   that provider) and pass the exact explicitly approved URL in `trusted_servers`
   to `YggdrasilClient::new`.
2. Call `authenticate(username, password)`.
3. Present `YggdrasilLogin::available_profiles` if necessary, then consume the
   login with `select_profile(login, profile_id)`.
4. Save the resulting `credentials` in the system keyring, and use `session`
   backend-only for launch. `refresh(&credentials)` returns a new session and
   replacement credentials, preserving the profile selection.

Trust is exact normalized base-URL matching, not a hostname suffix match. A
configuration must not silently approve a provider URL imported from a modpack.
The crate does not install an authlib-injector Java agent; that is launch-layer
integration and may be needed by a third-party Yggdrasil provider.

## Microsoft

1. Supply your own registered public-client application ID and registered
   redirect URI through `MicrosoftConfig::new`. No application ID or client
   secret is embedded in this crate.
2. Call `begin_authorization()`, keep the returned object backend-only in
   memory, and open `authorization.url()` in the system browser.
3. The desktop host owns a short-lived loopback HTTP listener, opens the system browser, receives the redirect, and passes the complete callback URL to `MicrosoftFlow::finish_callback`. The flow object is consumed once, expires, and checks state, the exact callback URI, and duplicate parameters.
4. Save returned credentials in the system keyring. `refresh(&credentials)`
   repeats OAuth, Xbox Live, XSTS, Minecraft login, Java entitlement, and profile
   validation. Save replacement credentials after each successful refresh.

HTTP redirect URIs are accepted only for literal loopback IP addresses; other
redirect URIs must use HTTPS. Request transport remains HTTPS-only, including
for Microsoft OAuth token exchange. S256 PKCE uses OS randomness, and the
verifier is never present in the browser authorization URL.

`AuthSession` exposes `profile_id`, `profile_name`, a zeroizing `access_token`, optional Xbox `xuid`, public application `client_id`, and optional `expires_at` (Unix seconds). It is intentionally neither Debug nor serializable.
`Credentials` is serializable solely for a system-keyring adapter; never put its
serialized representation in launcher settings, logs, telemetry, command output,
or IPC. Its `Debug` implementation is intentionally absent. The host owns account
metadata, UI, credential persistence, callback delivery, and launch integration.

## Tests and workspace integration

`cargo test --manifest-path crates/launcher-auth/Cargo.toml` runs deterministic
PKCE/validation tests and local mocked HTTP tests for Yggdrasil refresh and the
complete Microsoft chain. Production constructors never expose the test HTTP
transport or Microsoft endpoint overrides. Tests do not need a real account or
live Microsoft services.

This crate is a member of the root Rust workspace. The optional `native-credentials` feature provides an explicit OS credential-store adapter; core authentication tests do not enable that feature. The desktop host enables it and runs blocking keyring operations outside the async executor.
