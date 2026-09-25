//! Strict reader for the TOML subset used by configuration files.
//!
//! Accepted syntax:
//!
//! - blank lines and `#` comments, including a comment after a value;
//! - table headers holding one bare key, such as `[paths]`;
//! - `key = "value"` pairs with a bare key (`A-Z a-z 0-9 _ -`) and a
//!   double-quoted string without escape sequences.
//!
//! Every other construct is rejected instead of being interpreted, so an
//! accepted file is also valid TOML with the same meaning. Error messages
//! never reproduce values or unparsed text, because a malformed line may hold
//! a mistakenly pasted secret; they name a key or table only once it has
//! parsed as one.

use std::collections::BTreeMap;

/// A parsed configuration file.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Document {
    /// Tables by name. Keys that appear before the first table header belong
    /// to the root table, named `""`.
    pub(crate) tables: BTreeMap<String, Table>,
}

/// One table of a [`Document`].
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Table {
    /// Line of the table header, or 0 for the root table.
    pub(crate) line: usize,
    /// Values by key.
    pub(crate) entries: BTreeMap<String, Entry>,
}

/// One `key = "value"` pair.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Entry {
    pub(crate) value: String,
    pub(crate) line: usize,
}

/// Why a file could not be parsed.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct SyntaxError {
    /// 1-based line number.
    pub(crate) line: usize,
    pub(crate) message: String,
}

const WHITESPACE: [char; 2] = [' ', '\t'];

/// Parses `text`, stopping at the first syntax error.
pub(crate) fn parse(text: &str) -> Result<Document, SyntaxError> {
    let mut document = Document::default();
    let mut current_table = String::new();
    for (index, raw_line) in text.split('\n').enumerate() {
        let line = index + 1;
        let fail = |message: String| SyntaxError { line, message };
        let content = raw_line.strip_suffix('\r').unwrap_or(raw_line);
        if content.chars().any(is_disallowed_control) {
            return Err(fail(
                "control characters other than tab are not allowed".to_owned(),
            ));
        }
        let content = content.trim_start_matches(WHITESPACE);
        if content.is_empty() || content.starts_with('#') {
            continue;
        }
        if let Some(header) = content.strip_prefix('[') {
            let name = parse_header(header).map_err(|message| fail(message.to_owned()))?;
            if document.tables.contains_key(name) {
                return Err(fail(format!("table [{name}] is defined more than once")));
            }
            let table = Table {
                line,
                entries: BTreeMap::new(),
            };
            document.tables.insert(name.to_owned(), table);
            current_table = name.to_owned();
        } else {
            let (key, value) = parse_pair(content).map_err(|message| fail(message.to_owned()))?;
            let table = document
                .tables
                .entry(current_table.clone())
                .or_insert_with(|| Table {
                    line: 0,
                    entries: BTreeMap::new(),
                });
            if let Some(first) = table.entries.get(key) {
                return Err(fail(format!(
                    "key `{key}` is already defined on line {}",
                    first.line
                )));
            }
            let entry = Entry {
                value: value.to_owned(),
                line,
            };
            table.entries.insert(key.to_owned(), entry);
        }
    }
    Ok(document)
}

/// Parses the rest of a table header line after its opening `[`.
fn parse_header(text: &str) -> Result<&str, &'static str> {
    if text.starts_with('[') {
        return Err("arrays of tables are not supported");
    }
    let (name, rest) = split_key(text.trim_start_matches(WHITESPACE))?;
    let Some(rest) = rest.trim_start_matches(WHITESPACE).strip_prefix(']') else {
        return Err("expected `]` after the table name");
    };
    if !is_line_end(rest) {
        return Err("unexpected characters after the table header");
    }
    Ok(name)
}

/// Parses a `key = "value"` line into its key and value.
fn parse_pair(text: &str) -> Result<(&str, &str), &'static str> {
    let (key, rest) = split_key(text)?;
    let Some(rest) = rest.trim_start_matches(WHITESPACE).strip_prefix('=') else {
        return Err("expected `=` after the key");
    };
    let (value, rest) = split_string(rest.trim_start_matches(WHITESPACE))?;
    if !is_line_end(rest) {
        return Err("unexpected characters after the value");
    }
    Ok((key, value))
}

/// Splits a leading bare key from `text`.
fn split_key(text: &str) -> Result<(&str, &str), &'static str> {
    let end = text
        .find(|c: char| !is_bare_key_char(c))
        .unwrap_or(text.len());
    let (key, rest) = text.split_at(end);
    if key.is_empty() {
        return Err(if rest.starts_with(['"', '\'']) {
            "quoted keys are not supported"
        } else {
            "expected a key"
        });
    }
    if rest.trim_start_matches(WHITESPACE).starts_with('.') {
        return Err("dotted keys are not supported");
    }
    Ok((key, rest))
}

/// Splits a leading double-quoted string from `text`, returning its content
/// and the text after the closing quote.
fn split_string(text: &str) -> Result<(&str, &str), &'static str> {
    let Some(body) = text.strip_prefix('"') else {
        return Err(match text.chars().next() {
            None | Some('#') => "expected a value after `=`",
            Some('\'') => "literal strings are not supported; use double quotes",
            Some('[') => "arrays are not supported",
            Some('{') => "inline tables are not supported",
            Some(_) => "only double-quoted string values are supported",
        });
    };
    if body.starts_with("\"\"") {
        return Err("multi-line strings are not supported");
    }
    let end = body.find(['"', '\\']).ok_or("unterminated string")?;
    if body[end..].starts_with('\\') {
        return Err("escape sequences are not supported");
    }
    Ok((&body[..end], &body[end + 1..]))
}

/// Whether `text` holds only whitespace and an optional comment.
fn is_line_end(text: &str) -> bool {
    let rest = text.trim_start_matches(WHITESPACE);
    rest.is_empty() || rest.starts_with('#')
}

fn is_bare_key_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == '-'
}

/// Control characters TOML forbids in unescaped text: all but tab.
fn is_disallowed_control(c: char) -> bool {
    matches!(c, '\u{0}'..='\u{8}' | '\u{a}'..='\u{1f}' | '\u{7f}')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(value: &str, line: usize) -> Entry {
        Entry {
            value: value.to_owned(),
            line,
        }
    }

    fn error_of(text: &str) -> SyntaxError {
        parse(text).expect_err("text should be rejected")
    }

    #[test]
    fn parses_tables_pairs_and_comments() {
        let text = "# leading comment\n\
                    \n\
                    [paths]\n\
                    data = \"data\"  # trailing comment\n\
                    \t models\t=\t\"ümlaut dir\"\r\n\
                    [ logging ] # header comment\n\
                    level = \"DEBUG\"\n";
        let document = parse(text).unwrap();
        assert_eq!(document.tables.len(), 2);
        let paths = &document.tables["paths"];
        assert_eq!(paths.line, 3);
        assert_eq!(paths.entries.len(), 2);
        assert_eq!(paths.entries["data"], entry("data", 4));
        assert_eq!(paths.entries["models"], entry("ümlaut dir", 5));
        let logging = &document.tables["logging"];
        assert_eq!(logging.line, 6);
        assert_eq!(logging.entries.len(), 1);
        assert_eq!(logging.entries["level"], entry("DEBUG", 7));
    }

    #[test]
    fn keys_before_the_first_header_belong_to_the_root_table() {
        let document = parse("name = \"x\"\n[t]\n").unwrap();
        assert_eq!(document.tables[""].line, 0);
        assert_eq!(document.tables[""].entries["name"], entry("x", 1));
        assert!(document.tables["t"].entries.is_empty());
    }

    #[test]
    fn accepts_empty_documents_and_empty_strings() {
        assert_eq!(parse("").unwrap(), Document::default());
        assert_eq!(parse("\n# only a comment\n").unwrap(), Document::default());
        let document = parse("[t]\nk = \"\"").unwrap();
        assert_eq!(document.tables["t"].entries["k"], entry("", 2));
    }

    #[test]
    fn rejects_constructs_outside_the_subset() {
        let cases = [
            ("[[servers]]", "arrays of tables are not supported"),
            ("[a.b]", "dotted keys are not supported"),
            ("[a . b]", "dotted keys are not supported"),
            ("a.b = \"x\"", "dotted keys are not supported"),
            ("\"a\" = \"x\"", "quoted keys are not supported"),
            ("'a' = \"x\"", "quoted keys are not supported"),
            (
                "a = 'x'",
                "literal strings are not supported; use double quotes",
            ),
            ("a = \"\"\"x\"\"\"", "multi-line strings are not supported"),
            ("a = [\"x\"]", "arrays are not supported"),
            ("a = { b = \"x\" }", "inline tables are not supported"),
            ("a = true", "only double-quoted string values are supported"),
            ("a = 1", "only double-quoted string values are supported"),
            (
                "a = 1979-05-27",
                "only double-quoted string values are supported",
            ),
            ("a = \"x\\ty\"", "escape sequences are not supported"),
        ];
        for (text, message) in cases {
            let expected = SyntaxError {
                line: 1,
                message: message.to_owned(),
            };
            assert_eq!(error_of(text), expected, "{text:?}");
        }
    }

    #[test]
    fn rejects_malformed_lines() {
        let cases = [
            ("a = \"x", "unterminated string"),
            ("a = \"x\" y", "unexpected characters after the value"),
            ("a \"x\"", "expected `=` after the key"),
            ("a", "expected `=` after the key"),
            ("a =", "expected a value after `=`"),
            ("a = # nothing", "expected a value after `=`"),
            ("= \"x\"", "expected a key"),
            ("[a", "expected `]` after the table name"),
            ("[]", "expected a key"),
            ("[a] b", "unexpected characters after the table header"),
            (
                "a = \"x\u{1}\"",
                "control characters other than tab are not allowed",
            ),
            (
                "# comment \u{7f}",
                "control characters other than tab are not allowed",
            ),
            (
                "a = \"x\"\r\r",
                "control characters other than tab are not allowed",
            ),
        ];
        for (text, message) in cases {
            let expected = SyntaxError {
                line: 1,
                message: message.to_owned(),
            };
            assert_eq!(error_of(text), expected, "{text:?}");
        }
    }

    #[test]
    fn rejects_redefinitions_with_line_numbers() {
        assert_eq!(
            error_of("[t]\na = \"1\"\n\na = \"2\""),
            SyntaxError {
                line: 4,
                message: "key `a` is already defined on line 2".to_owned(),
            }
        );
        assert_eq!(
            error_of("[t]\n[u]\n[t]"),
            SyntaxError {
                line: 3,
                message: "table [t] is defined more than once".to_owned(),
            }
        );
    }

    #[test]
    fn reports_the_line_of_the_first_error() {
        assert_eq!(error_of("[t]\nok = \"1\"\n\nbad = 2\nalso bad").line, 4);
    }

    #[test]
    fn error_messages_never_contain_values_or_unparsed_text() {
        let secret = "fake_secret_7Qx9";
        let cases = [
            format!("token = {secret}"),
            format!("token = '{secret}'"),
            format!("token = \"{secret}"),
            format!("token = \"{secret}\" {secret}"),
            format!("token = \"{secret}\\n\""),
            secret.to_owned(),
            format!("[{secret}"),
        ];
        for text in cases {
            let error = error_of(&text);
            assert!(!error.message.contains(secret), "{text:?} -> {error:?}");
        }
    }
}
