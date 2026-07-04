//! A small, lenient RFC 4180-style scanner for delimiter-separated values.
//!
//! Hand-rolled instead of using the `csv` crate because we need precise
//! per-field source spans in LSP coordinates (line + UTF-16 column) and we
//! want the parsing core to be trivially auditable. Quoted fields may span
//! multiple lines; quotes are escaped by doubling (`""`).

/// A position in the document, LSP-style: 0-based line, 0-based column
/// measured in UTF-16 code units.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Pos {
    pub line: u32,
    pub col: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Field {
    pub start: Pos,
    pub end: Pos,
    pub quoted: bool,
    /// Decoded field content (quotes stripped, `""` unescaped).
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Record {
    pub start: Pos,
    pub end: Pos,
    pub fields: Vec<Field>,
}

impl Record {
    /// A record that is just an empty line (single empty unquoted field).
    pub fn is_blank(&self) -> bool {
        self.fields.len() == 1 && !self.fields[0].quoted && self.fields[0].text.is_empty()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    /// A quoted field was never closed before end of file.
    UnclosedQuote,
    /// Content followed the closing quote of a quoted field, e.g. `"a"b`.
    TextAfterClosingQuote,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError {
    pub start: Pos,
    pub end: Pos,
    pub kind: ErrorKind,
}

#[derive(Debug, Default)]
pub struct Parsed {
    pub records: Vec<Record>,
    pub errors: Vec<ParseError>,
}

struct Scanner<'a> {
    chars: std::iter::Peekable<std::str::Chars<'a>>,
    pos: Pos,
}

impl<'a> Scanner<'a> {
    fn new(text: &'a str) -> Self {
        Scanner {
            chars: text.chars().peekable(),
            pos: Pos { line: 0, col: 0 },
        }
    }

    fn peek(&mut self) -> Option<char> {
        self.chars.peek().copied()
    }

    /// Consume one char, updating the LSP position. `\r\n`, `\n`, and a lone
    /// `\r` each advance the line counter; the caller detects newlines by
    /// comparing `pos.line` before and after.
    fn bump(&mut self) -> Option<char> {
        let c = self.chars.next()?;
        match c {
            '\n' => {
                self.pos.line += 1;
                self.pos.col = 0;
            }
            '\r' => {
                if self.peek() == Some('\n') {
                    self.chars.next();
                }
                self.pos.line += 1;
                self.pos.col = 0;
            }
            _ => self.pos.col += c.len_utf16() as u32,
        }
        Some(c)
    }
}

/// Parse `text` into records of fields, using `delim` as the field separator.
/// Never fails: malformed input is recovered from and reported in `errors`.
pub fn parse(text: &str, delim: char) -> Parsed {
    let mut s = Scanner::new(text);
    let mut parsed = Parsed::default();

    while s.peek().is_some() {
        let record_start = s.pos;
        let mut fields = Vec::new();
        loop {
            let (field, more_in_record) = scan_field(&mut s, delim, &mut parsed.errors);
            fields.push(field);
            if !more_in_record {
                break;
            }
        }
        parsed.records.push(Record {
            start: record_start,
            end: fields.last().map(|f| f.end).unwrap_or(record_start),
            fields,
        });
    }
    parsed
}

/// Scan a single field. Returns the field and whether the record continues
/// (true when the field was terminated by a delimiter rather than a newline
/// or end of input).
fn scan_field(s: &mut Scanner, delim: char, errors: &mut Vec<ParseError>) -> (Field, bool) {
    let start = s.pos;
    let mut text = String::new();
    let quoted = s.peek() == Some('"');

    if quoted {
        s.bump();
        loop {
            match s.bump() {
                None => {
                    errors.push(ParseError {
                        start,
                        end: s.pos,
                        kind: ErrorKind::UnclosedQuote,
                    });
                    return (make_field(start, s.pos, true, text), false);
                }
                Some('"') => {
                    if s.peek() == Some('"') {
                        s.bump();
                        text.push('"');
                    } else {
                        break;
                    }
                }
                Some(c) => text.push(c),
            }
        }
        // After the closing quote only a delimiter, newline, or EOF is valid.
        match s.peek() {
            None => return (make_field(start, s.pos, true, text), false),
            Some(c) if c == delim => {
                let end = s.pos;
                s.bump();
                return (make_field(start, end, true, text), true);
            }
            Some('\n') | Some('\r') => {
                let end = s.pos;
                s.bump();
                return (make_field(start, end, true, text), false);
            }
            Some(_) => {
                let junk_start = s.pos;
                let more = scan_unquoted_tail(s, delim, &mut text);
                let end = s.pos;
                errors.push(ParseError {
                    start: junk_start,
                    end,
                    kind: ErrorKind::TextAfterClosingQuote,
                });
                if s.peek().is_some() {
                    s.bump(); // consume the delimiter or newline
                }
                return (make_field(start, end, true, text), more);
            }
        }
    }

    let more = scan_unquoted_tail(s, delim, &mut text);
    let end = s.pos;
    if s.peek().is_some() {
        s.bump(); // consume the delimiter or newline
    }
    (make_field(start, end, false, text), more)
}

/// Consume unquoted content up to (not including) the next delimiter,
/// newline, or EOF. Returns true if stopped at a delimiter.
fn scan_unquoted_tail(s: &mut Scanner, delim: char, text: &mut String) -> bool {
    loop {
        match s.peek() {
            None => return false,
            Some('\n') | Some('\r') => return false,
            Some(c) if c == delim => return true,
            Some(c) => {
                text.push(c);
                s.bump();
            }
        }
    }
}

fn make_field(start: Pos, end: Pos, quoted: bool, text: String) -> Field {
    Field {
        start,
        end,
        quoted,
        text,
    }
}

/// Delimiter for a Zed language id; `None` means unknown (caller may sniff).
pub fn delimiter_for_language_id(language_id: &str) -> Option<char> {
    match language_id.to_ascii_lowercase().as_str() {
        "csv" => Some(','),
        "tsv" => Some('\t'),
        "ssv" => Some(';'),
        "psv" => Some('|'),
        _ => None,
    }
}

/// Guess the delimiter by counting candidate separators outside quotes on
/// the first non-empty line. Ties resolve in the order `, \t ; |`.
pub fn sniff_delimiter(text: &str) -> char {
    let first_line = text.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
    let mut counts = [(',', 0usize), ('\t', 0), (';', 0), ('|', 0)];
    let mut in_quotes = false;
    for c in first_line.chars() {
        if c == '"' {
            in_quotes = !in_quotes;
        } else if !in_quotes {
            for entry in counts.iter_mut() {
                if entry.0 == c {
                    entry.1 += 1;
                }
            }
        }
    }
    // Strictly-greater comparison so ties keep the earliest candidate.
    let mut best = counts[0];
    for &(c, n) in &counts[1..] {
        if n > best.1 {
            best = (c, n);
        }
    }
    best.0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texts(parsed: &Parsed) -> Vec<Vec<&str>> {
        parsed
            .records
            .iter()
            .map(|r| r.fields.iter().map(|f| f.text.as_str()).collect())
            .collect()
    }

    #[test]
    fn simple() {
        let p = parse("a,b,c\n1,2,3\n", ',');
        assert_eq!(texts(&p), vec![vec!["a", "b", "c"], vec!["1", "2", "3"]]);
        assert!(p.errors.is_empty());
    }

    #[test]
    fn no_trailing_newline() {
        let p = parse("a,b\n1,2", ',');
        assert_eq!(texts(&p), vec![vec!["a", "b"], vec!["1", "2"]]);
    }

    #[test]
    fn crlf() {
        let p = parse("a,b\r\n1,2\r\n", ',');
        assert_eq!(texts(&p), vec![vec!["a", "b"], vec!["1", "2"]]);
        assert_eq!(p.records[1].start, Pos { line: 1, col: 0 });
    }

    #[test]
    fn quoted_with_escapes_and_delims() {
        let p = parse("\"a,x\",\"say \"\"hi\"\"\"\n", ',');
        assert_eq!(texts(&p), vec![vec!["a,x", "say \"hi\""]]);
        assert!(p.errors.is_empty());
        assert!(p.records[0].fields[0].quoted);
    }

    #[test]
    fn quoted_multiline_field() {
        let p = parse("a,\"line1\nline2\",c\nx,y,z\n", ',');
        assert_eq!(
            texts(&p),
            vec![vec!["a", "line1\nline2", "c"], vec!["x", "y", "z"]]
        );
        // The record after the multi-line field starts on line 2.
        assert_eq!(p.records[1].start, Pos { line: 2, col: 0 });
    }

    #[test]
    fn empty_fields() {
        let p = parse(",,\n", ',');
        assert_eq!(texts(&p), vec![vec!["", "", ""]]);
    }

    #[test]
    fn blank_line_is_blank_record() {
        let p = parse("a,b\n\nc,d\n", ',');
        assert_eq!(p.records.len(), 3);
        assert!(p.records[1].is_blank());
        assert!(!p.records[0].is_blank());
    }

    #[test]
    fn unclosed_quote() {
        let p = parse("a,\"oops\n", ',');
        assert_eq!(p.errors.len(), 1);
        assert_eq!(p.errors[0].kind, ErrorKind::UnclosedQuote);
    }

    #[test]
    fn text_after_closing_quote() {
        let p = parse("\"a\"junk,b\n", ',');
        assert_eq!(p.errors.len(), 1);
        assert_eq!(p.errors[0].kind, ErrorKind::TextAfterClosingQuote);
        // Recovers: still two fields.
        assert_eq!(p.records[0].fields.len(), 2);
        assert_eq!(p.records[0].fields[0].text, "ajunk");
    }

    #[test]
    fn utf16_columns() {
        // '😀' is 2 UTF-16 units; 'é' is 1.
        let p = parse("😀é,b\n", ',');
        let f = &p.records[0].fields[1];
        assert_eq!(f.start, Pos { line: 0, col: 4 });
    }

    #[test]
    fn tsv() {
        let p = parse("a\tb\n1\t2\n", '\t');
        assert_eq!(texts(&p), vec![vec!["a", "b"], vec!["1", "2"]]);
    }

    #[test]
    fn field_spans() {
        let p = parse("ab,cde\n", ',');
        let r = &p.records[0];
        assert_eq!(r.fields[0].start, Pos { line: 0, col: 0 });
        assert_eq!(r.fields[0].end, Pos { line: 0, col: 2 });
        assert_eq!(r.fields[1].start, Pos { line: 0, col: 3 });
        assert_eq!(r.fields[1].end, Pos { line: 0, col: 6 });
    }

    #[test]
    fn sniff() {
        assert_eq!(sniff_delimiter("a;b;c\n1;2;3\n"), ';');
        assert_eq!(sniff_delimiter("a\tb\tc\n"), '\t');
        assert_eq!(sniff_delimiter("\"a;b\",c\n"), ',');
        assert_eq!(sniff_delimiter(""), ',');
    }

    #[test]
    fn language_ids() {
        assert_eq!(delimiter_for_language_id("CSV"), Some(','));
        assert_eq!(delimiter_for_language_id("tsv"), Some('\t'));
        assert_eq!(delimiter_for_language_id("plain"), None);
    }
}
