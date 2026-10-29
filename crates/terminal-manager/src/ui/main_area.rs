//! Right side: tab bar, toolbar, terminal and status bar for the selected project.

use super::{dot, pill, pill_width, ACCENT, BAD, BG, DIM, FG, IDLE, LINE, OK, SIDE, WARN};
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
            Connection::Connecting => Some(("Connecting to the background host…".to_string(), false)),
            Connection::Disconnected(reason) => Some((format!("{reason} Retrying…"), false)),
            Connection::Denied(reason) => Some((reason.clone(), true)),
        };
        if let Some((msg, denied)) = connection {
            Panel::top("connection")
                .frame(Frame::new().fill(BAD.gamma_multiply(0.16)).inner_margin(Margin::symmetric(14, 8)))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(msg).color(FG));
                        if denied && cfg!(windows) {
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                if ui.button("Restart as administrator").clicked() {
                                    if crate::client::restart_elevated() {
                                        ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                                    } else {
                                        app.banner = Some("Could not restart as administrator (the prompt was cancelled or failed).".into());
                                    }
                                }
                            });
                        }
                    });
                });
        }

        if app.search_open {
            super::logs::global_search(app, ui);
            return;
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
    let tabs: Vec<(Uuid, String, Status, (u32, u32))> = project
        .terminals
        .iter()
        .map(|t| {
            let st = app.state(t.id);
            (t.id, t.name.clone(), st.status, (st.unseen_errors, st.unseen_warnings))
        })
        .collect();
    let kinds: Vec<super::kind::Kind> = tabs.iter().map(|(tid, ..)| app.kind_of(pid, *tid)).collect();

    ScrollArea::horizontal().scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden).show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            for ((tid, name, status, unseen), kind) in tabs.into_iter().zip(kinds) {
                tab(app, ui, pid, tid, &name, &status, unseen, kind, active == Some(tid));
            }
            ui.add_space(4.0);
            let plus = ui.add(egui::Button::new(RichText::new("+").size(16.0).color(DIM)).frame(false).min_size(egui::vec2(36.0, 36.0)));
            if plus.on_hover_text("New terminal (Ctrl+Shift+T)").clicked() {
                app.push(Action::NewTerminal(pid));
            }
        });
    });
}

#[allow(clippy::too_many_arguments)]
fn tab(
    app: &mut App,
    ui: &mut Ui,
    pid: Uuid,
    tid: Uuid,
    name: &str,
    status: &Status,
    unseen: (u32, u32),
    kind: super::kind::Kind,
    selected: bool,
) {
    let font = FontId::proportional(13.0);
    let code = match status {
        Status::Exited(Some(c)) if *c != 0 => Some(c.to_string()),
        Status::Failed(_) => Some("!".into()),
        _ => None,
    };
    let text_w = ui.ctx().fonts_mut(|f| f.layout_no_wrap(name.to_string(), font.clone(), FG).size().x);
    let code_w = code.as_ref().map_or(0.0, |c| c.len() as f32 * 7.0 + 8.0);
    // Badge: unseen errors (red) or, failing that, unseen warnings (amber).
    let badge = match unseen {
        (e, _) if e > 0 => Some((badge_text(e), BAD)),
        (_, w) if w > 0 => Some((badge_text(w), WARN)),
        _ => None,
    };
    let badge_w = badge.as_ref().map_or(0.0, |(t, _)| pill_width(t) + 8.0);
    // Layout: status dot, project-type badge, name, exit code, unseen badge.
    const TEXT_X: f32 = 50.0;
    let width = TEXT_X + text_w + code_w + badge_w + 14.0;
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(width, 36.0), Sense::click());
    let painter = ui.painter_at(rect);

    if selected {
        painter.rect_filled(rect, CornerRadius::ZERO, BG);
        painter.rect_filled(egui::Rect::from_min_size(rect.min, egui::vec2(rect.width(), 2.0)), CornerRadius::ZERO, ACCENT);
    } else if resp.hovered() {
        painter.rect_filled(rect, CornerRadius::ZERO, BG.gamma_multiply(0.5));
    }
    painter.line_segment([rect.right_top(), rect.right_bottom()], Stroke::new(1.0, LINE));
    dot(&painter, egui::pos2(rect.min.x + 16.0, rect.center().y), status_color(status));
    kind.paint(&painter, egui::pos2(rect.min.x + 34.0, rect.center().y));
    painter.text(
        egui::pos2(rect.min.x + TEXT_X, rect.center().y),
        Align2::LEFT_CENTER,
        name,
        font,
        if selected { FG } else { DIM },
    );
    if let Some(code) = code {
        painter.text(
            egui::pos2(rect.min.x + TEXT_X + text_w + 8.0, rect.center().y),
            Align2::LEFT_CENTER,
            code,
            FontId::monospace(11.0),
            BAD,
        );
    }
    if let Some((text, color)) = &badge {
        let x = rect.min.x + TEXT_X + text_w + code_w + 8.0;
        pill(&painter, egui::pos2(x, rect.center().y), text, *color);
    }

    let resp = resp.on_hover_text(kind.label());
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
            // Live screen or saved logs.
            let logs = app.log_mode.contains(&tid);
            if ui.selectable_label(logs, "Logs").on_hover_text("Saved output with timestamps; search and filter").clicked() && !logs {
                app.push(Action::ShowLogs(tid, true));
            }
            if ui.selectable_label(!logs, "Live").clicked() && logs {
                app.push(Action::ShowLogs(tid, false));
            }
            if !logs && ui.button("Find").on_hover_text("Find in this terminal's output (Ctrl+F)").clicked() {
                app.push(Action::OpenFind(tid));
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
    if app.log_mode.contains(&tid) {
        super::logs::terminal_logs(app, ui, tid);
        return;
    }
    let interactive = app.dialog.is_none() && app.is_connected();
    let focus = app.focus_terminal && interactive;
    if interactive && ui.ctx().input(|i| i.focused) {
        app.mark_seen(tid);
    }
    let status = app.status(tid);
    let has_run = !matches!(status, Status::NotStarted | Status::Failed(_));
    if has_run && app.find.as_ref().is_some_and(|f| f.terminal == tid) {
        find_bar(app, ui, tid);
    }
    let focus = focus && app.find.as_ref().is_none_or(|f| f.terminal != tid);
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

fn badge_text(n: u32) -> String {
    if n > 999 { "999+".into() } else { n.to_string() }
}

/// Ctrl+F bar over the live terminal: Enter = next (newer), Shift+Enter = previous, Esc = close.
fn find_bar(app: &mut App, ui: &mut Ui, tid: Uuid) {
    let (Some(bar), Some(screen)) = (app.find.as_mut(), app.screens.get_mut(&tid)) else { return };
    let mut close = false;
    let mut step: Option<bool> = None;
    Panel::top("find-bar")
        .frame(Frame::new().fill(SIDE).inner_margin(Margin::symmetric(12, 6)).stroke(Stroke::new(1.0, LINE)))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                let edit = ui.add(
                    egui::TextEdit::singleline(&mut bar.query)
                        .id(Id::unique(("find-text", tid)))
                        .hint_text("Find in output")
                        .font(egui::TextStyle::Monospace)
                        .desired_width(260.0),
                );
                if bar.focus {
                    edit.request_focus();
                    bar.focus = false;
                }
                let (enter, shift, esc) = ui.input(|i| (i.key_pressed(egui::Key::Enter), i.modifiers.shift, i.key_pressed(egui::Key::Escape)));
                if edit.lost_focus() && enter {
                    step = Some(!shift);
                    edit.request_focus();
                }
                if esc && (edit.has_focus() || edit.lost_focus()) {
                    close = true;
                }
                if ui.selectable_label(bar.case_sensitive, "Aa").on_hover_text("Match case").clicked() {
                    bar.case_sensitive = !bar.case_sensitive;
                }
                screen.find_set(&bar.query, bar.case_sensitive);
                screen.find_refresh(false);
                let n = screen.find.count();
                let label = match (n, screen.find.current) {
                    (_, _) if bar.query.is_empty() => String::new(),
                    (0, _) => "No matches".into(),
                    (n, Some(i)) => format!("{} of {n}", i + 1),
                    (n, None) => format!("{n} matches"),
                };
                ui.label(RichText::new(label).size(12.0).color(if n == 0 && !bar.query.is_empty() { BAD } else { DIM }));
                if ui.button("↑").on_hover_text("Previous (Shift+Enter)").clicked() {
                    step = Some(false);
                }
                if ui.button("↓").on_hover_text("Next (Enter)").clicked() {
                    step = Some(true);
                }
                ui.label(RichText::new("Scrollback only covers recent output; use Logs for older lines.").size(11.0).color(DIM));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui.button("Close").on_hover_text("Esc").clicked() {
                        close = true;
                    }
                });
            });
        });
    if let Some(forward) = step {
        screen.find_step(forward);
    }
    if close {
        screen.find_clear();
        app.find = None;
        app.focus_terminal = true;
    }
}
