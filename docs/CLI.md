# MFMCL command line

`mfmcl` is the terminal frontend for MFMCL. It downloads, launches, backs up, and
configures the same instances and accounts the desktop application uses — there is
one data directory, one SQLite database, and one set of game files.

Nothing here opens a window. The only exception is Microsoft account sign-in, which
is not implemented on the command line yet; see [Limits](#limits).

## How you reach it

The two frontends are packaged differently per platform, because Windows makes one
binary serve both impossible (see [Why Windows differs](#why-windows-differs)).

| Platform | Launch the GUI | Use the CLI |
| --- | --- | --- |
| Linux, macOS | `mfmcl` | `mfmcl cli download …` |
| Windows | `mfmcl.exe` | `mfmcl-cli.exe download …` |

On Linux and macOS the GUI binary forwards `cli <function>` to the CLI before any
window is created, so a `.deb`, `.rpm` or AppImage contains one executable. On
Windows the CLI ships as its own console executable alongside the GUI.

The commands themselves are identical; only the program name before the first
command differs.

## Build

```sh
# GUI (and, on Linux/macOS, the CLI inside it)
npm run tauri -- build

# standalone CLI executable
cargo build --release -p mfmcl-cli
```

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

### Download progress

`download` reports progress the way a package manager does, rather than printing a
line per file:

```text
安装 1.21.1
已下载  47% [==========>          ] 412.3 MiB / 877.7 MiB  18.4 MiB/s  剩余 25s  71/3984 个文件
```

The progress line is rewritten in place (about eight times a second), so a full
1.21.1 install occupies one row of the terminal instead of several thousand. The
bar uses the `====>` shape so "almost done" never looks identical to "done".
While files are still being verified after the last byte arrives, the estimate
reads `收尾` instead of a misleading `—`.

The line shows the **whole task** — total bytes and file count across the entire
install — not the current file. A per-file figure drops to zero at every file
boundary and reads as if the install restarted.

When stderr is not a terminal (a pipe, a log file, CI), in-place rewriting is
impossible, so the output switches to one line per phase plus a closing summary:

```text
安装 1.21.1
↓ 获取游戏文件（1/73 个文件）  964 B / 89.7 MiB
↓ 获取资源文件（73/3984 个文件）  89.7 MiB / 877.7 MiB
完成 已获取 877.7 MiB，3984/3984 个文件，用时 4m 22s（平均 3.3 MiB/s）
```

`--quiet` suppresses all of it; errors and the final result still print.

`--colour auto|always|never` controls ANSI colour. `NO_COLOR` is honoured, and a
redirected stdout never receives escapes. `TERM=dumb` also disables colour and
in-place redrawing, on the assumption that the terminal cannot handle them.

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

## Why Windows differs

A Windows executable declares a subsystem in its PE header. A `windows`-subsystem
process has no console attached, so `println!` goes nowhere; a `console` subsystem
process always opens (or inherits) a console window, which flashes on every desktop
launch.

Neither can be made to work for both modes in one file:

- **GUI subsystem forwarding to a terminal** does not deliver output. `println!`
  writes nothing, and an explicit `AttachConsole` plus `CreateFileW("CONOUT$")` and
  `SetStdHandle` still produced zero bytes in a ConPTY. Both were measured on
  Windows with a minimal reproduction.
- **Console subsystem hiding its own window** requires detecting how the process was
  started. `AttachConsole(-1)` fails under ConPTY while `is_terminal()` returns true,
  so the check reports "started from Explorer" for a terminal launch and hides the
  window the user is reading.

Linux and macOS have no subsystem distinction: one executable opens a window when it
has no `cli` argument and writes a terminal when it does. That is why the packaging
differs rather than the commands.

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
