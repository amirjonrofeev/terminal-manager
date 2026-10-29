//! The egui widget: paints the grid and turns input into PTY writes.

use super::keys::key_bytes;
use super::theme;
use super::Screen;
use alacritty_terminal::selection::SelectionType;
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::vte::ansi::CursorShape;
use egui::{Align2, Color32, CornerRadius, Event, EventFilter, FontId, Id, Pos2, Rect, Response, Sense, Stroke, Ui, Vec2};

const PADDING: f32 = 6.0;

pub struct TerminalView<'a> {
    backend: &'a mut Screen,
    id: Id,
    font: FontId,
    request_focus: bool,
    interactive: bool,
}

#[derive(Clone, Default)]
struct ViewState {
    scroll_acc: f32,
    skip_text: Option<String>,
}

impl<'a> TerminalView<'a> {
    pub fn new(backend: &'a mut Screen, id: Id) -> Self {
        Self { backend, id, font: FontId::monospace(14.0), request_focus: false, interactive: true }
    }

    pub fn request_focus(mut self, focus: bool) -> Self {
        self.request_focus = focus;
        self
    }

    /// When false (e.g. a dialog is open) the view still paints but ignores input.
    pub fn interactive(mut self, interactive: bool) -> Self {
        self.interactive = interactive;
        self
    }

    pub fn show(self, ui: &mut Ui) -> Response {
        let (rect, _) = ui.allocate_exact_size(ui.available_size(), Sense::hover());
        let resp = ui.interact(rect, self.id, Sense::click_and_drag());
        let (cell_w, cell_h) = ui.ctx().fonts_mut(|f| (f.glyph_width(&self.font, 'M'), f.row_height(&self.font)));
        let grid_rect = rect.shrink(PADDING);
        self.backend.resize(grid_rect.width(), grid_rect.height(), cell_w, cell_h);

        if self.interactive && (self.request_focus || resp.clicked() || resp.drag_started()) {
            resp.request_focus();
        }
        if resp.has_focus() {
            ui.memory_mut(|m| {
                m.set_focus_lock_filter(
                    self.id,
                    EventFilter { tab: true, horizontal_arrows: true, vertical_arrows: true, escape: true },
                )
            });
        }

        let mut state: ViewState = ui.data(|d| d.get_temp(self.id)).unwrap_or_default();
        if self.interactive {
            self.handle_mouse(ui, &resp, grid_rect, cell_h, &mut state);
        }
        if self.interactive && resp.has_focus() {
            self.handle_keys(ui, &mut state);
        }
        ui.data_mut(|d| d.insert_temp(self.id, state));

        self.paint(ui, rect, grid_rect.min, cell_w, cell_h, resp.has_focus());
        if resp.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Text);
        }
        resp
    }

    fn handle_mouse(&self, ui: &Ui, resp: &Response, grid: Rect, cell_h: f32, state: &mut ViewState) {
        if resp.hovered() {
            let dy: f32 = ui.input(|i| {
                i.events
                    .iter()
                    .map(|e| match e {
                        Event::MouseWheel { unit: egui::MouseWheelUnit::Line, delta, .. } => delta.y * cell_h,
                        Event::MouseWheel { unit: egui::MouseWheelUnit::Page, delta, .. } => delta.y * cell_h * 20.0,
                        Event::MouseWheel { delta, .. } => delta.y,
                        _ => 0.0,
                    })
                    .sum()
            });
            if dy != 0.0 {
                state.scroll_acc += dy;
                let lines = (state.scroll_acc / cell_h).trunc();
                state.scroll_acc -= lines * cell_h;
                if lines != 0.0 {
                    self.backend.scroll(lines as i32);
                }
            }
        }
        let rel = |p: Pos2| (p.x - grid.min.x, p.y - grid.min.y);
        if resp.triple_clicked() || resp.double_clicked() {
            if let Some(p) = resp.interact_pointer_pos() {
                let ty = if resp.triple_clicked() { SelectionType::Lines } else { SelectionType::Semantic };
                let (x, y) = rel(p);
                self.backend.start_selection(ty, x, y);
                self.backend.update_selection(x, y);
            }
        } else if resp.drag_started() {
            if let Some(p) = resp.interact_pointer_pos() {
                let (x, y) = rel(p);
                self.backend.start_selection(SelectionType::Simple, x, y);
            }
        } else if resp.dragged() {
            if let Some(p) = resp.interact_pointer_pos() {
                let (x, y) = rel(p);
                self.backend.update_selection(x, y);
            }
        } else if resp.clicked() {
            self.backend.clear_selection();
        }
        if resp.secondary_clicked() {
            // Right click copies the selection, or pastes when there is none (like Windows Terminal).
            match self.backend.selection_text() {
                Some(text) => {
                    ui.ctx().copy_text(text);
                    self.backend.clear_selection();
                }
                None => ui.ctx().send_viewport_cmd(egui::ViewportCommand::RequestPaste),
            }
        }
    }

    fn handle_keys(&self, ui: &Ui, state: &mut ViewState) {
        let events = ui.input(|i| i.events.clone());
        let mode = self.backend.mode();
        let mut typed = false;
        for event in events {
            match event {
                Event::Text(text) => {
                    if state.skip_text.take().as_deref() == Some(text.as_str()) || text.is_empty() {
                        continue;
                    }
                    self.backend.write(text.into_bytes());
                    typed = true;
                }
                Event::Key { key, pressed: true, modifiers, .. } => {
                    use egui::Key;
                    if modifiers.ctrl && modifiers.shift && key == Key::C {
                        if let Some(text) = self.backend.selection_text() {
                            ui.ctx().copy_text(text);
                        }
                        continue;
                    }
                    if modifiers.ctrl && modifiers.shift && key == Key::V {
                        ui.ctx().send_viewport_cmd(egui::ViewportCommand::RequestPaste);
                        continue;
                    }
                    if let Some(bytes) = key_bytes(key, modifiers, mode) {
                        if modifiers.alt && !modifiers.ctrl && bytes.len() == 2 && bytes[0] == 0x1b {
                            state.skip_text = Some((bytes[1] as char).to_string());
                        }
                        self.backend.write(bytes);
                        typed = true;
                    }
                }
                Event::Copy => match self.backend.selection_text() {
                    Some(text) => {
                        ui.ctx().copy_text(text);
                        self.backend.clear_selection();
                    }
                    None => {
                        self.backend.write(&b"\x03"[..]);
                        typed = true;
                    }
                },
                Event::Cut => {
                    self.backend.write(&b"\x18"[..]);
                    typed = true;
                }
                Event::Paste(text) => {
                    self.backend.paste(&text);
                    self.backend.clear_selection();
                }
                _ => {}
            }
        }
        if typed {
            self.backend.clear_selection();
            self.backend.scroll_to_bottom();
        }
    }

    fn paint(&self, ui: &Ui, rect: Rect, origin: Pos2, cw: f32, ch: f32, focused: bool) {
        let painter = ui.painter_at(rect);
        let finds = self.backend.find_visible();
        let term = self.backend.term.borrow();
        let content = term.renderable_content();
        let colors = content.colors;
        let default_bg = theme::background(colors);
        let default_fg = theme::foreground(colors);
        painter.rect_filled(rect, CornerRadius::ZERO, default_bg);

        let offset = content.display_offset as i32;
        let selection = content.selection;
        let cursor = content.cursor;
        let cursor_line = cursor.point.line.0 + offset;
        let block_cursor = focused && cursor.shape == CursorShape::Block;

        let mut texts: Vec<(Pos2, String, Color32)> = Vec::new();
        let mut run: Option<(Pos2, String, Color32, i32, usize)> = None; // pos, text, fg, line, next col
        let mut lines: Vec<(Pos2, Pos2, Color32)> = Vec::new();

        for indexed in content.display_iter {
            let cell = &indexed.cell;
            let flags = cell.flags;
            if flags.contains(Flags::WIDE_CHAR_SPACER) {
                continue;
            }
            let line = indexed.point.line.0 + offset;
            let col = indexed.point.column.0;
            let width = if flags.contains(Flags::WIDE_CHAR) { 2.0 } else { 1.0 };
            let pos = Pos2::new(origin.x + col as f32 * cw, origin.y + line as f32 * ch);

            let mut fg = theme::resolve(cell.fg, colors);
            let mut bg = theme::resolve(cell.bg, colors);
            if flags.contains(Flags::INVERSE) {
                std::mem::swap(&mut fg, &mut bg);
            }
            if selection.is_some_and(|s| s.contains(indexed.point)) {
                bg = theme::selection();
            }
            if let Some((_, current)) = finds.iter().find(|(m, _)| m.contains(&indexed.point)) {
                bg = if *current { theme::find_current() } else { theme::find() };
                if *current {
                    fg = default_bg;
                }
            }
            // Like Windows Terminal: text a program colored almost like its background (white on
            // a light theme, black on a dark one) is drawn in a readable color instead.
            if theme::indistinguishable(fg, bg) && !flags.contains(Flags::HIDDEN) {
                fg = theme::readable_on(bg, default_fg);
            }
            // Dim text moves part of the way toward the background (opaque, so it stays readable
            // on light themes too; fading the alpha turned dim white into nothing there).
            if flags.intersects(Flags::DIM | Flags::DIM_BOLD) {
                fg = theme::mix(fg, bg, 0.35);
            }
            if flags.contains(Flags::HIDDEN) {
                fg = bg;
            }
            let is_cursor = cursor.shape != CursorShape::Hidden && indexed.point == cursor.point;
            if is_cursor && block_cursor {
                bg = theme::resolve(alacritty_terminal::vte::ansi::Color::Named(
                    alacritty_terminal::vte::ansi::NamedColor::Cursor,
                ), colors);
                fg = default_bg;
            }

            if bg != default_bg {
                painter.rect_filled(
                    Rect::from_min_size(pos, Vec2::new(cw * width + 0.5, ch + 0.5)),
                    CornerRadius::ZERO,
                    bg,
                );
            }
            if flags.intersects(Flags::ALL_UNDERLINES) {
                let y = pos.y + ch - 1.0;
                lines.push((Pos2::new(pos.x, y), Pos2::new(pos.x + cw * width, y), fg));
            }
            if flags.contains(Flags::STRIKEOUT) {
                let y = pos.y + ch / 2.0;
                lines.push((Pos2::new(pos.x, y), Pos2::new(pos.x + cw * width, y), fg));
            }

            let c = cell.c;
            if c == ' ' || c == '\t' || c == '\0' {
                if let Some(r) = run.take() {
                    texts.push((r.0, r.1, r.2));
                }
                continue;
            }
            // ASCII runs share one text shape; anything else gets its own cell so fallback glyph widths don't shift the row.
            let simple = c.is_ascii() && width == 1.0 && !is_cursor;
            if simple {
                if let Some(r) = run.as_mut().filter(|r| r.2 == fg && r.3 == line && r.4 == col) {
                    r.1.push(c);
                    r.4 += 1;
                    continue;
                }
                if let Some(r) = run.take() {
                    texts.push((r.0, r.1, r.2));
                }
                run = Some((pos, c.to_string(), fg, line, col + 1));
            } else {
                if let Some(r) = run.take() {
                    texts.push((r.0, r.1, r.2));
                }
                let mut s = c.to_string();
                if let Some(extra) = cell.zerowidth() {
                    s.extend(extra);
                }
                let center = Pos2::new(pos.x + cw * width / 2.0, pos.y);
                painter.text(center, Align2::CENTER_TOP, s, self.font.clone(), fg);
            }
        }
        if let Some(r) = run.take() {
            texts.push((r.0, r.1, r.2));
        }

        // Cursor outline / beam / underline (block-and-focused was drawn as the cell background).
        if cursor.shape != CursorShape::Hidden && !block_cursor && cursor_line >= 0 {
            let pos = Pos2::new(origin.x + cursor.point.column.0 as f32 * cw, origin.y + cursor_line as f32 * ch);
            let color = theme::resolve(
                alacritty_terminal::vte::ansi::Color::Named(alacritty_terminal::vte::ansi::NamedColor::Cursor),
                colors,
            );
            let cell = Rect::from_min_size(pos, Vec2::new(cw, ch));
            match cursor.shape {
                CursorShape::Beam => {
                    painter.rect_filled(Rect::from_min_size(pos, Vec2::new(2.0, ch)), CornerRadius::ZERO, color);
                }
                CursorShape::Underline => {
                    painter.rect_filled(
                        Rect::from_min_size(Pos2::new(pos.x, pos.y + ch - 2.0), Vec2::new(cw, 2.0)),
                        CornerRadius::ZERO,
                        color,
                    );
                }
                _ => {
                    painter.rect_stroke(cell.shrink(0.5), CornerRadius::ZERO, Stroke::new(1.0, color), egui::StrokeKind::Inside);
                }
            }
        }

        for (pos, text, color) in texts {
            painter.text(pos, Align2::LEFT_TOP, text, self.font.clone(), color);
        }
        for (a, b, color) in lines {
            painter.line_segment([a, b], Stroke::new(1.0, color));
        }
    }
}
