//! Unicode16 data shared with the Hirsel app. ASCII keeps Rust's fast path;
//! non-ASCII uses full, locale-independent mappings and exact L/Nd categories.
include!(concat!(env!("OUT_DIR"), "/unicode16.rs"));
fn scalar(data: &[u8], offset: usize) -> u32 {
    u32::from_be_bytes([0, data[offset], data[offset + 1], data[offset + 2]])
}
fn property(data: &[u8], c: char) -> bool {
    let cp = c as u32;
    let mut low = 0;
    let mut high = data.len() / 6;
    while low < high {
        let mid = (low + high) / 2;
        if cp < scalar(data, mid * 6) {
            high = mid;
        } else if cp > scalar(data, mid * 6 + 3) {
            low = mid + 1;
        } else {
            return true;
        }
    }
    false
}
fn append(out: &mut String, table: &[u8], data: &[u8], c: char) {
    if c.is_ascii() {
        out.push(c.to_ascii_lowercase());
        return;
    }
    let cp = c as u32;
    let mut low = 0;
    let mut high = table.len() / 7;
    while low < high {
        let mid = (low + high) / 2;
        if scalar(table, mid * 7) < cp {
            low = mid + 1;
        } else {
            high = mid;
        }
    }
    if low < table.len() / 7 && scalar(table, low * 7) == cp {
        let offset = scalar(table, low * 7 + 3) as usize;
        for i in 0..usize::from(table[low * 7 + 6]) {
            out.push(char::from_u32(scalar(data, offset + i * 3)).unwrap());
        }
    } else {
        out.push(c);
    }
}
pub fn fold(text: &str) -> String {
    if text.is_ascii() {
        return text.to_ascii_lowercase();
    }
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        append(&mut out, CASEFOLD, CASEFOLD_DATA, c);
    }
    out
}
pub fn lower(text: &str) -> String {
    if text.is_ascii() {
        return text.to_ascii_lowercase();
    }
    let chars: Vec<_> = text.chars().collect();
    let mut preceding = false;
    let mut out = String::with_capacity(text.len());
    for (i, &c) in chars.iter().enumerate() {
        if c == '\u{3a3}' && preceding {
            let following = chars[i + 1..]
                .iter()
                .find(|&&c| !property(CASE_IGNORABLE, c));
            out.push(if following.is_some_and(|&c| property(CASED, c)) {
                '\u{3c3}'
            } else {
                '\u{3c2}'
            });
        } else {
            append(&mut out, FULL_LOWER, FULL_LOWER_DATA, c);
        }
        if !property(CASE_IGNORABLE, c) {
            preceding = property(CASED, c);
        }
    }
    out
}
pub fn tag_char(c: char) -> bool {
    c == '-' || c == '_' || property(LETTERS, c) || property(DIGITS, c)
}
