# MFMCL command line

`mfmcl` is the terminal frontend for MFMCL. It downloads, launches, backs up, and
configures the same instances and accounts the desktop application uses — there is
one data directory, one SQLite database, and one set of game files.

Nothing here opens a window. The only exception is Microsoft account sign-in, which
is not implemented on the command line yet; see [Limits](#limits).

## Install and run

Build from the repository root:

```sh
cargo build --release -p mfmcl-cli
```

The binary is `target/release/mfmcl` (`target\release\mfmcl.exe` on Windows).

## Commands

| Command | Purpose |
| --- | --- |
| `download` | Create an instance, install the game and an optional loader, install mods |
| `launch` | Start an installed instance and stream its log |
| `list` | Instances, versions, loaders, accounts, tasks, mods, Modrinth search |
| `backup` | Create, preview, and restore instance backups |
| `plugin` | List, enable, disable, inspect, add, and remove plugins |
| `config` | Read and write launcher settings |
| `account` | Offline profiles, selection, removal |
| `doctor` | Diagnose an instance without changing it |
| `remove` | Delete an instance (needs confirmation) |
| `help` | Long-form documentation, `mfmcl help --function <command>` |

Everything also answers `--help` for its exact flags.

## Examples

```sh
# Vanilla
mfmcl download -v 1.21.1

# Fabric, newest stable loader, with two mods
mfmcl download -v 1.21.1 --mod-loader fabric latest --mod jei --mod sodium

# NeoForge at a pinned loader version
mfmcl download -v 1.21.1 --mod-loader neoforge@21.1.80 --name "1.21.1_neoforge"

# Add a mod to an existing instance
mfmcl download --instance 1.21.1_neoforge -v 1.21.1 --mod jei

# Run it
mfmcl launch --instance 1.21.1_neoforge

# Back it up
mfmcl backup create --instance 1.21.1_neoforge --to "D:\Backups"

# See what is wrong with it
mfmcl doctor --instance 1.21.1_neoforge
```

## Conventions

**`--instance` takes a name or an id.** Names are matched case-insensitively; an
ambiguous name lists the candidates rather than picking one.

**`latest` resolves to the newest stable loader.** A concrete version is taken
literally, so `--mod-loader fabric 0.16.10` and `--mod-loader fabric@0.16.10` are
equivalent.

**`--mod` does not install dependencies.** Modrinth versions declare dependencies;
the CLI reports how many and leaves the choice to you. It also refuses to install
into a vanilla instance, because nothing would load those jars.

**Destructive commands need confirmation.** Without a terminal, a command that
would delete something declines unless `--yes` is passed. Silence is not consent.

## Output

`--json` affects stdout only. Progress and narration always go to stderr, so
`mfmcl list instances --json | jq` cannot be broken by a progress bar.

`--quiet` suppresses progress but keeps errors and the final result.

`--colour auto|always|never` controls ANSI colour. `NO_COLOR` is honoured, and a
redirected stdout never receives escapes.

On Windows the console output code page is switched to UTF-8 so Chinese text is
readable with the default Chinese console settings. Redirected output is written as
UTF-8 and is unaffected.

## Exit codes

| Code | Meaning |
| --- | --- |
| 0 | Success |
| 1 | The command failed |
| 2 | The command line was wrong |
| 3 | The user declined, or a destructive command lacked `--yes` |

## Sharing state with the desktop application

Both frontends resolve the data directory the same way
(`%APPDATA%\dev.mfmcl.launcher` on Windows). An instance created by `mfmcl` appears
in the GUI and vice versa.

`LauncherStore` opens one SQLite connection per process with a five-second busy
timeout and no WAL. Running the GUI and the CLI at the same moment on the same
instance can therefore make the second writer fail after that timeout. Install and
launch operations on a single instance are guarded in-process, not across
processes.

## Limits

- **No Microsoft sign-in on the command line.** The flow needs a browser and a
  profile-selection step; it is implemented in `launcher-auth` but not wired into
  this frontend. Use the desktop app to sign in, then `mfmcl launch` with the saved
  account. Offline profiles work fully (`mfmcl account add-offline <name>`).
- **No TUI.** The command surface is non-interactive by design so it composes in
  scripts.
- **No dependency graph resolution** for `--mod`; see the conventions above.
- **No CurseForge.** `list search` and `--mod` use Modrinth, matching the current
  state of the catalogue in `launcher-core`.

## Architecture

This binary contains no download, install, or launch logic of its own. Everything
it runs comes from `crates/launcher-runtime`, which is also intended for the
desktop shell. The runtime's two frontend seams are `RuntimeContext` (data root and
resource directory instead of `tauri::AppHandle::path()`) and `EventSink` (progress
instead of `app.emit`). The CLI implements the latter as terminal output.

That split exists so "how to launch" has one definition. Two copies drift, and the
drift shows up as "works in the GUI, fails in the CLI".
