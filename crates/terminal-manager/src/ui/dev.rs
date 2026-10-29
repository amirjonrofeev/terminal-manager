//! The dev panel: a project's developer terminals (Claude Code, shells), tiled beside the service
//! terminals. They run in the signed-in user's own host, so they have the user's tools, login
//! and desktop (`code .` opens a window), and keep running when the window closes.

use super::main_area::status_color;
use super::{dot, ACCENT, BG, DIM, FG, LINE, SIDE};
use crate::app::{Action, App, Connection, DevPreset, DevSide};
use crate::term::TerminalView;
use egui::{Align, Align2, CentralPanel, CornerRadius, FontId, Frame, Id, Layout, Margin, Panel, RichText, Sense, Stroke, Ui};
use std::sync::OnceLock;
use tm_core::model::TerminalSpec;
use tm_core::protocol::Status;
use uuid::Uuid;

fn claude_available() -> bool {
    static FOUND: OnceLock<bool> = OnceLock::new();
    *FOUND.get_or_init(|| tm_core::shell::find_in_path("claude").is_some())
}

fn editor_available() -> bool {
    static FOUND: OnceLock<bool> = OnceLock::new();
    *FOUND.get_or_init(crate::client::editor_available)
}

pub fn panel(app: &mut App, ui: &mut Ui, pid: Uuid) {
    let terms = app.dev_terminals(pid);
    let active = app.active_dev_tab(pid);

    Panel::top("dev-head")
        .frame(Frame::new().fill(SIDE).stroke(Stroke::new(1.0, LINE)))
        .show(ui, |ui| header(app, ui, pid, &terms, active));

    let status = match &app.dev_connection {
        Connection::Connected(_) => None,
        Connection::Connecting => Some("Starting your dev host…".to_string()),
        Connection::Disconnected(reason) | Connection::Denied(reason) => Some(format!("{reason} Retrying…")),
    };
    if let Some(msg) = status {
        CentralPanel::no_frame().show(ui, |ui| {
            ui.vertical_centered(|ui| {
                ui.add_space(ui.available_height() / 3.0);
                ui.label(RichText::new(msg).color(DIM));
            });
        });
        return;
    }

    let Some(spec) = active.and_then(|tid| terms.iter().find(|t| t.id == tid)).cloned() else {
        CentralPanel::no_frame().frame(Frame::new().fill(BG)).show(ui, |ui| empty(app, ui, pid));
        return;
    };
    let tid = spec.id;
    let status = app.status(tid);
    Panel::top("dev-toolbar")
        .frame(Frame::new().fill(BG).inner_margin(Margin::symmetric(10, 5)).stroke(Stroke::new(1.0, LINE)))
        .show(ui, |ui| toolbar(app, ui, pid, &spec, &status));

    CentralPanel::no_frame().frame(Frame::new().fill(BG)).show(ui, |ui| {
        let interactive = app.dialog.is_none() && app.dev_connected();
        let has_run = !matches!(status, Status::NotStarted | Status::Failed(_));
        let focus = app.focus_dev && interactive;
        if has_run && let Some(screen) = app.screens.get_mut(&tid) {
            TerminalView::new(screen, Id::unique(("dev-terminal", tid))).request_focus(focus).interactive(interactive).show(ui);
            return;
        }
        ui.vertical_centered(|ui| {
            ui.add_space(ui.available_height() / 3.0);
            if let Status::Failed(msg) = &status {
                ui.label(RichText::new(msg).color(super::BAD));
            } else {
                ui.label(RichText::new("Not running.").color(DIM));
            }
            if !spec.command.trim().is_empty() {
                ui.label(RichText::new(format!("$ {}", spec.command.trim())).monospace().color(FG));
            }
            ui.add_space(8.0);
            if ui.button("Start").clicked() {
                app.push(Action::Start(tid));
            }
        });
    });
}

fn header(app: &mut App, ui: &mut Ui, pid: Uuid, terms: &[TerminalSpec], active: Option<Uuid>) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        ui.add_space(10.0);
        ui.label(RichText::new("DEV").size(11.0).color(ACCENT).extra_letter_spacing(1.0));
        ui.add_space(8.0);
        for t in terms {
            dev_tab(app, ui, pid, t, active == Some(t.id));
        }
        ui.add_space(4.0);
        ui.menu_button(RichText::new("+").size(16.0).color(DIM), |ui| {
            let claude = ui.add_enabled(claude_available(), egui::Button::new("Claude Code"));
            let claude = if claude_available() {
                claude.on_hover_text("Runs `claude` in the project folder")
            } else {
                claude.on_disabled_hover_text("The `claude` command isn't on PATH")
            };
            if claude.clicked() {
                app.push(Action::NewDevTerminal(pid, DevPreset::ClaudeCode));
                ui.close();
            }
            if ui.button("Shell").clicked() {
                app.push(Action::NewDevTerminal(pid, DevPreset::Shell));
                ui.close();
            }
            if ui.button("Custom…").on_hover_text("Choose the command, shell and folder").clicked() {
                app.push(Action::NewDevTerminal(pid, DevPreset::Custom));
                ui.close();
            }
        });

        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            ui.add_space(6.0);
            if ui.small_button("✕").on_hover_text("Hide the dev panel (dev terminals keep running)").clicked() {
                app.push(Action::SetDevEnabled(false));
            }
            let (other, label) = match app.ui.dev_side {
                DevSide::Left => (DevSide::Right, "Move to the right"),
                DevSide::Right => (DevSide::Left, "Move to the left"),
            };
            if ui.small_button("⇄").on_hover_text(label).clicked() {
                app.push(Action::SetDevSide(other));
            }
            if editor_available() && ui.small_button("VS Code").on_hover_text("Open the project folder in VS Code").clicked() {
                app.push(Action::OpenInEditor(pid));
            }
        });
    });
}

fn dev_tab(app: &mut App, ui: &mut Ui, pid: Uuid, t: &TerminalSpec, selected: bool) {
    let font = FontId::proportional(13.0);
    let text_w = ui.ctx().fonts_mut(|f| f.layout_no_wrap(t.name.clone(), font.clone(), FG).size().x);
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(48.0 + text_w + 12.0, 34.0), Sense::click());
    let painter = ui.painter_at(rect);
    if selected {
        painter.rect_filled(rect, CornerRadius::ZERO, BG);
        painter.rect_filled(egui::Rect::from_min_size(rect.min, egui::vec2(rect.width(), 2.0)), CornerRadius::ZERO, ACCENT);
    } else if resp.hovered() {
        painter.rect_filled(rect, CornerRadius::ZERO, BG.gamma_multiply(0.5));
    }
    let status = app.status(t.id);
    dot(&painter, egui::pos2(rect.min.x + 14.0, rect.center().y), status_color(&status));
    let kind = app.kind_of(pid, t.id);
    kind.paint(&painter, egui::pos2(rect.min.x + 32.0, rect.center().y));
    painter.text(egui::pos2(rect.min.x + 46.0, rect.center().y), Align2::LEFT_CENTER, &t.name, font, if selected { FG } else { DIM });
    painter.line_segment([rect.right_top(), rect.right_bottom()], Stroke::new(1.0, LINE));

    let resp = resp.on_hover_text(kind.label());
    if resp.clicked() {
        app.push(Action::SelectDevTab(pid, t.id));
    }
    let tid = t.id;
    resp.context_menu(|ui| {
        if ui.button("Edit…").clicked() {
            app.push(Action::EditTerminal(pid, tid));
            ui.close();
        }
        if ui.button(if status.is_running() { "↻ Restart" } else { "Start" }).clicked() {
            app.push(Action::Start(tid));
            ui.close();
        }
        if status.is_running() && ui.button("Stop").clicked() {
            app.push(Action::Stop(tid));
            ui.close();
        }
        ui.separator();
        if ui.button(RichText::new("Remove…").color(super::BAD)).clicked() {
            app.push(Action::RemoveTerminal(pid, tid));
            ui.close();
        }
    });
}

fn toolbar(app: &mut App, ui: &mut Ui, pid: Uuid, spec: &TerminalSpec, status: &Status) {
    let tid = spec.id;
    ui.horizontal(|ui| {
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if ui.small_button("✎ Edit").clicked() {
                app.push(Action::EditTerminal(pid, tid));
            }
            if status.is_running() {
                if ui.small_button("Stop").clicked() {
                    app.push(Action::Stop(tid));
                }
                if ui.small_button("↻ Restart").clicked() {
                    app.push(Action::Start(tid));
                }
            } else if ui.small_button("Start").clicked() {
                app.push(Action::Start(tid));
            }
            ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                let (r, _) = ui.allocate_exact_size(egui::vec2(10.0, 14.0), Sense::hover());
                dot(ui.painter(), r.center(), status_color(status));
                ui.label(RichText::new(status.label()).size(12.0).color(FG));
                let cmd = spec.command.trim();
                let shown = if cmd.is_empty() { "shell".to_string() } else { format!("$ {cmd}") };
                ui.add(egui::Label::new(RichText::new(shown).monospace().size(12.0).color(DIM)).truncate());
            });
        });
    });
}

fn empty(app: &mut App, ui: &mut Ui, pid: Uuid) {
    let folder = app.ws.project(pid).map(|p| p.root.display().to_string()).unwrap_or_default();
    ui.vertical_centered(|ui| {
        ui.add_space(ui.available_height() / 4.0);
        ui.label(RichText::new("Dev terminals").size(17.0).color(FG));
        ui.add_space(4.0);
        ui.label(RichText::new(format!("Run Claude Code or a shell in {folder}")).color(DIM));
        ui.label(
            RichText::new("They run as you, aren't written to the logs, and keep running when you close the window.")
                .size(12.0)
                .color(DIM),
        );
        ui.add_space(12.0);
        if claude_available() {
            if ui.button("Start Claude Code").clicked() {
                app.push(Action::NewDevTerminal(pid, DevPreset::ClaudeCode));
            }
        } else {
            ui.add_enabled(false, egui::Button::new("Start Claude Code"))
                .on_disabled_hover_text("The `claude` command isn't on PATH");
        }
        if ui.button("Start a shell").clicked() {
            app.push(Action::NewDevTerminal(pid, DevPreset::Shell));
        }
        if editor_available() && ui.button("Open in VS Code").clicked() {
            app.push(Action::OpenInEditor(pid));
        }
    });
}
