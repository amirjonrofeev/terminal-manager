# Project Manager

A small native app (Rust, egui, alacritty_terminal) for Windows and Linux that remembers your projects and the terminals you run in them. Close it or reboot, open it again, and every open project comes back with its auto-start commands running.

## Concepts

- **Project**: a folder, plus env vars and a default shell shared by its terminals.
- **Terminal**: a tab inside a project. It has a shell, an optional command that is typed into the shell when it starts (the shell stays open afterwards), a working dir relative to the project folder, its own env vars, and an auto-start flag.
- Env merge order: system env, then project env, then terminal env (keys are case-insensitive on Windows).

## Saved state

`workspace.json` is saved in:

- Windows: `%APPDATA%\project-manager\`
- Linux: `~/.config/project-manager/`

It is written at most once a second and on quit, by writing a temp file and renaming it over the old one. If the file can't be parsed, it is kept as `workspace.json.bak` and the app starts with an empty workspace. Set `PROJECT_MANAGER_WORKSPACE=<path>` to use a different file.

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
cargo build --release   # target/release/project-manager(.exe)
cargo test
```

Linux needs the usual winit/wgpu dev packages (for example `libxkbcommon-dev libwayland-dev libx11-dev`).
