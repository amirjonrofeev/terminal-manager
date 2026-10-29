//! Right side: tab bar, toolbar, terminal and status bar for the selected project.

use super::{dot, ACCENT, BAD, BG, DIM, FG, IDLE, LINE, OK, SIDE, WARN};
use crate::app::{Action, App};
use crate::session::Status;
use crate::term::TerminalView;
use egui::{Align, Align2, CentralPanel, CornerRadius, FontId, Frame, Id, Layout, Margin, Panel, RichText, ScrollArea, Sense, Stroke, Ui};
use uuid::Uuid;

pub fn status_color(s: &Status) -> egui::Color32 {
    match s {
        Status::Running => OK,
        s if s.is_error() => BAD,
        _ => IDLE,
    }
}

pub fn show(app: &mut App, ui: &mut Ui) {
    CentralPanel::no_frame().frame(Frame::new().fill(BG)).show(ui, |ui| {
        if let Some(msg) = app.banner.clone() {
            Panel::top("banner")
                .frame(Frame::new().fill(WARN.gamma_multiply(0.18)).inner_margin(Margin::symmetric(14, 8)))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(msg).color(WARN));
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if ui.button("Dismiss").clicked() {
                                app.push(Action::DismissBanner);
                            }
                        });
                    });
                });
        }

        let Some(project) = app.active_project() else {
            welcome(app, ui);
            return;
        };
        let pid = project.id;
        let tid = App::active_tab(project);

        Panel::top("tabs").frame(Frame::new().fill(SIDE)).show(ui, |ui| tab_bar(app, ui, pid, tid));
        Panel::bottom("statusbar")
            .frame(Frame::new().fill(SIDE).inner_margin(Margin::symmetric(14, 4)).stroke(Stroke::new(1.0, LINE)))
            .show(ui, |ui| status_bar(app, ui, pid));

        let Some(tid) = tid else {
            CentralPanel::no_frame().show(ui, |ui| {
                ui.vertical_centered(|ui| {
                    ui.add_space(ui.available_height() / 3.0);
                    ui.label(RichText::new("This project has no terminals yet.").color(DIM));
                    ui.add_space(6.0);
                    if ui.button("+ New terminal").clicked() {
                        app.push(Action::NewTerminal(pid));
                    }
                });
            });
            return;
        };

        Panel::top("toolbar")
            .frame(Frame::new().fill(BG).inner_margin(Margin::symmetric(14, 6)).stroke(Stroke::new(1.0, LINE)))
            .show(ui, |ui| toolbar(app, ui, pid, tid));

        CentralPanel::no_frame().frame(Frame::new().fill(BG)).show(ui, |ui| terminal_area(app, ui, pid, tid));
    });
}

fn welcome(app: &mut App, ui: &mut Ui) {
    ui.vertical_centered(|ui| {
        ui.add_space(ui.available_height() / 4.0);
        ui.label(RichText::new("Terminal Manager").size(26.0).color(FG));
        ui.add_space(6.0);
        ui.label(
            RichText::new("Add a project folder, give it the terminals you run there, and they come back after every restart.")
                .color(DIM),
        );
        ui.add_space(14.0);
        if ui.button(RichText::new("+ Add project").size(15.0)).clicked() {
            app.push(Action::NewProject);
        }
    });
}

fn tab_bar(app: &mut App, ui: &mut Ui, pid: Uuid, active: Option<Uuid>) {
    let Some(project) = app.ws.project(pid) else { return };
    let tabs: Vec<(Uuid, String, Status)> =
        project.terminals.iter().map(|t| (t.id, t.name.clone(), app.status(t.id))).collect();

    ScrollArea::horizontal().scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden).show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            for (tid, name, status) in tabs {
                tab(app, ui, pid, tid, &name, &status, active == Some(tid));
            }
            ui.add_space(4.0);
            let plus = ui.add(egui::Button::new(RichText::new("+").size(16.0).color(DIM)).frame(false).min_size(egui::vec2(36.0, 36.0)));
            if plus.on_hover_text("New terminal (Ctrl+Shift+T)").clicked() {
                app.push(Action::NewTerminal(pid));
            }
        });
    });
}

fn tab(app: &mut App, ui: &mut Ui, pid: Uuid, tid: Uuid, name: &str, status: &Status, selected: bool) {
    let font = FontId::proportional(13.0);
    let code = match status {
        Status::Exited(Some(c)) if *c != 0 => Some(c.to_string()),
        Status::Failed(_) => Some("!".into()),
        _ => None,
    };
    let text_w = ui.ctx().fonts_mut(|f| f.layout_no_wrap(name.to_string(), font.clone(), FG).size().x);
    let code_w = code.as_ref().map_or(0.0, |c| c.len() as f32 * 7.0 + 8.0);
    let width = 14.0 + 10.0 + 8.0 + text_w + code_w + 14.0;
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(width, 36.0), Sense::click());
    let painter = ui.painter_at(rect);

    if selected {
        painter.rect_filled(rect, CornerRadius::ZERO, BG);
        painter.rect_filled(egui::Rect::from_min_size(rect.min, egui::vec2(rect.width(), 2.0)), CornerRadius::ZERO, ACCENT);
    } else if resp.hovered() {
        painter.rect_filled(rect, CornerRadius::ZERO, BG.gamma_multiply(0.5));
    }
    painter.line_segment([rect.right_top(), rect.right_bottom()], Stroke::new(1.0, LINE));
    dot(&painter, egui::pos2(rect.min.x + 18.0, rect.center().y), status_color(status));
    painter.text(
        egui::pos2(rect.min.x + 30.0, rect.center().y),
        Align2::LEFT_CENTER,
        name,
        font,
        if selected { FG } else { DIM },
    );
    if let Some(code) = code {
        painter.text(
            egui::pos2(rect.min.x + 30.0 + text_w + 8.0, rect.center().y),
            Align2::LEFT_CENTER,
            code,
            FontId::monospace(11.0),
            BAD,
        );
    }

    if resp.clicked() {
        app.push(Action::SelectTab(pid, tid));
    }
    if resp.double_clicked() {
        app.push(Action::EditTerminal(pid, tid));
    }
    let running = status.is_running();
    resp.context_menu(|ui| {
        if ui.button("Edit…").clicked() {
            app.push(Action::EditTerminal(pid, tid));
            ui.close();
        }
        if ui.button(if running { "↻ Restart" } else { "Start" }).clicked() {
            app.push(Action::Start(pid, tid));
            ui.close();
        }
        if running && ui.button("Stop").clicked() {
            app.push(Action::Stop(tid));
            ui.close();
        }
        if ui.button("Duplicate").clicked() {
            app.push(Action::DuplicateTerminal(pid, tid));
            ui.close();
        }
        ui.separator();
        if ui.button(RichText::new("Remove…").color(BAD)).clicked() {
            app.push(Action::RemoveTerminal(pid, tid));
            ui.close();
        }
    });
}

fn toolbar(app: &mut App, ui: &mut Ui, pid: Uuid, tid: Uuid) {
    let Some(project) = app.ws.project(pid) else { return };
    let Some(spec) = project.terminal(tid) else { return };
    let status = app.status(tid);
    let session = app.sessions.get(&tid);
    let shell = crate::shell::resolve(&spec.shell, &project.default_shell).program;
    let command = spec.command.trim().to_string();
    let uptime = session.and_then(|s| s.uptime());
    let title = session.and_then(|s| s.title.clone());

    // Buttons are laid out first from the right, so the info on the left truncates instead of running under them.
    ui.horizontal(|ui| {
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if ui.button("✎ Edit").clicked() {
                app.push(Action::EditTerminal(pid, tid));
            }
            if status.is_running() {
                if ui.button("Stop").clicked() {
                    app.push(Action::Stop(tid));
                }
                if ui.button("↻ Restart").on_hover_text("Ctrl+Shift+R").clicked() {
                    app.push(Action::Start(pid, tid));
                }
            } else {
                let label = if status == Status::NotStarted { "Start" } else { "↻ Restart" };
                if ui.button(label).clicked() {
                    app.push(Action::Start(pid, tid));
                }
            }
            ui.add_space(12.0);
            ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                let (r, _) = ui.allocate_exact_size(egui::vec2(10.0, 16.0), Sense::hover());
                dot(ui.painter(), r.center(), status_color(&status));
                let label = ui.label(RichText::new(status.label()).color(FG));
                if let Some(t) = &title {
                    label.on_hover_text(format!("Title: {t}"));
                }
                if let Some(up) = uptime {
                    ui.label(RichText::new(up).monospace().size(12.0).color(DIM)).on_hover_text("Uptime");
                }
                ui.label(RichText::new(shell).monospace().size(12.0).color(DIM));
                if !command.is_empty() {
                    ui.add(egui::Label::new(RichText::new(format!("$ {command}")).monospace().size(12.0).color(DIM)).truncate())
                        .on_hover_text(&command);
                }
            });
        });
    });
}

fn terminal_area(app: &mut App, ui: &mut Ui, pid: Uuid, tid: Uuid) {
    let interactive = app.dialog.is_none();
    let focus = app.focus_terminal && interactive;
    if let Some(backend) = app.sessions.get_mut(&tid).and_then(|s| s.backend.as_mut()) {
        TerminalView::new(backend, Id::unique(("terminal", tid))).request_focus(focus).interactive(interactive).show(ui);
        return;
    }

    let status = app.status(tid);
    let Some(spec) = app.ws.project(pid).and_then(|p| p.terminal(tid)) else { return };
    let command = spec.command.trim().to_string();
    let auto = spec.auto_start;
    ui.vertical_centered(|ui| {
        ui.add_space(ui.available_height() / 3.0);
        match &status {
            Status::Failed(msg) => {
                ui.label(RichText::new(msg).color(BAD));
            }
            _ if !auto => {
                ui.label(RichText::new("Auto-start is off for this terminal.").color(DIM));
            }
            _ => {
                ui.label(RichText::new("Not started.").color(DIM));
            }
        }
        if !command.is_empty() {
            ui.add_space(4.0);
            ui.label(RichText::new(format!("$ {command}")).monospace().color(FG));
        }
        ui.add_space(10.0);
        if ui.button("Start").clicked() {
            app.push(Action::Start(pid, tid));
        }
    });
}

fn status_bar(app: &App, ui: &mut Ui, pid: Uuid) {
    let root = app.ws.project(pid).map(|p| p.root.display().to_string()).unwrap_or_default();
    ui.horizontal(|ui| {
        ui.label(RichText::new(root).monospace().size(11.0).color(DIM));
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let text = match &app.save_error {
                Some(e) => RichText::new(format!("Save failed: {e}")).color(BAD),
                None => RichText::new(format!("Saved · {}", short_ago(app.last_save().elapsed().as_secs()))).color(DIM),
            };
            ui.label(text.monospace().size(11.0)).on_hover_text(app.store_path().display().to_string());
        });
    });
}

fn short_ago(secs: u64) -> String {
    match secs {
        0..5 => "just now".into(),
        5..60 => format!("{secs}s ago"),
        60..3600 => format!("{}m ago", secs / 60),
        _ => format!("{}h ago", secs / 3600),
    }
}
