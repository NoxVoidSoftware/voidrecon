//! vt100 to ratatui widget for voidrecon
//! Maps vt100::Screen::cells to ratatui::buffer::Cell, preserving fg/bg colors and modifiers.

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
    widgets::Widget,
};
use std::sync::{Arc, Mutex};
use vt100::Parser;

/// Widget for rendering vt100 screen in ratatui
pub struct VtScreenWidget<'a> {
    pub parser: &'a Arc<Mutex<Parser>>,
}

impl<'a> Widget for VtScreenWidget<'a> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if let Ok(parser) = self.parser.lock() {
            let screen = parser.screen();
            let (rows, cols) = screen.size();
            for row in 0..rows.min(area.height) {
                for col in 0..cols.min(area.width) {
                    if let Some(cell) = screen.cell(row, col) {
                        let x = area.left() + col;
                        let y = area.top() + row;
                        if x < area.right() && y < area.bottom() {
                            let mut style = Style::default();

                            match cell.fgcolor() {
                                vt100::Color::Default => {}
                                vt100::Color::Idx(i) => style = style.fg(Color::Indexed(i)),
                                vt100::Color::Rgb(r, g, b) => style = style.fg(Color::Rgb(r, g, b)),
                            }

                            match cell.bgcolor() {
                                vt100::Color::Default => {}
                                vt100::Color::Idx(i) => style = style.bg(Color::Indexed(i)),
                                vt100::Color::Rgb(r, g, b) => style = style.bg(Color::Rgb(r, g, b)),
                            }

                            if cell.bold() {
                                style = style.add_modifier(Modifier::BOLD);
                            }
                            if cell.italic() {
                                style = style.add_modifier(Modifier::ITALIC);
                            }
                            if cell.underline() {
                                style = style.add_modifier(Modifier::UNDERLINED);
                            }
                            if cell.inverse() {
                                style = style.add_modifier(Modifier::REVERSED);
                            }

                            let symbol = if cell.has_contents() {
                                cell.contents()
                            } else {
                                " ".to_string()
                            };
                            buf.get_mut(x, y).set_symbol(&symbol).set_style(style);
                        }
                    }
                }
            }
        }
    }
}