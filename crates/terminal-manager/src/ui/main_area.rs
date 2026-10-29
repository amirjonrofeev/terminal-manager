//! Right side: tab bar, toolbar, terminal and status bar for the selected project.

use super::{dot, ACCENT, BAD, BG, DIM, FG, IDLE, LINE, OK, SIDE, WARN};
use crate::app::{Action, App, Connection};
use tm_core::protocol::Status;
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

        let connection = match &app.connection {
            Connection::Connected(_) => None,
            Connection::Connecting => Some("Connecting to the background host…".to_string()),
            Connection::Disconnected(reason) => Some(format!("{reason} Retrying…")),
        };
        if let Some(msg) = connection {
            Panel::top("connection")
                .frame(Frame::new().fill(BAD.gamma_multiply(0.16)).inner_margin(Margin::symmetric(14, 8)))
                .show(ui, |ui| {
                    ui.label(RichText::new(msg).color(FG));
                });
        }

        let Some(project) = app.active_project() else {
            welcome(app, ui);
            return;
        };
        let pid = project.id;
        let tid = app.active_tab(project);

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
        if !app.is_connected() {
            ui.label(RichText::new("Waiting for the background host…").color(DIM));
            return;
        }
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
            app.push(Action::Start(tid));
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
    let state = app.state(tid);
    let shell = tm_core::shell::resolve(&spec.shell, &project.default_shell).program;
    let command = spec.command.trim().to_string();
    let uptime = crate::app::uptime(state.started_at_ms);
    let title = state.title.clone();

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
                    app.push(Action::Start(tid));
                }
            } else {
                let label = if status == Status::NotStarted { "Start" } else { "↻ Restart" };
                if ui.button(label).clicked() {
                    app.push(Action::Start(tid));
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
    let interactive = app.dialog.is_none() && app.is_connected();
    let focus = app.focus_terminal && interactive;
    let status = app.status(tid);
    let has_run = !matches!(status, Status::NotStarted | Status::Failed(_));
    if has_run && let Some(screen) = app.screens.get_mut(&tid) {
        TerminalView::new(screen, Id::unique(("terminal", tid))).request_focus(focus).interactive(interactive).show(ui);
        return;
    }

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
            app.push(Action::Start(tid));
        }
    });
}

fn status_bar(app: &App, ui: &mut Ui, pid: Uuid) {
    let root = app.ws.project(pid).map(|p| p.root.display().to_string()).unwrap_or_default();
    ui.horizontal(|ui| {
        ui.label(RichText::new(root).monospace().size(11.0).color(DIM));
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let (text, hover) = match &app.connection {
                Connection::Connected(info) => {
                    let kind = match info.mode {
                        tm_core::paths::HostMode::Service => "the service",
                        tm_core::paths::HostMode::User => "the background host",
                    };
                    (
                        RichText::new(format!("Running in {kind} · closing this window leaves them running")).color(DIM),
                        format!("Host {} (pid {}), data in {}", info.version, info.pid, info.data_dir),
                    )
                }
                _ => (RichText::new("Not connected").color(BAD), String::new()),
            };
            let label = ui.label(text.monospace().size(11.0));
            if !hover.is_empty() {
                label.on_hover_text(hover);
            }
        });
    });
}
