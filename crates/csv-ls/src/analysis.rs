//! Turns parse results into LSP diagnostics and hover content.

use lsp_types::{Diagnostic, DiagnosticSeverity, Position, Range};

use crate::parse::{ErrorKind, Field, Parsed, Pos, Record};

const SOURCE: &str = "csv-ls";

fn range(start: Pos, end: Pos) -> Range {
    Range {
        start: Position::new(start.line, start.col),
        end: Position::new(end.line, end.col),
    }
}

/// The header record: the first non-blank record, if any.
pub fn header(parsed: &Parsed) -> Option<&Record> {
    parsed.records.iter().find(|r| !r.is_blank())
}

pub fn diagnostics(parsed: &Parsed) -> Vec<Diagnostic> {
    let mut diags = Vec::new();

    for err in &parsed.errors {
        let message = match err.kind {
            ErrorKind::UnclosedQuote => "unclosed quote: quoted field is never terminated",
            ErrorKind::TextAfterClosingQuote => {
                "text after closing quote; to include a literal quote, double it (\"\")"
            }
        };
        diags.push(Diagnostic {
            range: range(err.start, err.end),
            severity: Some(DiagnosticSeverity::ERROR),
            source: Some(SOURCE.into()),
            message: message.into(),
            ..Diagnostic::default()
        });
    }

    if let Some(head) = header(parsed) {
        let expected = head.fields.len();
        let header_line = head.start.line;
        for record in &parsed.records {
            if record.start.line == header_line || record.is_blank() {
                continue;
            }
            let got = record.fields.len();
            if got != expected {
                diags.push(Diagnostic {
                    range: range(record.start, record.end),
                    severity: Some(DiagnosticSeverity::WARNING),
                    source: Some(SOURCE.into()),
                    message: format!(
                        "row has {got} field{}, but the header row has {expected}",
                        if got == 1 { "" } else { "s" }
                    ),
                    ..Diagnostic::default()
                });
            }
        }
    }

    diags
}

/// Find the record and field index containing `pos`. A position exactly at a
/// field's end (e.g. the cursor sitting just after the last character) still
/// counts as inside that field.
fn field_at(parsed: &Parsed, pos: Pos) -> Option<(usize, &Record, usize, &Field)> {
    for (row_idx, record) in parsed.records.iter().enumerate() {
        if pos < record.start || pos > record.end {
            continue;
        }
        for (col_idx, f) in record.fields.iter().enumerate() {
            if pos >= f.start && pos <= f.end {
                return Some((row_idx, record, col_idx, f));
            }
        }
    }
    None
}

pub struct HoverInfo {
    pub markdown: String,
    pub range: Range,
}

pub fn hover(parsed: &Parsed, line: u32, col: u32) -> Option<HoverInfo> {
    let pos = Pos { line, col };
    let (row_idx, record, col_idx, field) = field_at(parsed, pos)?;
    if record.is_blank() {
        return None;
    }

    let head = header(parsed)?;
    let is_header_row = std::ptr::eq(record, head);
    let column_name = head.fields.get(col_idx).map(|f| f.text.as_str());
    let total_cols = record.fields.len();
    let data_rows = parsed.records.iter().filter(|r| !r.is_blank()).count() - 1;

    let mut md = match column_name {
        Some(name) if !name.is_empty() => {
            format!("**{}** — column {} of {}", name, col_idx + 1, total_cols)
        }
        _ => format!("column {} of {}", col_idx + 1, total_cols),
    };
    if is_header_row {
        md.push_str("\n\nheader row");
    } else {
        // 1-based data row index: count non-blank records before this one,
        // excluding the header.
        let data_row = parsed.records[..row_idx]
            .iter()
            .filter(|r| !r.is_blank())
            .count();
        md.push_str(&format!("\n\nrow {data_row} of {data_rows}"));
    }
    if field.quoted {
        md.push_str(" · quoted field");
    }

    Some(HoverInfo {
        markdown: md,
        range: range(field.start, field.end),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse::parse;

    #[test]
    fn ragged_row_warning() {
        let p = parse("a,b,c\n1,2\n1,2,3\n", ',');
        let d = diagnostics(&p);
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].severity, Some(DiagnosticSeverity::WARNING));
        assert!(d[0].message.contains("2 fields"));
        assert_eq!(d[0].range.start.line, 1);
    }

    #[test]
    fn blank_lines_not_flagged() {
        let p = parse("a,b\n\n1,2\n", ',');
        assert!(diagnostics(&p).is_empty());
    }

    #[test]
    fn unclosed_quote_error() {
        let p = parse("a,b\n\"x,y\n", ',');
        let d = diagnostics(&p);
        assert!(d
            .iter()
            .any(|d| d.severity == Some(DiagnosticSeverity::ERROR)));
    }

    #[test]
    fn hover_data_cell() {
        let p = parse("name,age\nalice,30\nbob,40\n", ',');
        let h = hover(&p, 1, 6).expect("hover on '30'");
        assert!(h.markdown.contains("**age**"), "{}", h.markdown);
        assert!(h.markdown.contains("column 2 of 2"));
        assert!(h.markdown.contains("row 1 of 2"));
    }

    #[test]
    fn hover_header_cell() {
        let p = parse("name,age\nalice,30\n", ',');
        let h = hover(&p, 0, 0).expect("hover on 'name'");
        assert!(h.markdown.contains("header row"));
    }

    #[test]
    fn hover_outside_any_field() {
        let p = parse("a,b\n", ',');
        assert!(hover(&p, 5, 0).is_none());
    }

    #[test]
    fn hover_extra_column_without_header_name() {
        let p = parse("a,b\n1,2,3\n", ',');
        let h = hover(&p, 1, 4).expect("hover on '3'");
        assert!(h.markdown.contains("column 3 of 3"));
        assert!(!h.markdown.contains("**"));
    }
}
