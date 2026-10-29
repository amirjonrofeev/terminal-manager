//! Loading and saving `workspace.json` in the user's config directory.

use crate::model::Workspace;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

pub fn config_dir() -> PathBuf {
    directories::BaseDirs::new()
        .map(|d| d.config_dir().to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."))
        .join("project-manager")
}

/// `PROJECT_MANAGER_WORKSPACE` overrides the location (handy for testing a separate workspace).
pub fn workspace_path() -> PathBuf {
    match std::env::var_os("PROJECT_MANAGER_WORKSPACE") {
        Some(p) if !p.is_empty() => PathBuf::from(p),
        _ => config_dir().join("workspace.json"),
    }
}

pub struct Loaded {
    pub workspace: Workspace,
    /// Set when the file existed but could not be read; it was kept as `.bak`.
    pub warning: Option<String>,
}

pub fn load(path: &Path) -> Loaded {
    let text = match fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            return Loaded { workspace: Workspace::default(), warning: None };
        }
        Err(e) => {
            return Loaded {
                workspace: Workspace::default(),
                warning: Some(format!("Could not read {}: {e}", path.display())),
            };
        }
    };
    match serde_json::from_str::<Workspace>(&text) {
        Ok(workspace) => Loaded { workspace, warning: None },
        Err(e) => {
            let bak = path.with_extension("json.bak");
            let _ = fs::rename(path, &bak);
            Loaded {
                workspace: Workspace::default(),
                warning: Some(format!(
                    "workspace.json could not be parsed ({e}). It was kept as {} and an empty workspace was started.",
                    bak.display()
                )),
            }
        }
    }
}

/// Writes to a temporary file and renames it over the target, so a crash never leaves a half-written file.
pub fn save(path: &Path, workspace: &Workspace) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let json = serde_json::to_string_pretty(workspace).map_err(io::Error::other)?;
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, json)?;
    fs::rename(&tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{EnvVar, Project, ShellKind, TerminalSpec};

    fn temp_path(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("pm-test-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        dir.join(name)
    }

    #[test]
    fn round_trip() {
        let path = temp_path("workspace.json");
        let mut ws = Workspace::default();
        let mut project = Project { name: "shop".into(), root: "W:/shop".into(), ..Default::default() };
        project.env.push(EnvVar { key: "A".into(), value: "1".into() });
        project.terminals.push(TerminalSpec {
            name: "dev".into(),
            command: "npm run dev".into(),
            shell: ShellKind::Custom { program: "nu".into(), args: "-l".into() },
            ..Default::default()
        });
        ws.active_project = Some(project.id);
        ws.projects.push(project);

        save(&path, &ws).unwrap();
        let loaded = load(&path);
        assert!(loaded.warning.is_none());
        assert_eq!(loaded.workspace, ws);
    }

    #[test]
    fn missing_file_is_empty() {
        let loaded = load(&temp_path("nope.json"));
        assert!(loaded.warning.is_none());
        assert!(loaded.workspace.projects.is_empty());
    }

    #[test]
    fn corrupt_file_is_backed_up() {
        let path = temp_path("workspace.json");
        fs::write(&path, "{ not json").unwrap();
        let loaded = load(&path);
        assert!(loaded.warning.is_some());
        assert!(path.with_extension("json.bak").exists());
        assert!(!path.exists());
    }

    #[test]
    fn missing_fields_use_defaults() {
        let path = temp_path("workspace.json");
        fs::write(&path, r#"{"projects":[{"name":"x","terminals":[{"command":"ls"}]}]}"#).unwrap();
        let ws = load(&path).workspace;
        let t = &ws.projects[0].terminals[0];
        assert_eq!(t.command, "ls");
        assert!(t.auto_start);
        assert!(ws.projects[0].open);
    }
}
