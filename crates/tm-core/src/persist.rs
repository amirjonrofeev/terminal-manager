//! Loading and saving JSON state files (the host's workspace, the GUI's UI state).

use serde::Serialize;
use serde::de::DeserializeOwned;
use std::fs;
use std::io;
use std::path::Path;

pub struct Loaded<T> {
    pub value: T,
    /// Set when the file existed but could not be read; it was kept as `.bak`.
    pub warning: Option<String>,
}

pub fn load<T: DeserializeOwned + Default>(path: &Path) -> Loaded<T> {
    let text = match fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Loaded { value: T::default(), warning: None },
        Err(e) => {
            return Loaded { value: T::default(), warning: Some(format!("Could not read {}: {e}", path.display())) };
        }
    };
    match serde_json::from_str::<T>(&text) {
        Ok(value) => Loaded { value, warning: None },
        Err(e) => {
            let bak = path.with_extension("json.bak");
            let _ = fs::rename(path, &bak);
            Loaded {
                value: T::default(),
                warning: Some(format!(
                    "{} could not be parsed ({e}). It was kept as {} and an empty one was started.",
                    path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
                    bak.display()
                )),
            }
        }
    }
}

/// Writes to a temporary file and renames it over the target, so a crash never leaves a half-written file.
pub fn save<T: Serialize>(path: &Path, value: &T) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let json = serde_json::to_string_pretty(value).map_err(io::Error::other)?;
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, json)?;
    fs::rename(&tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{EnvVar, Project, ShellKind, TerminalSpec, Workspace};
    use std::path::PathBuf;

    fn temp_path(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("tm-test-{}", uuid::Uuid::new_v4()));
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
        ws.projects.push(project);

        save(&path, &ws).unwrap();
        let loaded = load::<Workspace>(&path);
        assert!(loaded.warning.is_none());
        assert_eq!(loaded.value, ws);
    }

    #[test]
    fn missing_file_is_empty() {
        let loaded = load::<Workspace>(&temp_path("nope.json"));
        assert!(loaded.warning.is_none());
        assert!(loaded.value.projects.is_empty());
    }

    #[test]
    fn corrupt_file_is_backed_up() {
        let path = temp_path("workspace.json");
        fs::write(&path, "{ not json").unwrap();
        let loaded = load::<Workspace>(&path);
        assert!(loaded.warning.is_some());
        assert!(path.with_extension("json.bak").exists());
        assert!(!path.exists());
    }

    #[test]
    fn old_files_load_with_defaults() {
        // v0.1 files had UI fields (active_project, window, active_tab) that now live in the GUI.
        let path = temp_path("workspace.json");
        fs::write(
            &path,
            r#"{"active_project":null,"window":null,"projects":[{"name":"x","active_tab":null,"terminals":[{"command":"ls"}]}]}"#,
        )
        .unwrap();
        let ws = load::<Workspace>(&path).value;
        let t = &ws.projects[0].terminals[0];
        assert_eq!(t.command, "ls");
        assert!(t.auto_start);
        assert!(ws.projects[0].open);
    }
}
