//! `AdsGetSQLStmtParams`: the parameter markers of a statement's text.
//!
//! The original engine rewrites the text in place, keeping its length: each
//! parameter marker (`?`, or `:name`) stays where it is and every other
//! character becomes a space, string literals, quoted names and comments
//! included. Delphi's `TParams.ParseSQL` then finds the parameters in what
//! is left (`TAdsQuery` in the SDK's TDataset sources).

/// Rewrites `text` (Windows-1252 bytes, up to its NUL) as
/// `AdsGetSQLStmtParams` does: everything but parameter markers becomes
/// spaces.
pub fn blank_all_but_params(text: &mut [u8]) {
    let n = text.len();
    let mut i = 0;
    let name = |b: u8| b.is_ascii_alphanumeric() || b == b'_';
    while i < n {
        let b = text[i];
        let next = text.get(i + 1).copied();
        match b {
            b'?' => i += 1,
            b':' if next == Some(b':') => {
                // `::` is no marker, and neither is the name after it.
                let mut j = i + 2;
                while j < n && (name(text[j]) || text[j] >= 0x80) {
                    j += 1;
                }
                text[i..j].fill(b' ');
                i = j;
            }
            b':' if next.is_some_and(name) => {
                i += 1;
                while i < n && name(text[i]) {
                    i += 1;
                }
                // A name that runs into other letters is no marker.
                if i < n && text[i] >= 0x80 {
                    let start = text[..i].iter().rposition(|&b| b == b':').unwrap_or(i);
                    let mut j = i;
                    while j < n && (name(text[j]) || text[j] >= 0x80) {
                        j += 1;
                    }
                    text[start..j].fill(b' ');
                    i = j;
                }
            }
            b':' if next.is_some_and(|b| b >= 0x80) => {
                let mut j = i + 1;
                while j < n && (name(text[j]) || text[j] >= 0x80) {
                    j += 1;
                }
                text[i..j].fill(b' ');
                i = j;
            }
            b':' => i += 1,
            b'\'' | b'"' | b'[' => {
                let close = if b == b'[' { b']' } else { b };
                let mut j = i + 1;
                loop {
                    if j >= n {
                        break;
                    }
                    if text[j] == close {
                        // `''` inside a string is a quote.
                        if close == b'\'' && text.get(j + 1) == Some(&b'\'') {
                            j += 2;
                            continue;
                        }
                        j += 1;
                        break;
                    }
                    j += 1;
                }
                text[i..j].fill(b' ');
                i = j;
            }
            b'-' if next == Some(b'-') => i = blank_line(text, i),
            b'/' if next == Some(b'/') => i = blank_line(text, i),
            // A block comment only once it is closed; unclosed, its text
            // is like any other.
            b'/' if next == Some(b'*') && closes(text, i + 2) => {
                let mut j = i + 2;
                while !(text[j] == b'*' && text[j + 1] == b'/') {
                    j += 1;
                }
                text[i..j + 2].fill(b' ');
                i = j + 2;
            }
            _ => {
                text[i] = b' ';
                i += 1;
            }
        }
    }
}

/// The parameter markers of `text`, in the order they come: `None` for a
/// `?`, the name (without its `:`) for `:name`. A name that comes twice is
/// two markers, as the original engine counts them
/// (`fixtures/sql/params5.json`); text that does not parse has them too.
pub fn markers(text: &str) -> Vec<Option<String>> {
    let mut bytes = tflip_storage::cp1252::encode(text);
    blank_all_but_params(&mut bytes);
    let mut found = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'?' => found.push(None),
            b':' => {
                let is_name = |b: u8| b.is_ascii_alphanumeric() || b == b'_';
                let mut end = i + 1;
                // A name goes on over `:name` after it: `:a:b` is one marker.
                while end < bytes.len() && is_name(bytes[end]) {
                    while end < bytes.len() && is_name(bytes[end]) {
                        end += 1;
                    }
                    if bytes.get(end) == Some(&b':')
                        && bytes.get(end + 1).copied().is_some_and(is_name)
                    {
                        end += 1;
                    }
                }
                if end > i + 1 {
                    found.push(Some(
                        String::from_utf8_lossy(&bytes[i + 1..end]).into_owned(),
                    ));
                    i = end - 1;
                }
            }
            _ => {}
        }
        i += 1;
    }
    found
}

/// Whether a `*/` follows position `from`.
fn closes(text: &[u8], from: usize) -> bool {
    text.get(from..)
        .is_some_and(|rest| rest.windows(2).any(|w| w == b"*/"))
}

/// Blanks a comment from `i` to the end of its line, the line break
/// included; returns where the next character is.
fn blank_line(text: &mut [u8], i: usize) -> usize {
    let mut j = i;
    while j < text.len() && text[j] != b'\n' && text[j] != b'\r' {
        j += 1;
    }
    text[i..j].fill(b' ');
    j
}

#[cfg(test)]
mod tests {
    use super::*;

    fn blanked(text: &str) -> String {
        let mut bytes = tflip_storage::cp1252::encode(text);
        blank_all_but_params(&mut bytes);
        tflip_storage::cp1252::decode(&bytes)
    }

    #[test]
    fn markers_are_counted_as_the_original_counts_them() {
        // `fixtures/sql/params5.json`.
        let named = |name: &str| Some(name.to_owned());
        assert_eq!(
            markers("SELECT ?, :p, :P, :q"),
            [None, named("p"), named("P"), named("q")]
        );
        assert_eq!(markers("SELECT ? WHERE 'a?b' = ? -- ?"), [None, None]);
        assert_eq!(
            markers("SELECT :a:b, :c-d, a::b"),
            [named("a:b"), named("c")]
        );
        assert_eq!(markers("SELECT :'q', :#a"), []);
    }

    #[test]
    fn markers_stay_in_place() {
        // As the original engine rewrites these (fixtures/sql/statements.json).
        let cases = [
            (
                "SELECT * FROM indexed WHERE id = :id AND name = 'a:b' -- :c",
                "                                 :id                       ",
            ),
            (
                "SELECT * FROM t WHERE a = ? /* :x */",
                "                          ?         ",
            ),
            (
                "SELECT\t* FROM \"a:b\" WHERE x=:p1 AND y = [:q] AND z=:_z9 and w = ::v and u = :1",
                "                            :p1                    :_z9                     :1",
            ),
            (
                "SELECT * FROM t WHERE a = :a1b2 AND b = ? AND c = '?' AND d = \"?\"",
                "                          :a1b2         ?                        ",
            ),
            (
                "SELECT * FROM t // :x\r\nWHERE a = :y",
                "                                 :y",
            ),
            (
                "SELECT 'it''s :x' FROM t WHERE a=:b,c=:d)",
                "                                 :b   :d ",
            ),
            (
                "SELECT * FROM t WHERE a = :ä",
                "                            ",
            ),
            ("", ""),
            (":", ":"),
            ("?", "?"),
            ("/* unterminated :x", "                :x"),
        ];
        for (text, expected) in cases {
            assert_eq!(blanked(text), expected, "{text}");
        }
    }
}
