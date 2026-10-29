//! Left panel: the project list.

use super::{dot, BG, DIM, FG, IDLE, LINE, OK, ROW_SELECTED, SIDE, WARN, BAD};
use crate::app::{Action, App, Rollup};
use egui::{Align, Align2, Color32, CornerRadius, FontId, Frame, Layout, Margin, Panel, RichText, ScrollArea, Sense, Stroke, Ui};

pub fn rollup_color(r: Rollup) -> Color32 {
    match r {
        Rollup::Ok => OK,
        Rollup::Warn => WARN,
        Rollup::Bad => BAD,
        Rollup::Idle => IDLE,
    }
}

pub fn show(app: &mut App, ui: &mut Ui) {
    Panel::left("sidebar")
        .resizable(true)
        .default_size(240.0)
        .size_range(180.0..=420.0)
        .frame(Frame::new().fill(SIDE).stroke(Stroke::new(1.0, LINE)))
        .show(ui, |ui| {
            Panel::bottom("sidebar-footer")
                .frame(Frame::new().fill(SIDE).inner_margin(Margin::symmetric(12, 10)))
                .show(ui, |ui| footer(app, ui));

            ui.add_space(10.0);
            ui.horizontal(|ui| {
                ui.add_space(14.0);
                ui.label(RichText::new("PROJECTS").size(11.0).color(DIM).extra_letter_spacing(1.0));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.add_space(10.0);
                    if ui.button("+ Project").on_hover_text("Add a project folder (Ctrl+Shift+N)").clicked() {
                        app.push(Action::NewProject);
                    }
                });
            });
            ui.add_space(6.0);

            ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 2.0;
                let (open, closed): (Vec<_>, Vec<_>) = app.ws.projects.iter().map(|p| p.id).partition(|id| {
                    app.ws.project(*id).is_some_and(|p| p.open)
                });
                if app.ws.projects.is_empty() {
                    ui.add_space(8.0);
                    ui.horizontal_wrapped(|ui| {
                        ui.add_space(14.0);
                        ui.label(RichText::new("No projects yet. Add a folder to get started.").color(DIM));
                    });
                }
                for id in open {
                    project_row(app, ui, id);
                }
                if !closed.is_empty() {
                    ui.add_space(12.0);
                    ui.horizontal(|ui| {
                        ui.add_space(14.0);
                        ui.label(RichText::new("CLOSED").size(11.0).color(DIM).extra_letter_spacing(1.0));
                    });
                    ui.add_space(4.0);
                    for id in closed {
                        project_row(app, ui, id);
                    }
                }
            });
        });
}

fn project_row(app: &mut App, ui: &mut Ui, id: uuid::Uuid) {
    let Some(project) = app.ws.project(id) else { return };
    let selected = app.ui.active_project == Some(id) && project.open;
    let open = project.open;
    let running = app.running_count(project);
    let total = project.terminals.len();
    let color = rollup_color(app.rollup(project));
    let name = project.name.clone();
    let path = project.root.display().to_string();

    let width = ui.available_width();
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(width, 44.0), Sense::click());
    let row = rect.shrink2(egui::vec2(8.0, 0.0));
    let painter = ui.painter_at(row);
    if selected {
        painter.rect_filled(row, CornerRadius::same(7), ROW_SELECTED);
    } else if resp.hovered() {
        painter.rect_filled(row, CornerRadius::same(7), BG.gamma_multiply(0.6));
    }
    let text_color = if open { FG } else { DIM };
    dot(&painter, egui::pos2(row.min.x + 14.0, row.center().y), if open { color } else { IDLE.gamma_multiply(0.6) });
    painter.text(egui::pos2(row.min.x + 28.0, row.min.y + 7.0), Align2::LEFT_TOP, &name, FontId::proportional(14.0), text_color);
    let count = format!("{running}/{total}");
    painter.text(egui::pos2(row.max.x - 10.0, row.min.y + 9.0), Align2::RIGHT_TOP, count, FontId::monospace(11.0), DIM);
    // Path, clipped to the row so long paths don't spill out.
    painter.text(
        egui::pos2(row.min.x + 28.0, row.min.y + 25.0),
        Align2::LEFT_TOP,
        &path,
        FontId::monospace(11.0),
        DIM,
    );

    let resp = resp.on_hover_text(&path);
    if resp.clicked() {
        app.push(if open { Action::SelectProject(id) } else { Action::OpenProject(id) });
    }
    resp.context_menu(|ui| {
        if ui.button("Edit project…").clicked() {
            app.push(Action::EditProject(id));
            ui.close();
        }
        if open {
            if ui.button("New terminal…").clicked() {
                app.push(Action::NewTerminal(id));
                ui.close();
            }
            ui.separator();
            if ui.button("Start all").clicked() {
                app.push(Action::StartAll(id));
                ui.close();
            }
            if ui.button("Stop all").clicked() {
                app.push(Action::StopAll(id));
                ui.close();
            }
            ui.separator();
            if ui.button("Close project").clicked() {
                app.push(Action::CloseProject(id));
                ui.close();
            }
        } else if ui.button("Open project").clicked() {
            app.push(Action::OpenProject(id));
            ui.close();
        }
        if ui.button(RichText::new("Remove…").color(BAD)).clicked() {
            app.push(Action::RemoveProject(id));
            ui.close();
        }
    });
}

fn footer(app: &mut App, ui: &mut Ui) {
    let Some(pid) = app.active_project().map(|p| p.id) else { return };
    ui.horizontal(|ui| {
        if ui.button("Start all").on_hover_text("Start every terminal in this project").clicked() {
            app.push(Action::StartAll(pid));
        }
        if ui.button("Stop all").clicked() {
            app.push(Action::StopAll(pid));
        }
    });
}
