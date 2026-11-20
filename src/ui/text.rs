//! Text sanitization and wrapping helpers for voidrecon UI

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

/// Wraps a styled line into rows of at most `width` display columns,
/// breaking at spaces where possible and inside long words otherwise.
/// Rendering pre-wrapped rows keeps the row count exact for scrolling.
pub fn wrap_line(line: &Line<'static>, width: usize) -> Vec<Line<'static>> {
    use unicode_width::UnicodeWidthChar;
    let cells: Vec<(char, Style, usize)> = line
        .spans
        .iter()
        .flat_map(|span| {
            let style = line.style.patch(span.style);
            span.content
                .chars()
                .filter(|c| *c != '\n' && *c != '\r')
                .map(move |c| (c, style, c.width().unwrap_or(0)))
        })
        .collect();
    if width == 0 || cells.iter().map(|cell| cell.2).sum::<usize>() <= width {
        return vec![line.clone()];
    }

    let mut rows: Vec<Vec<(char, Style, usize)>> = Vec::new();
    let mut row: Vec<(char, Style, usize)> = Vec::new();
    let mut row_width = 0;
    for cell in cells {
        if row_width + cell.2 > width && !row.is_empty() {
            if cell.0 == ' ' {
                // The break falls on a space: it is dropped.
                rows.push(std::mem::take(&mut row));
                row_width = 0;
                continue;
            }
            match row.iter().rposition(|c| c.0 == ' ') {
                Some(space) if space > 0 => {
                    let carried = row.split_off(space + 1);
                    row.pop();
                    rows.push(std::mem::replace(&mut row, carried));
                }
                _ => rows.push(std::mem::take(&mut row)),
            }
            row_width = row.iter().map(|c| c.2).sum();
        }
        row_width += cell.2;
        row.push(cell);
    }
    if !row.is_empty() {
        rows.push(row);
    }
    rows.into_iter().map(cells_to_line).collect()
}

fn cells_to_line(cells: Vec<(char, Style, usize)>) -> Line<'static> {
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut text = String::new();
    let mut current: Option<Style> = None;
    for (c, style, _) in cells {
        if current.is_some_and(|s| s != style) {
            spans.push(Span::styled(std::mem::take(&mut text), current.unwrap_or_default()));
        }
        current = Some(style);
        text.push(c);
    }
    if !text.is_empty() {
        spans.push(Span::styled(text, current.unwrap_or_default()));
    }
    Line::from(spans)
}

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
                Some(_) | None => {}
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


#[cfg(test)]
mod wrap_tests {
    use super::*;

    fn row_text(line: &Line) -> String {
        line.spans.iter().map(|span| span.content.as_ref()).collect()
    }

    #[test]
    fn wraps_at_spaces_and_keeps_styles() {
        let bold = Style::default().add_modifier(Modifier::BOLD);
        let line = Line::from(vec![
            Span::styled("│ ", Style::default().fg(Color::Yellow)),
            Span::styled("open port 445 smb signing disabled", bold),
        ]);
        let rows = wrap_line(&line, 14);
        let texts: Vec<String> = rows.iter().map(row_text).collect();
        assert_eq!(texts, vec!["│ open port", "445 smb", "signing", "disabled"]);
        assert!(rows.iter().all(|row| row_text(row).chars().count() <= 14));
        assert_eq!(rows[1].spans[0].style, bold);
    }

    #[test]
    fn breaks_long_words_and_keeps_short_lines() {
        let rows = wrap_line(&Line::from("aaaaaaaaaa"), 4);
        let texts: Vec<String> = rows.iter().map(row_text).collect();
        assert_eq!(texts, vec!["aaaa", "aaaa", "aa"]);
        assert_eq!(wrap_line(&Line::from(""), 4).len(), 1, "empty line keeps its row");
        assert_eq!(wrap_line(&Line::from("abc"), 4).len(), 1);
    }
}
