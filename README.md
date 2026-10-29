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

## Shortcuts

| Keys | Action |
| --- | --- |
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
