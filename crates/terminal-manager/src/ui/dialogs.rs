//! Modal editors and confirmations.

use super::{ACCENT, BAD, DIM, FG};
use crate::app::App;
use tm_core::model::{EnvVar, Project, ShellKind, TerminalSpec};
use tm_core::shell;
use egui::{Grid, Id, Modal, RichText, TextEdit, Ui};
use uuid::Uuid;

#[derive(Clone, Debug)]
pub enum ConfirmAction {
    RemoveProject(Uuid),
    RemoveTerminal(Uuid, Uuid),
}

#[derive(Clone, Debug)]
pub enum Dialog {
    Project { draft: Project, is_new: bool },
    Terminal { project: Uuid, draft: TerminalSpec, is_new: bool },
    Confirm { title: String, message: String, confirm_label: &'static str, action: ConfirmAction },
}

impl Dialog {
    pub fn project(draft: Project, is_new: bool) -> Self {
        Dialog::Project { draft, is_new }
    }

    pub fn terminal(project: Uuid, draft: TerminalSpec, is_new: bool) -> Self {
        Dialog::Terminal { project, draft, is_new }
    }

    pub fn confirm(title: String, message: String, confirm_label: &'static str, action: ConfirmAction) -> Self {
        Dialog::Confirm { title, message, confirm_label, action }
    }
}

enum Outcome {
    Open,
    Cancel,
    Confirm,
}

pub fn show(app: &mut App, ctx: &egui::Context) {
    let Some(mut dialog) = app.dialog.take() else { return };
    let running_in_project = match &dialog {
        Dialog::Terminal { draft, .. } => app.status(draft.id).is_running(),
        _ => false,
    };
    let terminal_project = match &dialog {
        Dialog::Terminal { project, .. } => app.ws.project(*project).cloned(),
        _ => None,
    };
    let modal = Modal::new(Id::unique("dialog")).show(ctx, |ui| {
        ui.set_width(460.0);
        match &mut dialog {
            Dialog::Project { draft, is_new } => project_editor(ui, draft, *is_new),
            Dialog::Terminal { draft, is_new, .. } => {
                terminal_editor(ui, draft, *is_new, running_in_project, terminal_project.as_ref())
            }
            Dialog::Confirm { title, message, confirm_label, .. } => confirm(ui, title, message, confirm_label),
        }
    });
    // Escape cancels (handled in the buttons); a click on the backdrop is ignored so edits aren't lost.
    match modal.inner {
        Outcome::Open => app.dialog = Some(dialog),
        Outcome::Cancel => app.focus_terminal = true,
        Outcome::Confirm => {
            app.focus_terminal = true;
            app.dialog_confirmed(dialog);
        }
    }
}

fn heading(ui: &mut Ui, text: &str) {
    ui.label(RichText::new(text).size(17.0).strong().color(FG));
    ui.add_space(8.0);
}

/// Cancel / primary button row. Returns the outcome; `can_save` disables the primary button.
fn buttons(ui: &mut Ui, primary: &str, can_save: bool) -> Outcome {
    let mut out = Outcome::Open;
    ui.add_space(10.0);
    ui.horizontal(|ui| {
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let save = ui.add_enabled(can_save, egui::Button::new(RichText::new(primary).color(FG)).fill(ACCENT.gamma_multiply(0.35)));
            if save.clicked() || (can_save && ui.input(|i| i.key_pressed(egui::Key::Enter) && i.modifiers.ctrl)) {
                out = Outcome::Confirm;
            }
            if ui.button("Cancel").clicked() || ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                out = Outcome::Cancel;
            }
        });
    });
    out
}

fn project_editor(ui: &mut Ui, p: &mut Project, is_new: bool) -> Outcome {
    heading(ui, if is_new { "New project" } else { "Edit project" });
    Grid::new("project-form").num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
        ui.label(RichText::new("Name").color(DIM));
        ui.add(TextEdit::singleline(&mut p.name).desired_width(f32::INFINITY));
        ui.end_row();

        ui.label(RichText::new("Folder").color(DIM));
        ui.horizontal(|ui| {
            let mut root = p.root.display().to_string();
            if ui.add(TextEdit::singleline(&mut root).desired_width(ui.available_width() - 80.0).font(egui::TextStyle::Monospace)).changed() {
                p.root = root.into();
            }
            if ui.button("Browse…").clicked()
                && let Some(dir) = rfd::FileDialog::new().set_directory(&p.root).pick_folder()
            {
                p.root = dir;
            }
        });
        ui.end_row();

        ui.label(RichText::new("Default shell").color(DIM));
        shell_picker(ui, "project-shell", &mut p.default_shell, "System default");
        ui.end_row();
    });
    ui.add_space(10.0);
    ui.label(RichText::new("Environment variables").color(DIM));
    env_editor(ui, "project-env", &mut p.env);
    ui.label(
        RichText::new("Every terminal in this project gets these. A terminal's own value overrides the project's.")
            .size(12.0)
            .color(DIM),
    );
    let root_ok = p.root.is_dir();
    if !root_ok {
        ui.label(RichText::new("This folder does not exist.").color(BAD));
    }
    buttons(ui, if is_new { "Add project" } else { "Save" }, !p.name.trim().is_empty() && root_ok)
}

fn terminal_editor(ui: &mut Ui, t: &mut TerminalSpec, is_new: bool, running: bool, project: Option<&Project>) -> Outcome {
    heading(ui, if is_new { "New terminal" } else { "Edit terminal" });
    Grid::new("terminal-form").num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
        ui.label(RichText::new("Name").color(DIM));
        ui.add(TextEdit::singleline(&mut t.name).desired_width(f32::INFINITY));
        ui.end_row();

        ui.label(RichText::new("Shell").color(DIM));
        shell_picker(ui, "terminal-shell", &mut t.shell, "Project default");
        ui.end_row();

        ui.label(RichText::new("Command").color(DIM));
        ui.add(
            TextEdit::singleline(&mut t.command)
                .hint_text("e.g. npm run dev (empty = plain shell)")
                .font(egui::TextStyle::Monospace)
                .desired_width(f32::INFINITY),
        );
        ui.end_row();

        ui.label(RichText::new("Working dir").color(DIM));
        ui.horizontal(|ui| {
            ui.add(
                TextEdit::singleline(&mut t.cwd)
                    .hint_text("project folder")
                    .font(egui::TextStyle::Monospace)
                    .desired_width(ui.available_width() - 80.0),
            );
            if let Some(p) = project
                && ui.button("Browse…").on_hover_text("Pick a folder; inside the project it's saved as a relative path").clicked()
            {
                let start = Some(p.resolve_cwd(&t.cwd)).filter(|d| d.is_dir()).unwrap_or_else(|| p.root.clone());
                if let Some(dir) = rfd::FileDialog::new().set_title("Choose the working folder").set_directory(start).pick_folder() {
                    t.cwd = p.cwd_for(&dir);
                }
            }
        });
        ui.end_row();
    });
    if let Some(p) = project {
        let dir = p.resolve_cwd(&t.cwd);
        if !dir.is_dir() {
            ui.label(RichText::new(format!("Folder not found: {}", dir.display())).size(12.0).color(BAD));
        }
    }
    ui.add_space(10.0);
    ui.label(RichText::new("Environment variables").color(DIM));
    env_editor(ui, "terminal-env", &mut t.env);
    ui.add_space(6.0);
    ui.checkbox(&mut t.auto_start, "Start automatically when the project opens");
    if running && !is_new {
        ui.label(RichText::new("Changes apply the next time this terminal restarts.").size(12.0).color(DIM));
    }
    buttons(ui, if is_new { "Create and start" } else { "Save" }, !t.name.trim().is_empty())
}

fn confirm(ui: &mut Ui, title: &str, message: &str, label: &str) -> Outcome {
    heading(ui, title);
    ui.label(RichText::new(message).color(FG));
    let mut out = Outcome::Open;
    ui.add_space(12.0);
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        if ui.add(egui::Button::new(RichText::new(label).color(FG)).fill(BAD.gamma_multiply(0.4))).clicked() {
            out = Outcome::Confirm;
        }
        if ui.button("Cancel").clicked() || ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            out = Outcome::Cancel;
        }
    });
    out
}

fn shell_picker(ui: &mut Ui, id: &str, shell: &mut ShellKind, default_label: &str) {
    ui.vertical(|ui| {
        let selected = match &*shell {
            ShellKind::Default => default_label.to_string(),
            ShellKind::Custom { .. } => "Custom…".to_string(),
            s => s.label(),
        };
        egui::ComboBox::from_id_salt(id).selected_text(selected).width(220.0).show_ui(ui, |ui| {
            ui.selectable_value(shell, ShellKind::Default, default_label);
            for choice in shell::choices() {
                let label = choice.label();
                ui.selectable_value(shell, choice, label);
            }
            let is_custom = matches!(shell, ShellKind::Custom { .. });
            if ui.selectable_label(is_custom, "Custom…").clicked() && !is_custom {
                *shell = ShellKind::Custom { program: String::new(), args: String::new() };
            }
        });
        if let ShellKind::Custom { program, args } = shell {
            ui.horizontal(|ui| {
                ui.add(TextEdit::singleline(program).hint_text("program, e.g. nu").desired_width(140.0).font(egui::TextStyle::Monospace));
                ui.add(TextEdit::singleline(args).hint_text("arguments").desired_width(160.0).font(egui::TextStyle::Monospace));
            });
        }
    });
}

fn env_editor(ui: &mut Ui, id: &str, vars: &mut Vec<EnvVar>) {
    let mut remove = None;
    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.set_width(ui.available_width());
        if vars.is_empty() {
            ui.label(RichText::new("No variables.").size(12.0).color(DIM));
        }
        Grid::new(id).num_columns(3).spacing([6.0, 4.0]).show(ui, |ui| {
            for (i, var) in vars.iter_mut().enumerate() {
                ui.add(TextEdit::singleline(&mut var.key).hint_text("NAME").desired_width(150.0).font(egui::TextStyle::Monospace));
                ui.add(TextEdit::singleline(&mut var.value).hint_text("value").desired_width(220.0).font(egui::TextStyle::Monospace));
                if ui.small_button("✕").on_hover_text("Remove").clicked() {
                    remove = Some(i);
                }
                ui.end_row();
            }
        });
        if ui.small_button("+ Add variable").clicked() {
            vars.push(EnvVar::default());
        }
    });
    if let Some(i) = remove {
        vars.remove(i);
    }
}
