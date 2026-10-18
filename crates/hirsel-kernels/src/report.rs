//! Test report records (LANG-9). Each record is one line whose user text is escaped, so
//! the test runner splits the report at line feeds without any other framing.

/// Appends `text` with `\`, every C0 control and DEL escaped: `\\`, `\n`, `\r`, `\t`,
/// and `\u{XX}` (lowercase hex) for the rest. Every other scalar is copied unchanged.
pub fn escape(text: &str, out: &mut Vec<u8>) {
    for c in text.chars() {
        match c {
            '\\' => out.extend_from_slice(b"\\\\"),
            '\n' => out.extend_from_slice(b"\\n"),
            '\r' => out.extend_from_slice(b"\\r"),
            '\t' => out.extend_from_slice(b"\\t"),
            c if (c as u32) < 0x20 || c == '\u{7f}' => {
                out.extend_from_slice(format!("\\u{{{:02x}}}", c as u32).as_bytes())
            }
            c => {
                let mut buffer = [0; 4];
                out.extend_from_slice(c.encode_utf8(&mut buffer).as_bytes());
            }
        }
    }
}

/// `test log: <escaped text>` and a line feed.
pub fn log(text: &str) -> Vec<u8> {
    let mut out = b"test log: ".to_vec();
    escape(text, &mut out);
    out.push(b'\n');
    out
}

/// A snapshot name is 1 to 64 ASCII letters, digits, `_` or `-`, so it is a portable
/// file name component and never needs escaping.
pub fn snapshot_name(name: &str) -> bool {
    (1..=64).contains(&name.len())
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// `test snapshot <name>: <escaped text>` and a line feed, for a valid name.
pub fn snapshot(name: &str, text: &str) -> Vec<u8> {
    let mut out = format!("test snapshot {name}: ").into_bytes();
    escape(text, &mut out);
    out.push(b'\n');
    out
}

#[cfg(test)]
mod tests {
    #[test]
    fn escapes_controls_backslash_and_keeps_other_scalars() {
        let mut out = Vec::new();
        super::escape("a\\b\nc\rd\te\u{0}\u{1f}\u{7f}é界", &mut out);
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "a\\\\b\\nc\\rd\\te\\u{00}\\u{1f}\\u{7f}é界"
        );
        assert_eq!(super::log(""), b"test log: \n");
        assert_eq!(
            super::snapshot("a-1_B", "x\ny"),
            b"test snapshot a-1_B: x\\ny\n"
        );
    }

    #[test]
    fn snapshot_names_are_bounded_portable_components() {
        for name in ["a", "Z9", "under_score", "dash-ed", &"n".repeat(64)] {
            assert!(super::snapshot_name(name), "{name}");
        }
        for name in ["", "a b", "a.b", "a/b", "a:b", "é", &"n".repeat(65)] {
            assert!(!super::snapshot_name(name), "{name}");
        }
    }
}
