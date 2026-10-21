//! All offsets name the same old UTF-8 bytes. LSP textDocumentEdit.md:5-20
//! (baf2db31) supplies nonoverlap; digests and ambiguous insertion rejection
//! are the candidate transaction's stronger preconditions.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ByteEdit {
    pub start: u32,
    pub end: u32,
    pub replacement: String,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EditError {
    InvalidRange,
    Utf8Boundary,
    Overlap,
    ResourceLimit,
}

pub fn apply_bytes(text: &str, edits: &[ByteEdit], byte_limit: usize) -> Result<String, EditError> {
    if text.len() > u32::MAX as usize {
        return Err(EditError::ResourceLimit);
    }
    let mut ordered: Vec<_> = edits.iter().collect();
    ordered.sort_by_key(|edit| (edit.start, edit.end));
    let mut length = text.len();
    for edit in &ordered {
        let (start, end) = (edit.start as usize, edit.end as usize);
        if start > end || end > text.len() {
            return Err(EditError::InvalidRange);
        }
        if !text.is_char_boundary(start) || !text.is_char_boundary(end) {
            return Err(EditError::Utf8Boundary);
        }
    }
    for pair in ordered.windows(2) {
        let (a, b) = (pair[0], pair[1]);
        // Sorted insertions precede nonempty replacements at the same start.
        // Equality catches both duplicate inserts and insertion-at-start.
        if a.end > b.start || a.start == b.start {
            return Err(EditError::Overlap);
        }
    }
    for edit in &ordered {
        length = length
            .checked_sub((edit.end - edit.start) as usize)
            .and_then(|n| n.checked_add(edit.replacement.len()))
            .ok_or(EditError::ResourceLimit)?;
    }
    if length > byte_limit || length > u32::MAX as usize {
        return Err(EditError::ResourceLimit);
    }
    let mut result = String::with_capacity(length);
    let mut cursor = 0;
    for edit in ordered {
        result.push_str(&text[cursor..edit.start as usize]);
        result.push_str(&edit.replacement);
        cursor = edit.end as usize;
    }
    result.push_str(&text[cursor..]);
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn edit(start: u32, end: u32, replacement: &str) -> ByteEdit {
        ByteEdit {
            start,
            end,
            replacement: replacement.into(),
        }
    }
    #[test]
    fn utf8_crlf_old_coordinates_and_adjacency() {
        let text = "α\r\nbcd";
        assert_eq!(
            apply_bytes(
                text,
                &[edit(4, 5, "B"), edit(5, 6, ""), edit(6, 6, "X")],
                100
            )
            .unwrap(),
            "α\r\nBXd"
        );
        assert_eq!(
            apply_bytes(text, &[edit(0, 2, "A"), edit(2, 2, "!")], 100).unwrap(),
            "A!\r\nbcd"
        );
        assert_eq!(
            apply_bytes(text, &[edit(1, 2, "A")], 100),
            Err(EditError::Utf8Boundary)
        );
    }
    #[test]
    fn malformed_and_ambiguous_edits_reject_without_partial_content() {
        let text = "abcd";
        for edits in [
            vec![edit(1, 3, "x"), edit(2, 4, "y")],
            vec![edit(1, 1, "x"), edit(1, 1, "y")],
            vec![edit(1, 1, "x"), edit(1, 3, "y")],
            vec![edit(0, 3, "x"), edit(1, 1, "y")],
        ] {
            assert_eq!(apply_bytes(text, &edits, 100), Err(EditError::Overlap));
        }
        assert_eq!(
            apply_bytes(text, &[edit(2, 1, "")], 100),
            Err(EditError::InvalidRange)
        );
        assert_eq!(
            apply_bytes(text, &[edit(1, 9, "")], 100),
            Err(EditError::InvalidRange)
        );
        assert_eq!(
            apply_bytes(text, &[edit(0, 0, "x")], 4),
            Err(EditError::ResourceLimit)
        );
        assert_eq!(text, "abcd");
    }
    #[test]
    fn resulting_content_ignores_order_or_decomposition() {
        let a = [edit(0, 1, "A"), edit(1, 2, "B")];
        let b = [edit(0, 2, "AB")];
        let mut reversed = a.clone();
        reversed.reverse();
        assert_eq!(apply_bytes("abcd", &a, 100), apply_bytes("abcd", &b, 100));
        assert_eq!(
            apply_bytes("abcd", &a, 100),
            apply_bytes("abcd", &reversed, 100)
        );
    }
}
