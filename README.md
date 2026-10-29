# Terminal Manager

A small native app (Rust, egui, alacritty_terminal) for Windows and Linux that runs and groups the terminals your projects need, aimed first at Windows servers where many long-running consoles get cluttered.

Terminals run in a background **host** process, not in the window. Closing the window leaves them running; opening it again shows their output where it left off.

## Concepts

- **Project**: a folder, plus env vars and a default shell shared by its terminals.
- **Terminal**: a tab inside a project. It has a shell, an optional command that is typed into the shell when it starts (the shell stays open afterwards), a working dir relative to the project folder, its own env vars, and an auto-start flag.
- Env merge order: system env, then project env, then terminal env (keys are case-insensitive on Windows).

## How it runs

- `terminal-manager` is the window. If no host is running, it starts one for the current user (`terminal-manager --host`) in the background.
- The host owns the processes and the saved projects. It keeps the last 1 MB of each terminal's output and replays it when a window connects.
- `tm-host` is the same host as a standalone binary with no GUI dependencies.
- Windows service mode (terminals that survive logoff and start at boot) is coming next.

## Saved state

| What | Where |
| --- | --- |
| Projects and terminals (host) | Windows `%LOCALAPPDATA%\terminal-manager\host\workspace.json`, Linux `~/.local/share/terminal-manager/workspace.json` |
| Host log | `host.log` next to `workspace.json` |
| Window size, selected project and tab (per user) | Windows `%APPDATA%\terminal-manager\ui.json`, Linux `~/.config/terminal-manager/ui.json` |

Files are written by writing a temp file and renaming it over the old one. A file that can't be parsed is kept as `.bak`. On first start the host imports the v0.1 `workspace.json` from the per-user config folder.

For testing, `TERMINAL_MANAGER_DATA`, `TERMINAL_MANAGER_SOCKET` and `TERMINAL_MANAGER_CONFIG` point the host data, the pipe/socket name and the GUI config somewhere else.

## Logs and search

Every terminal's output is saved by the host, one file per day per terminal:

```
<data>\logs\<terminal id>\2026-10-29.log      (continues in 2026-10-29.1.log past the size limit)
2026-10-29T14:47:38.449+05:00	E	fail: sync for client 4471 timed out after 30s
```

Each line has the local time it arrived, a level (`I` info, `W` warning, `E` error, `S` lines from the host such as *started*, *stopped*, *exited with code 3*) and the text with colors removed. `logs\index.json` says which project and terminal each folder belongs to.

- **Logs view**: the *Logs* button above a terminal shows its saved lines, filtered by text, date range and level.
- **Search logs** (sidebar, Ctrl+Shift+F): search all projects or one, e.g. a client id over *Yesterday* or a custom date range, errors only. Click a result to open that terminal's log at that moment, with the lines around it.
- **Find** (Ctrl+F): highlight matches in the live screen; Enter / Shift+Enter step through them.
- **Badges**: tabs and projects count error (red) and warning (amber) lines you haven't looked at yet.
- **Tab icons**: each tab shows the project type, detected from the command (`dotnet`, `npm`, `cargo`, …) or the files in its folder (`*.csproj`, `package.json`, `Cargo.toml`, …).

`settings.json` in the data folder:

| Setting | Default | |
| --- | --- | --- |
| `retention_days` | 30 | Days of logs kept (0 = forever) |
| `max_file_mb` | 200 | A day's file continues in a new part past this size |
| `error_pattern` / `warning_pattern` | empty | Regex overriding what counts as an error / warning (empty = built-in, which understands `fail:`, `[ERR]`, `Exception`, stack traces, `warn:`, `[WRN]`, …) |

The host reads settings when it starts.

## Dev panel

For working on a project, not just running it: a panel tiled beside the service terminals (left by default; **⇄** moves it right, **✕** or the **Dev** button hides it).

- **+ → Claude Code / Shell / Custom…** starts a dev terminal in the project folder; **VS Code** opens the folder in VS Code.
- Dev terminals run **as you, in your own background host**: they get your PATH, your Claude Code login and your desktop (so `code .` opens a window). They keep running when you close the window, aren't written to the logs, aren't part of Start all, and don't auto-start after a reboot.
- The Windows service never runs dev terminals. When the app is connected to the service the panel is **off by default** (so servers don't start a per-user host); turn it on with **Dev** if you want it there too.

## Shortcuts

| Keys | Action |
| --- | --- |
| Ctrl+Shift+F | Search logs across terminals |
| Ctrl+F | Find in the current terminal's output |
| Ctrl+B | Collapse / expand the sidebar |
| Ctrl+Shift+N | Add project |
| Ctrl+Shift+T | New terminal |
| Ctrl+Shift+W | Remove terminal |
| Ctrl+Shift+R | Restart terminal |
| Ctrl+Tab / Ctrl+Shift+Tab | Next / previous tab |
| Ctrl+C | Copy the selection, or send ^C when nothing is selected |
| Ctrl+V, Ctrl+Shift+V, right click | Paste (right click copies when there is a selection) |

## Build

```sh
cargo build --release --workspace   # target/release/terminal-manager(.exe) and tm-host(.exe)
cargo test --workspace
cargo run -p terminal-manager
```

The workspace has three crates: `tm-core` (model, protocol, IPC), `tm-host` (process host) and `terminal-manager` (GUI).

Linux needs the usual winit/wgpu dev packages (for example `libxkbcommon-dev libwayland-dev libx11-dev`).
