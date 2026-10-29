//! Turning a `ShellKind` into a program + args, and building a terminal's environment.

use crate::model::{EnvVar, ShellKind};
use std::path::PathBuf;

pub struct ShellCommand {
    pub program: String,
    pub args: Vec<String>,
}

impl ShellKind {
    pub fn label(&self) -> String {
        match self {
            ShellKind::Default => "Default".into(),
            ShellKind::Pwsh => "pwsh".into(),
            ShellKind::PowerShell => "Windows PowerShell".into(),
            ShellKind::Cmd => "cmd".into(),
            ShellKind::Bash => "bash".into(),
            ShellKind::Zsh => "zsh".into(),
            ShellKind::Wsl => "WSL".into(),
            ShellKind::Custom { program, .. } if program.is_empty() => "Custom".into(),
            ShellKind::Custom { program, .. } => program.clone(),
        }
    }
}

/// Shells offered in the editor for this platform (not counting Default and Custom).
pub fn choices() -> Vec<ShellKind> {
    if cfg!(windows) {
        vec![ShellKind::Pwsh, ShellKind::PowerShell, ShellKind::Cmd, ShellKind::Bash, ShellKind::Wsl]
    } else {
        vec![ShellKind::Bash, ShellKind::Zsh, ShellKind::Pwsh]
    }
}

/// Resolves the shell for a terminal, falling back to the project default and then the system default.
pub fn resolve(shell: &ShellKind, project_default: &ShellKind) -> ShellCommand {
    match shell {
        ShellKind::Default => match project_default {
            ShellKind::Default => system_default(),
            other => resolve(other, &ShellKind::Default),
        },
        ShellKind::Pwsh => cmd("pwsh", &["-NoLogo"]),
        ShellKind::PowerShell => cmd("powershell", &["-NoLogo"]),
        ShellKind::Cmd => cmd("cmd", &[]),
        ShellKind::Bash => cmd("bash", &[]),
        ShellKind::Zsh => cmd("zsh", &[]),
        ShellKind::Wsl => cmd("wsl", &[]),
        ShellKind::Custom { program, args } => {
            ShellCommand { program: program.trim().to_string(), args: split_args(args) }
        }
    }
}

fn cmd(program: &str, args: &[&str]) -> ShellCommand {
    ShellCommand { program: program.into(), args: args.iter().map(|a| a.to_string()).collect() }
}

fn system_default() -> ShellCommand {
    if cfg!(windows) {
        if find_in_path("pwsh").is_some() {
            cmd("pwsh", &["-NoLogo"])
        } else if find_in_path("powershell").is_some() {
            cmd("powershell", &["-NoLogo"])
        } else {
            cmd("cmd", &[])
        }
    } else {
        match std::env::var("SHELL") {
            Ok(sh) if !sh.is_empty() => cmd(&sh, &[]),
            _ => cmd("/bin/bash", &[]),
        }
    }
}

pub fn find_in_path(program: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    let exts: Vec<String> = if cfg!(windows) {
        std::env::var("PATHEXT")
            .unwrap_or_else(|_| ".EXE;.CMD;.BAT;.COM".into())
            .split(';')
            .map(|e| e.to_string())
            .chain(std::iter::once(String::new()))
            .collect()
    } else {
        vec![String::new()]
    };
    std::env::split_paths(&path).find_map(|dir| {
        exts.iter().map(|ext| dir.join(format!("{program}{ext}"))).find(|p| p.is_file())
    })
}

/// Splits an argument string on whitespace, keeping double-quoted parts together.
pub fn split_args(s: &str) -> Vec<String> {
    let mut args = Vec::new();
    let mut cur = String::new();
    let mut in_quotes = false;
    let mut has_arg = false;
    for c in s.chars() {
        match c {
            '"' => {
                in_quotes = !in_quotes;
                has_arg = true;
            }
            c if c.is_whitespace() && !in_quotes => {
                if has_arg {
                    args.push(std::mem::take(&mut cur));
                    has_arg = false;
                }
            }
            c => {
                cur.push(c);
                has_arg = true;
            }
        }
    }
    if has_arg {
        args.push(cur);
    }
    args
}

/// Project env first, then terminal env; a later key replaces an earlier one.
/// Keys compare case-insensitively on Windows. The system env is inherited by the PTY separately.
pub fn merge_env(project: &[EnvVar], terminal: &[EnvVar]) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    for var in project.iter().chain(terminal) {
        let key = var.key.trim();
        if key.is_empty() {
            continue;
        }
        let same = |k: &String| if cfg!(windows) { k.eq_ignore_ascii_case(key) } else { k == key };
        match out.iter_mut().find(|(k, _)| same(k)) {
            Some(slot) => slot.1 = var.value.clone(),
            None => out.push((key.to_string(), var.value.clone())),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn var(k: &str, v: &str) -> EnvVar {
        EnvVar { key: k.into(), value: v.into() }
    }

    #[test]
    fn terminal_env_overrides_project() {
        let merged = merge_env(&[var("PORT", "3000"), var("A", "1")], &[var("PORT", "5173"), var(" ", "x")]);
        assert_eq!(merged, vec![("PORT".into(), "5173".into()), ("A".into(), "1".into())]);
    }

    #[cfg(windows)]
    #[test]
    fn env_keys_case_insensitive_on_windows() {
        let merged = merge_env(&[var("Path", "a")], &[var("PATH", "b")]);
        assert_eq!(merged, vec![("Path".into(), "b".into())]);
    }

    #[test]
    fn split_args_handles_quotes() {
        assert_eq!(split_args(r#"-l  -c "echo hi" "" x"#), vec!["-l", "-c", "echo hi", "", "x"]);
        assert!(split_args("   ").is_empty());
    }

    #[test]
    fn default_falls_back_to_project_shell() {
        let c = resolve(&ShellKind::Default, &ShellKind::Zsh);
        assert_eq!(c.program, "zsh");
        let c = resolve(&ShellKind::Custom { program: " nu ".into(), args: "-l".into() }, &ShellKind::Default);
        assert_eq!((c.program.as_str(), c.args), ("nu", vec!["-l".to_string()]));
    }
}
