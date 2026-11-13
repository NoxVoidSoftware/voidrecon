//! Text sanitization and wrapping helpers for voidrecon UI

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

pub fn strip_ansi_escapes(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();

    while let Some(c) = chars.next() {
        if c == '\x1b' {
            match chars.next() {
                Some('[') => {
                    for next_c in chars.by_ref() {
                        if (0x40..=0x7E).contains(&(next_c as u32)) {
                            break;
                        }
                    }
                }
                Some(']') => {
                    while let Some(next_c) = chars.next() {
                        if next_c == '\x07' {
                            break;
                        }
                        if next_c == '\x1b' && chars.peek() == Some(&'\\') {
                            chars.next();
                            break;
                        }
                    }
                }
                Some('(') | Some(')') => {
                    chars.next();
                }
                Some(other) => {
                    if !(0x40..=0x7E).contains(&(other as u32)) {
                        // ignore
                    }
                }
                None => {}
            }
        } else if c == '\t' {
            out.push_str("    ");
        } else if c == '\r' || c == '\n' || !c.is_control() {
            out.push(c);
        }
    }
    out
}

pub fn resolve_carriage_returns(text: &str) -> String {
    let mut result = Vec::new();
    for raw_line in text.split('\n') {
        let line = raw_line.trim_end_matches('\r');
        if !line.contains('\r') {
            result.push(line.to_string());
            continue;
        }

        let mut char_buf: Vec<char> = Vec::new();
        let mut cursor = 0;
        for c in line.chars() {
            if c == '\r' {
                cursor = 0;
            } else {
                if cursor < char_buf.len() {
                    char_buf[cursor] = c;
                } else {
                    char_buf.push(c);
                }
                cursor += 1;
            }
        }
        let resolved: String = char_buf.into_iter().collect();
        result.push(resolved);
    }
    result.join("\n")
}

pub fn sanitize_terminal_output(raw: &str) -> String {
    let stripped = strip_ansi_escapes(raw);
    resolve_carriage_returns(&stripped)
}

pub fn push_chunked_line(
    lines_out: &mut Vec<Line<'static>>,
    prefix: &str,
    prefix_style: Style,
    line: &str,
    text_style: Style,
) {
    if line.len() <= 200 {
        lines_out.push(Line::from(vec![
            Span::styled(prefix.to_string(), prefix_style),
            Span::styled(line.to_string(), text_style),
        ]));
        return;
    }

    let mut chunk_start = 0;
    let mut chunks_shown = 0;
    while chunk_start < line.len() {
        if chunks_shown >= 4 {
            lines_out.push(Line::from(vec![
                Span::styled(prefix.to_string(), prefix_style),
                Span::styled(
                    format!("... [long line truncated, total {} chars] ...", line.len()),
                    Style::default().fg(Color::DarkGray).add_modifier(Modifier::ITALIC),
                ),
            ]));
            break;
        }

        let mut chunk_end = (chunk_start + 200).min(line.len());
        while chunk_end > chunk_start && !line.is_char_boundary(chunk_end) {
            chunk_end -= 1;
        }
        if chunk_end == chunk_start {
            break;
        }

        let piece = &line[chunk_start..chunk_end];
        let p = if chunks_shown == 0 { prefix } else { "  " };
        lines_out.push(Line::from(vec![
            Span::styled(p.to_string(), prefix_style),
            Span::styled(piece.to_string(), text_style),
        ]));

        chunk_start = chunk_end;
        chunks_shown += 1;
    }
}

pub fn calculate_line_wrapped_rows(line_text: &str, max_width: usize) -> usize {
    if max_width == 0 {
        return 1;
    }
    use unicode_width::UnicodeWidthStr;
    let total_width = line_text.width();
    if total_width == 0 {
        return 1;
    }
    if total_width <= max_width {
        return 1;
    }

    let mut rows = 1;
    let mut current_row_width = 0;

    for word in line_text.split(' ') {
        let word_width = word.width();
        if word_width == 0 {
            // space
            if current_row_width + 1 <= max_width {
                current_row_width += 1;
            } else {
                rows += 1;
                current_row_width = 1;
            }
            continue;
        }

        if current_row_width == 0 {
            if word_width <= max_width {
                current_row_width = word_width;
            } else {
                let extra_rows = (word_width.saturating_sub(1)) / max_width;
                rows += extra_rows;
                current_row_width = word_width % max_width;
                if current_row_width == 0 {
                    current_row_width = max_width;
                }
            }
        } else if current_row_width + 1 + word_width <= max_width {
            current_row_width += 1 + word_width;
        } else {
            rows += 1;
            if word_width <= max_width {
                current_row_width = word_width;
            } else {
                let extra_rows = (word_width.saturating_sub(1)) / max_width;
                rows += extra_rows;
                current_row_width = word_width % max_width;
                if current_row_width == 0 {
                    current_row_width = max_width;
                }
            }
        }
    }

    rows
}

