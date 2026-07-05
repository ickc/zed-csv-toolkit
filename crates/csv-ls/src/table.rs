//! Builds the Tabular Data Resource JSON
//! (https://specs.frictionlessdata.io/tabular-data-resource/) that Zed's
//! REPL renders as an inline table. Mirrors the semantics of the retired
//! Python `csv_kernel.py`, but reuses `parse::parse` instead of Python's
//! `csv` module.

use std::collections::HashMap;

use serde_json::{json, Map, Value};

use crate::parse;

pub const MIME: &str = "application/vnd.dataresource+json";

/// Delimiter from `CSV_KERNEL_DELIMITER` (first char, if set and non-empty),
/// else sniffed from `text` using the same candidates/precedence as the
/// retired Python kernel (comma, tab, semicolon, pipe).
pub fn resolve_delimiter(text: &str) -> char {
    if let Ok(val) = std::env::var("CSV_KERNEL_DELIMITER") {
        if let Some(c) = val.chars().next() {
            return c;
        }
    }
    parse::sniff_delimiter(text)
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum ColType {
    Integer,
    Number,
    String,
}

impl ColType {
    fn as_str(self) -> &'static str {
        match self {
            ColType::Integer => "integer",
            ColType::Number => "number",
            ColType::String => "string",
        }
    }
}

/// A built table plus the `text/plain` fallback summary shown next to it.
pub struct Table {
    pub value: Value,
    pub summary: String,
}

/// The non-blank rows of `text`, padded to a uniform width; the first row
/// is the header. `None` for empty/whitespace-only input (no rows at all).
fn grid(text: &str, delimiter: char) -> Option<(Vec<String>, Vec<Vec<String>>)> {
    let parsed = parse::parse(text, delimiter);
    let rows: Vec<Vec<String>> = parsed
        .records
        .into_iter()
        .filter(|r| !r.is_blank())
        .map(|r| r.fields.into_iter().map(|f| f.text).collect())
        .collect();
    if rows.is_empty() {
        return None;
    }

    let width = rows.iter().map(Vec::len).max().unwrap_or(0);
    let mut rows = rows.into_iter();
    let mut header = rows.next().unwrap();
    header.resize(width, String::new());
    let padded: Vec<Vec<String>> = rows
        .map(|mut r| {
            r.resize(width, String::new());
            r
        })
        .collect();
    Some((header, padded))
}

/// Parse `text` as delimiter-separated values and build a Tabular Data
/// Resource. `None` for empty/whitespace-only input (no rows at all).
pub fn build(text: &str, delimiter: char) -> Option<Table> {
    let (header, padded) = grid(text, delimiter)?;
    let width = header.len();
    let names = uniquify(&header);

    let types: Vec<ColType> = (0..width)
        .map(|i| column_type(padded.iter().map(|r| r[i].as_str())))
        .collect();

    let fields: Vec<Value> = names
        .iter()
        .zip(&types)
        .map(|(n, t)| json!({"name": n, "type": t.as_str()}))
        .collect();

    let data: Vec<Value> = padded
        .iter()
        .map(|row| {
            let mut obj = Map::new();
            for ((name, value), ty) in names.iter().zip(row.iter()).zip(&types) {
                obj.insert(name.clone(), convert(value, *ty));
            }
            Value::Object(obj)
        })
        .collect();

    let summary = format!("{} rows × {} columns", data.len(), fields.len());
    let value = json!({
        "schema": {"fields": fields},
        "data": data,
    });
    Some(Table { value, summary })
}

/// Field names must be unique and non-empty to key the data objects: blank
/// names become "column N" (1-based), duplicates get " (2)", " (3)", ...
fn uniquify(names: &[String]) -> Vec<String> {
    let mut seen: HashMap<String, usize> = HashMap::new();
    names
        .iter()
        .enumerate()
        .map(|(i, raw)| {
            let trimmed = raw.trim();
            let mut name = if trimmed.is_empty() {
                format!("column {}", i + 1)
            } else {
                trimmed.to_string()
            };
            if let Some(count) = seen.get(&name).copied() {
                let next = count + 1;
                seen.insert(name.clone(), next);
                name = format!("{name} ({next})");
            }
            seen.entry(name.clone()).or_insert(1);
            name
        })
        .collect()
}

/// Frictionless field type: integer/number if every non-empty value in the
/// column parses; an all-empty column is a string column.
fn column_type<'a>(values: impl Iterator<Item = &'a str>) -> ColType {
    let non_empty: Vec<&str> = values.filter(|v| !v.is_empty()).collect();
    if non_empty.is_empty() {
        return ColType::String;
    }
    if non_empty.iter().all(|v| v.parse::<i64>().is_ok()) {
        return ColType::Integer;
    }
    if non_empty.iter().all(|v| v.parse::<f64>().is_ok()) {
        return ColType::Number;
    }
    ColType::String
}

fn convert(value: &str, ty: ColType) -> Value {
    if value.is_empty() {
        return Value::Null;
    }
    match ty {
        ColType::Integer => value.parse::<i64>().map(Value::from).unwrap_or(Value::Null),
        ColType::Number => value
            .parse::<f64>()
            .ok()
            .and_then(serde_json::Number::from_f64)
            .map(Value::Number)
            .unwrap_or(Value::Null),
        ColType::String => Value::String(value.to_string()),
    }
}

/// Render `text` as a GitHub-flavored markdown pipe table (the same shape
/// Zed's table widget puts on the clipboard): header row, alignment row
/// (numeric columns right-aligned), data rows. Cells are escaped so pipes
/// and embedded newlines can't break the table. `None` for empty input.
pub fn markdown(text: &str, delimiter: char) -> Option<String> {
    let (header, padded) = grid(text, delimiter)?;
    let width = header.len();
    let types: Vec<ColType> = (0..width)
        .map(|i| column_type(padded.iter().map(|r| r[i].as_str())))
        .collect();

    let mut out = String::new();
    push_row(&mut out, header.iter().map(String::as_str));
    push_row(
        &mut out,
        types
            .iter()
            .map(|t| if *t == ColType::String { "---" } else { "--:" }),
    );
    for row in &padded {
        push_row(&mut out, row.iter().map(String::as_str));
    }
    Some(out)
}

fn push_row<'a>(out: &mut String, cells: impl Iterator<Item = &'a str>) {
    for cell in cells {
        out.push_str("| ");
        out.push_str(&escape_markdown_cell(cell));
        out.push(' ');
    }
    out.push_str("|\n");
}

fn escape_markdown_cell(cell: &str) -> String {
    cell.replace('\\', "\\\\")
        .replace('|', "\\|")
        .replace("\r\n", "<br>")
        .replace(['\r', '\n'], "<br>")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fields_and_data(t: &Table) -> (&Value, &Value) {
        (&t.value["schema"]["fields"], &t.value["data"])
    }

    #[test]
    fn empty_input_is_none() {
        assert!(build("", ',').is_none());
        assert!(build("\n\n\n", ',').is_none());
    }

    #[test]
    fn typing_and_nulls() {
        let t = build("name,age\nalice,30\nbob,\n", ',').unwrap();
        let (fields, data) = fields_and_data(&t);
        assert_eq!(
            *fields,
            json!([
                {"name": "name", "type": "string"},
                {"name": "age", "type": "integer"},
            ])
        );
        assert_eq!(
            *data,
            json!([
                {"name": "alice", "age": 30},
                {"name": "bob", "age": null},
            ])
        );
        assert_eq!(t.summary, "2 rows × 2 columns");
    }

    #[test]
    fn number_typing() {
        let t = build("a\tb\n1.5\t2\n", '\t').unwrap();
        assert_eq!(t.value["schema"]["fields"][0]["type"], "number");
        assert_eq!(t.value["data"], json!([{"a": 1.5, "b": 2}]));
    }

    #[test]
    fn all_empty_column_is_string() {
        let t = build("a,b\n1,\n2,\n", ',').unwrap();
        assert_eq!(t.value["schema"]["fields"][1]["type"], "string");
    }

    #[test]
    fn uniquify_blanks_and_duplicates() {
        assert_eq!(
            uniquify(&["".into(), "a".into(), "a".into(), "a".into()]),
            vec!["column 1", "a", "a (2)", "a (3)"]
        );
        assert_eq!(uniquify(&["  ".into(), "x".into()]), vec!["column 1", "x"]);
    }

    #[test]
    fn ragged_rows_are_padded() {
        let t = build("a,b,c\n1,2\n3,4,5,6\n", ',').unwrap();
        // Header width (3) wins for names; data width follows the max (4).
        let fields = t.value["schema"]["fields"].as_array().unwrap();
        assert_eq!(fields.len(), 4);
        assert_eq!(fields[3]["name"], "column 4");
        assert_eq!(t.value["data"][0]["column 4"], Value::Null);
    }

    #[test]
    fn markdown_table_with_numeric_alignment() {
        let md = markdown("name,age\nalice,30\nbob,\n", ',').unwrap();
        assert_eq!(
            md,
            "| name | age |\n\
             | --- | --: |\n\
             | alice | 30 |\n\
             | bob |  |\n"
        );
    }

    #[test]
    fn markdown_escapes_pipes_and_newlines() {
        // "a|b" and an embedded newline (via a quoted field) must not break
        // the pipe table.
        let md = markdown("h\n\"a|b\"\n\"x\ny\"\n", ',').unwrap();
        assert_eq!(md, "| h |\n| --- |\n| a\\|b |\n| x<br>y |\n");
    }

    #[test]
    fn markdown_empty_input_is_none() {
        assert!(markdown("", ',').is_none());
    }

    #[test]
    fn blank_lines_are_dropped() {
        let t = build("a,b\n\n1,2\n", ',').unwrap();
        assert_eq!(t.value["data"], json!([{"a": 1, "b": 2}]));
    }

    // Cargo runs tests on parallel threads and the environment is
    // process-global, so the two tests touching CSV_KERNEL_DELIMITER
    // serialize on this lock instead of racing each other.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn resolve_delimiter_env_override() {
        let _guard = ENV_LOCK.lock().unwrap();
        std::env::set_var("CSV_KERNEL_DELIMITER", ";");
        assert_eq!(resolve_delimiter("a,b;c\n"), ';');
        std::env::remove_var("CSV_KERNEL_DELIMITER");
    }

    #[test]
    fn resolve_delimiter_sniffs_without_env() {
        let _guard = ENV_LOCK.lock().unwrap();
        std::env::remove_var("CSV_KERNEL_DELIMITER");
        assert_eq!(resolve_delimiter("a;b;c\n"), ';');
    }
}
