//! Custom widgets for voidrecon UI

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::Style,
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
            let contents = screen.contents();
            for (y, line) in contents.lines().enumerate() {
                let row = area.top() + y as u16;
                if row >= area.bottom() {
                    break;
                }
                for (x, ch) in line.chars().enumerate() {
                    let col = area.left() + x as u16;
                    if col >= area.right() {
                        break;
                    }
                    buf.get_mut(col, row).set_symbol(&ch.to_string()).set_style(Style::default());
                }
            }
        }
    }
}