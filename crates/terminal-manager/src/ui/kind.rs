//! Project type badges on tabs: detected from the terminal's command, then from the files in
//! its working folder.

use egui::{Align2, Color32, CornerRadius, FontId, Painter, Pos2, Stroke, Vec2};
use std::path::Path;
use tm_core::model::{Project, TerminalSpec};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    ClaudeCode,
    DotNet,
    Node,
    Rust,
    Python,
    Go,
    Java,
    Docker,
    Shell,
}

impl Kind {
    pub fn label(self) -> &'static str {
        match self {
            Kind::ClaudeCode => "Claude Code",
            Kind::DotNet => ".NET",
            Kind::Node => "Node.js",
            Kind::Rust => "Rust",
            Kind::Python => "Python",
            Kind::Go => "Go",
            Kind::Java => "Java",
            Kind::Docker => "Docker",
            Kind::Shell => "Shell",
        }
    }

    fn from_program(program: &str) -> Option<Kind> {
        let p = program.trim_start_matches(".\\").trim_start_matches("./").to_ascii_lowercase();
        let p = p.strip_suffix(".exe").or_else(|| p.strip_suffix(".cmd")).unwrap_or(&p);
        Some(match p {
            "claude" => Kind::ClaudeCode,
            "dotnet" => Kind::DotNet,
            "npm" | "npx" | "node" | "yarn" | "pnpm" | "bun" | "deno" | "nodemon" | "ng" | "vite" | "next" => Kind::Node,
            "cargo" | "rustc" => Kind::Rust,
            "python" | "python3" | "py" | "pip" | "uvicorn" | "gunicorn" | "flask" | "uv" | "poetry" => Kind::Python,
            "go" | "air" => Kind::Go,
            "java" | "mvn" | "mvnw" | "gradle" | "gradlew" => Kind::Java,
            "docker" | "docker-compose" | "podman" => Kind::Docker,
            _ => return None,
        })
    }

    fn from_folder(dir: &Path) -> Option<Kind> {
        let names: Vec<String> = std::fs::read_dir(dir)
            .ok()?
            .flatten()
            .filter_map(|e| e.file_name().to_str().map(|n| n.to_ascii_lowercase()))
            .collect();
        let has = |n: &str| names.iter().any(|x| x == n);
        let ends = |s: &str| names.iter().any(|x| x.ends_with(s));
        Some(if ends(".csproj") || ends(".sln") || ends(".slnx") || ends(".fsproj") {
            Kind::DotNet
        } else if has("package.json") {
            Kind::Node
        } else if has("cargo.toml") {
            Kind::Rust
        } else if has("pyproject.toml") || has("requirements.txt") || has("setup.py") {
            Kind::Python
        } else if has("go.mod") {
            Kind::Go
        } else if has("pom.xml") || has("build.gradle") || has("build.gradle.kts") {
            Kind::Java
        } else if has("docker-compose.yml") || has("compose.yaml") || has("dockerfile") {
            Kind::Docker
        } else {
            return None;
        })
    }

    /// Detects a terminal's project type. Reads its folder, so callers cache the result.
    pub fn detect(project: &Project, spec: &TerminalSpec) -> Kind {
        let command = spec.command.trim();
        let first = command.split_whitespace().next().unwrap_or("");
        Kind::from_program(first)
            .or_else(|| Kind::from_folder(&project.resolve_cwd(&spec.cwd)))
            .unwrap_or(Kind::Shell)
    }

    /// Cache key: detection depends on the command and the folder.
    pub fn cache_key(project: &Project, spec: &TerminalSpec) -> String {
        format!("{}|{}", spec.command.trim(), project.resolve_cwd(&spec.cwd).display())
    }

    /// Paints the badge (about 16x16) centered at `c`.
    pub fn paint(self, painter: &Painter, c: Pos2) {
        let rect = egui::Rect::from_center_size(c, Vec2::splat(16.0));
        let text = |s: &str, size: f32, color: Color32| {
            painter.text(c, Align2::CENTER_CENTER, s, FontId::proportional(size), color);
        };
        match self {
            Kind::ClaudeCode => {
                // A coral spark: rays around the center.
                let color = Color32::from_rgb(0xd9, 0x77, 0x57);
                for i in 0..8 {
                    let a = std::f32::consts::FRAC_PI_4 * i as f32;
                    let (inner, outer) = (2.0, if i % 2 == 0 { 7.5 } else { 5.5 });
                    let dir = Vec2::new(a.cos(), a.sin());
                    painter.line_segment([c + dir * inner, c + dir * outer], Stroke::new(2.0, color));
                }
            }
            Kind::DotNet => {
                painter.rect_filled(rect, CornerRadius::same(3), Color32::from_rgb(0x51, 0x2b, 0xd4));
                text("N", 11.0, Color32::WHITE);
            }
            Kind::Node => {
                // Hexagon, like the Node.js mark.
                let r = 8.0;
                let pts: Vec<Pos2> = (0..6)
                    .map(|i| {
                        let a = std::f32::consts::FRAC_PI_3 * i as f32 + std::f32::consts::FRAC_PI_6;
                        Pos2::new(c.x + r * a.cos(), c.y + r * a.sin())
                    })
                    .collect();
                painter.add(egui::Shape::convex_polygon(pts, Color32::from_rgb(0x5f, 0xa0, 0x4e), Stroke::NONE));
                text("js", 9.0, Color32::WHITE);
            }
            Kind::Rust => {
                painter.circle_filled(c, 8.0, Color32::from_rgb(0xce, 0x42, 0x2b));
                text("R", 10.5, Color32::WHITE);
            }
            Kind::Python => {
                // Blue and yellow halves.
                let (top, bottom) = rect.split_top_bottom_at_fraction(0.5);
                painter.rect_filled(top, CornerRadius { nw: 3, ne: 3, sw: 0, se: 0 }, Color32::from_rgb(0x37, 0x76, 0xab));
                painter.rect_filled(bottom, CornerRadius { nw: 0, ne: 0, sw: 3, se: 3 }, Color32::from_rgb(0xff, 0xd3, 0x43));
                text("Py", 8.5, Color32::from_rgb(0x1b, 0x1f, 0x22));
            }
            Kind::Go => {
                painter.rect_filled(rect, CornerRadius::same(8), Color32::from_rgb(0x00, 0xad, 0xd8));
                text("Go", 8.5, Color32::WHITE);
            }
            Kind::Java => {
                painter.rect_filled(rect, CornerRadius::same(3), Color32::from_rgb(0xe7, 0x6f, 0x00));
                text("J", 11.0, Color32::WHITE);
            }
            Kind::Docker => {
                painter.rect_filled(rect, CornerRadius::same(3), Color32::from_rgb(0x1d, 0x63, 0xed));
                text("D", 10.5, Color32::WHITE);
            }
            Kind::Shell => {
                painter.rect_stroke(rect.shrink(1.0), CornerRadius::same(3), Stroke::new(1.0, super::pal::dim()), egui::StrokeKind::Inside);
                text(">_", 8.5, super::pal::dim());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(command: &str) -> TerminalSpec {
        TerminalSpec { command: command.into(), ..Default::default() }
    }

    #[test]
    fn detects_from_command() {
        let p = Project { root: std::env::temp_dir(), ..Default::default() };
        assert_eq!(Kind::detect(&p, &spec("dotnet run --project api")), Kind::DotNet);
        assert_eq!(Kind::detect(&p, &spec("claude --continue")), Kind::ClaudeCode);
        assert_eq!(Kind::detect(&p, &spec("npm.cmd start")), Kind::Node);
        assert_eq!(Kind::detect(&p, &spec("cargo watch -x test")), Kind::Rust);
        assert_eq!(Kind::detect(&p, &spec("py -m uvicorn app:main")), Kind::Python);
    }

    #[test]
    fn detects_from_folder() {
        let dir = std::env::temp_dir().join(format!("tm-kind-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(dir.join("api")).unwrap();
        std::fs::write(dir.join("api").join("Billing.Api.csproj"), "").unwrap();
        std::fs::write(dir.join("package.json"), "{}").unwrap();
        let p = Project { root: dir.clone(), ..Default::default() };
        let api = TerminalSpec { cwd: "api".into(), ..spec("") };
        assert_eq!(Kind::detect(&p, &api), Kind::DotNet);
        assert_eq!(Kind::detect(&p, &spec("")), Kind::Node);
        assert_eq!(Kind::detect(&Project { root: dir.join("nope"), ..Default::default() }, &spec("")), Kind::Shell);
    }
}
