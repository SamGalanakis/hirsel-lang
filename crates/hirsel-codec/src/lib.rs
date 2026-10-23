//! Deterministic normalized snapshots of raw artifacts.
//!
//! One canonical text encoding serves as the normalized dump, the round-trip
//! format and the digest input. Every value is an S-expression: `(Tag atoms...)`,
//! unsigned and signed decimal integers, quoted strings and `#hex` bytes. The
//! encoding contains only artifact-local indices and stable identities
//! (DefPaths, content hashes); it never contains a `TypeId`, an address or a
//! host-dependent size. Decoding yields raw artifacts only: no validated wrapper
//! or receipt implements [`Snapshot`], so a deserialized artifact must be
//! revalidated.
//!
//! [`Snapshot`], [`Writer`] and [`Reader`] take a context type parameter
//! defaulting to `()`. A context is an encoder/decoder-side table (interned
//! names, span maps); it is never part of the text, so the encoding of a value
//! is identical under every context.

#[cfg(test)]
hirsel_guard::install!();

use hirsel_syntax::{ContentHash, DefPath, ItemPath, PackageId};
use std::fmt::Write as _;
use std::marker::PhantomData;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DecodeError {
    pub offset: usize,
    pub message: String,
}

/// A raw artifact with a canonical encoding under context `C`.
pub trait Snapshot<C = ()>: Sized {
    fn write(&self, w: &mut Writer<C>);
    fn read(r: &mut Reader<'_, C>) -> Result<Self, DecodeError>;
}

/// Canonical text of a raw artifact.
pub fn to_text<T: Snapshot>(value: &T) -> String {
    let mut w = Writer::default();
    value.write(&mut w);
    w.out
}

/// Canonical text of a raw artifact under `context`; returns the context.
pub fn to_text_with<C, T: Snapshot<C>>(value: &T, context: C) -> (String, C) {
    let mut w = Writer::with_context(context);
    value.write(&mut w);
    w.finish()
}

/// Decode a complete raw artifact; trailing input is an error.
pub fn from_text<T: Snapshot>(text: &str) -> Result<T, DecodeError> {
    from_text_with(text, ()).map(|(value, ())| value)
}

/// Decode a complete raw artifact under `context`; trailing input is an error.
pub fn from_text_with<C, T: Snapshot<C>>(text: &str, context: C) -> Result<(T, C), DecodeError> {
    let mut r = Reader::with_context(text, context);
    let value = T::read(&mut r)?;
    let context = r.finish()?;
    Ok((value, context))
}

/// Content digest of the canonical text under a domain and artifact label.
pub fn digest<T: Snapshot>(domain: &str, label: &str, value: &T) -> ContentHash {
    ContentHash::of_parts(domain, [label.as_bytes(), to_text(value).as_bytes()])
}

/// 1-based line and 1-based Unicode-scalar column of a byte offset, for
/// located decode errors.
pub fn line_column(text: &str, offset: usize) -> (u32, u32) {
    let mut line = 1u32;
    let mut column = 1u32;
    for (i, c) in text.char_indices() {
        if i >= offset {
            break;
        }
        if c == '\n' {
            line += 1;
            column = 1;
        } else {
            column += 1;
        }
    }
    (line, column)
}

#[derive(Default)]
pub struct Writer<C = ()> {
    out: String,
    depth: usize,
    discard: bool,
    pub context: C,
}

impl<C> Writer<C> {
    /// An empty writer carrying `context`.
    pub fn with_context(context: C) -> Self {
        Self {
            out: String::new(),
            depth: 0,
            discard: false,
            context,
        }
    }
    /// Visit a snapshot with `context`, without allocating or formatting text.
    /// Snapshot implementations and context observations run exactly as for an
    /// emitting writer. Like serde's `IgnoredAny`, only scalar output is ignored.
    pub fn discarding(context: C) -> Self {
        Self {
            discard: true,
            ..Self::with_context(context)
        }
    }
    /// The written text (empty for a discard writer) and the context.
    pub fn finish(self) -> (String, C) {
        (self.out, self.context)
    }
    fn separate(&mut self) {
        if !self.out.is_empty() && !self.out.ends_with('(') {
            self.out.push(' ');
        }
    }
    pub fn open(&mut self, tag: &str) {
        if self.discard {
            return;
        }
        if self.depth > 0 {
            self.out.push('\n');
            let mut indent = 2 * self.depth;
            while indent > 0 {
                let run = indent.min(INDENT.len());
                self.out.push_str(&INDENT[..run]);
                indent -= run;
            }
        }
        self.out.push('(');
        self.out.push_str(tag);
        self.depth += 1;
    }
    pub fn close(&mut self) {
        if self.discard {
            return;
        }
        self.depth -= 1;
        self.out.push(')');
    }
    pub fn uint(&mut self, n: u128) {
        if self.discard {
            return;
        }
        self.separate();
        push_decimal(&mut self.out, n);
    }
    pub fn int(&mut self, n: i128) {
        if self.discard {
            return;
        }
        self.separate();
        if n < 0 {
            self.out.push('-');
        }
        push_decimal(&mut self.out, n.unsigned_abs());
    }
    pub fn text(&mut self, s: &str) {
        if self.discard {
            return;
        }
        self.separate();
        self.out.push('"');
        for c in s.chars() {
            match c {
                '"' => self.out.push_str("\\\""),
                '\\' => self.out.push_str("\\\\"),
                ' '..='~' => self.out.push(c),
                c => {
                    let _ = write!(self.out, "\\u{{{:x}}}", c as u32);
                }
            }
        }
        self.out.push('"');
    }
    pub fn bytes(&mut self, b: &[u8]) {
        if self.discard {
            return;
        }
        self.separate();
        self.out.push('#');
        push_hex(&mut self.out, b);
    }
}

/// Spaces the writer indents with, taken in runs.
const INDENT: &str = "                                                                ";

/// Append `n` in decimal, as `{n}` formats it, without `core::fmt`.
fn push_decimal(out: &mut String, n: u128) {
    let mut digits = [0u8; 39];
    let mut at = digits.len();
    if let Ok(mut small) = u64::try_from(n) {
        loop {
            at -= 1;
            digits[at] = b'0' + (small % 10) as u8;
            small /= 10;
            if small == 0 {
                break;
            }
        }
    } else {
        let mut big = n;
        while big != 0 {
            at -= 1;
            digits[at] = b'0' + (big % 10) as u8;
            big /= 10;
        }
    }
    // Only ASCII digits were written.
    out.push_str(std::str::from_utf8(&digits[at..]).unwrap_or_default());
}

/// Append `bytes` as lowercase hex pairs, as `{byte:02x}` formats each, by table.
pub fn push_hex(out: &mut String, bytes: &[u8]) {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    out.reserve(2 * bytes.len());
    for &byte in bytes {
        out.push(HEX[usize::from(byte >> 4)] as char);
        out.push(HEX[usize::from(byte & 0xf)] as char);
    }
}

/// The kind of the next value a [`Reader`] would read.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Next {
    Open,
    Close,
    Text,
    Bytes,
    Integer,
    End,
}

pub struct Reader<'a, C = ()> {
    text: &'a str,
    pos: usize,
    pub context: C,
}

impl<'a> Reader<'a, ()> {
    /// A reader over `text` with unit context.
    pub fn new(text: &'a str) -> Self {
        Self::with_context(text, ())
    }
}

impl<'a, C> Reader<'a, C> {
    /// A reader over `text` carrying `context`.
    pub fn with_context(text: &'a str, context: C) -> Self {
        Self {
            text,
            pos: 0,
            context,
        }
    }
    /// Current byte offset into the input.
    pub fn offset(&self) -> usize {
        self.pos
    }
    /// Bytes of input not yet consumed.
    pub fn remaining(&self) -> usize {
        self.text.len() - self.pos
    }
    /// End-of-input check; returns the context. Trailing input is an error.
    pub fn finish(mut self) -> Result<C, DecodeError> {
        self.skip_space();
        if self.pos != self.text.len() {
            return Err(self.error("trailing input"));
        }
        Ok(self.context)
    }
    pub fn error(&self, message: impl Into<String>) -> DecodeError {
        DecodeError {
            offset: self.pos,
            message: message.into(),
        }
    }
    fn skip_space(&mut self) {
        let rest = &self.text[self.pos..];
        self.pos += rest.len() - rest.trim_start().len();
    }
    fn peek(&mut self) -> Option<u8> {
        self.skip_space();
        self.text.as_bytes().get(self.pos).copied()
    }
    fn word(&mut self) -> &'a str {
        let rest = &self.text[self.pos..];
        let end = rest
            .find(|c: char| c.is_whitespace() || c == '(' || c == ')')
            .unwrap_or(rest.len());
        self.pos += end;
        &rest[..end]
    }
    /// What the next value is, without consuming it, for readers that walk
    /// a text without knowing its schema.
    pub fn next_kind(&mut self) -> Next {
        match self.peek() {
            None => Next::End,
            Some(b'(') => Next::Open,
            Some(b')') => Next::Close,
            Some(b'"') => Next::Text,
            Some(b'#') => Next::Bytes,
            Some(_) => Next::Integer,
        }
    }
    /// Open a node and return its tag.
    pub fn open_any(&mut self) -> Result<&'a str, DecodeError> {
        if self.peek() != Some(b'(') {
            return Err(self.error("expected '('"));
        }
        self.pos += 1;
        let tag = self.word();
        if tag.is_empty() {
            return Err(self.error("empty tag"));
        }
        Ok(tag)
    }
    pub fn open(&mut self, tag: &str) -> Result<(), DecodeError> {
        let start = self.pos;
        let found = self.open_any()?;
        if found != tag {
            self.pos = start;
            return Err(self.error(format!("expected ({tag}, found ({found}")));
        }
        Ok(())
    }
    pub fn at_close(&mut self) -> bool {
        self.peek() == Some(b')')
    }
    pub fn close(&mut self) -> Result<(), DecodeError> {
        if self.peek() != Some(b')') {
            return Err(self.error("expected ')'"));
        }
        self.pos += 1;
        Ok(())
    }
    pub fn uint(&mut self) -> Result<u128, DecodeError> {
        self.skip_space();
        let word = self.word();
        if word.is_empty() || !word.bytes().all(|b| b.is_ascii_digit()) {
            return Err(self.error(format!("expected unsigned integer, found {word:?}")));
        }
        word.parse().map_err(|_| self.error("integer out of range"))
    }
    pub fn int(&mut self) -> Result<i128, DecodeError> {
        self.skip_space();
        let word = self.word();
        let digits = word.strip_prefix('-').unwrap_or(word);
        if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
            return Err(self.error(format!("expected integer, found {word:?}")));
        }
        word.parse().map_err(|_| self.error("integer out of range"))
    }
    pub fn text(&mut self) -> Result<String, DecodeError> {
        if self.peek() != Some(b'"') {
            return Err(self.error("expected string"));
        }
        self.pos += 1;
        let mut out = String::new();
        let mut chars = self.text[self.pos..].char_indices();
        while let Some((i, c)) = chars.next() {
            match c {
                '"' => {
                    self.pos += i + 1;
                    return Ok(out);
                }
                '\\' => match chars.next() {
                    Some((_, '"')) => out.push('"'),
                    Some((_, '\\')) => out.push('\\'),
                    Some((_, 'u')) => {
                        let mut hex = String::new();
                        if chars.next().map(|p| p.1) != Some('{') {
                            return Err(self.error("bad escape"));
                        }
                        loop {
                            match chars.next() {
                                Some((_, '}')) => break,
                                Some((_, h)) => hex.push(h),
                                None => return Err(self.error("unterminated escape")),
                            }
                        }
                        let c = u32::from_str_radix(&hex, 16)
                            .ok()
                            .and_then(char::from_u32)
                            .ok_or_else(|| self.error("bad escape"))?;
                        out.push(c);
                    }
                    _ => return Err(self.error("bad escape")),
                },
                c => out.push(c),
            }
        }
        Err(self.error("unterminated string"))
    }
    pub fn bytes(&mut self) -> Result<Vec<u8>, DecodeError> {
        if self.peek() != Some(b'#') {
            return Err(self.error("expected bytes"));
        }
        self.pos += 1;
        let word = self.word();
        if !word.len().is_multiple_of(2) {
            return Err(self.error("odd hex length"));
        }
        (0..word.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&word[i..i + 2], 16).map_err(|_| self.error("bad hex")))
            .collect()
    }
}

/// `snapshot_struct!(Name { a, b })`: a node tagged with the type name, fields in order.
///
/// `snapshot_struct!(Name { a, b } in Ctx)` implements `Snapshot<Ctx>` instead
/// of `Snapshot<()>`.
#[macro_export]
macro_rules! snapshot_struct {
    ($ty:ident { $($field:ident),* $(,)? } trailing_optional $optional:ident trailing_empty $pool:ident) => {
        impl $crate::Snapshot for $ty {
            fn write(&self, w: &mut $crate::Writer) {
                w.open(stringify!($ty));
                $($crate::Snapshot::write(&self.$field, w);)*
                if self.$optional.is_some() || !self.$pool.is_empty() {
                    $crate::Snapshot::write(&self.$optional, w);
                }
                if !self.$pool.is_empty() { $crate::Snapshot::write(&self.$pool, w); }
                w.close();
            }
            fn read(r: &mut $crate::Reader<'_>) -> Result<Self, $crate::DecodeError> {
                r.open(stringify!($ty))?;
                $(let $field = $crate::Snapshot::read(r)?;)*
                let $optional = if r.next_kind() == $crate::Next::Close { None }
                    else { $crate::Snapshot::read(r)? };
                let $pool = if r.next_kind() == $crate::Next::Close { Default::default() }
                    else { $crate::Snapshot::read(r)? };
                r.close()?;
                Ok(Self { $($field,)* $optional, $pool })
            }
        }
    };
    // A new empty operand pool preserves the original record's bytes.
    ($ty:ident { $($field:ident),* $(,)? } trailing_empty $pool:ident) => {
        impl $crate::Snapshot for $ty {
            fn write(&self, w: &mut $crate::Writer) {
                w.open(stringify!($ty));
                $($crate::Snapshot::write(&self.$field, w);)*
                if !self.$pool.is_empty() { $crate::Snapshot::write(&self.$pool, w); }
                w.close();
            }
            fn read(r: &mut $crate::Reader<'_>) -> Result<Self, $crate::DecodeError> {
                r.open(stringify!($ty))?;
                $(let $field = $crate::Snapshot::read(r)?;)*
                let $pool = if r.next_kind() == $crate::Next::Close {
                    Default::default()
                } else { $crate::Snapshot::read(r)? };
                r.close()?;
                Ok(Self { $($field,)* $pool })
            }
        }
    };

    // An absent additive contract has no wire footprint. Existing fields keep
    // their exact order and bytes; a present contract is an optional trailer.
    ($ty:ident { $($field:ident),* $(,)? } trailing_optional $optional:ident) => {
        impl $crate::Snapshot for $ty {
            fn write(&self, w: &mut $crate::Writer) {
                w.open(stringify!($ty));
                $($crate::Snapshot::write(&self.$field, w);)*
                if self.$optional.is_some() {
                    $crate::Snapshot::write(&self.$optional, w);
                }
                w.close();
            }
            fn read(r: &mut $crate::Reader<'_>) -> Result<Self, $crate::DecodeError> {
                r.open(stringify!($ty))?;
                $(let $field = $crate::Snapshot::read(r)?;)*
                let $optional = if r.next_kind() == $crate::Next::Close {
                    None
                } else {
                    $crate::Snapshot::read(r)?
                };
                r.close()?;
                Ok(Self { $($field,)* $optional })
            }
        }
    };
    ($ty:ident { $($field:ident),* $(,)? }) => {
        $crate::snapshot_struct!($ty { $($field),* } in ());
    };
    ($ty:ident { $($field:ident),* $(,)? } in $ctx:ty) => {
        impl $crate::Snapshot<$ctx> for $ty {
            fn write(&self, w: &mut $crate::Writer<$ctx>) {
                w.open(stringify!($ty));
                $($crate::Snapshot::<$ctx>::write(&self.$field, w);)*
                w.close();
            }
            fn read(r: &mut $crate::Reader<'_, $ctx>) -> Result<Self, $crate::DecodeError> {
                r.open(stringify!($ty))?;
                let value = $ty { $($field: $crate::Snapshot::<$ctx>::read(r)?,)* };
                r.close()?;
                Ok(value)
            }
        }
    };
}

/// `snapshot_enum!(Name { Unit, Tuple(a, b), Struct { x, y } })`: one node per variant.
///
/// `snapshot_enum!(Name { ... } in Ctx)` implements `Snapshot<Ctx>` instead of
/// `Snapshot<()>`.
#[macro_export]
macro_rules! snapshot_enum {
    ($ty:ident { $($variant:ident $( { $($field:ident),* $(,)? } )? $( ( $($tuple:ident),* $(,)? ) )? ),* $(,)? }) => {
        $crate::snapshot_enum!($ty { $($variant $({ $($field),* })? $( ( $($tuple),* ) )? ),* } in ());
    };
    ($ty:ident { $($variant:ident $( { $($field:ident),* $(,)? } )? $( ( $($tuple:ident),* $(,)? ) )? ),* $(,)? } in $ctx:ty) => {
        impl $crate::Snapshot<$ctx> for $ty {
            #[allow(unused_variables)]
            fn write(&self, w: &mut $crate::Writer<$ctx>) {
                match self {
                    $(Self::$variant $({ $($field),* })? $(( $($tuple),* ))? => {
                        w.open(stringify!($variant));
                        $($($crate::Snapshot::<$ctx>::write($field, w);)*)?
                        $($($crate::Snapshot::<$ctx>::write($tuple, w);)*)?
                        w.close();
                    })*
                }
            }
            fn read(r: &mut $crate::Reader<'_, $ctx>) -> Result<Self, $crate::DecodeError> {
                let start = r.open_any()?;
                let value = match start {
                    $(stringify!($variant) => Self::$variant
                        $({ $($field: $crate::Snapshot::<$ctx>::read(r)?),* })?
                        $(( $({ let $tuple = $crate::Snapshot::<$ctx>::read(r)?; $tuple }),* ))?,)*
                    other => return Err(r.error(format!(concat!("unknown ", stringify!($ty), " variant {}"), other))),
                };
                r.close()?;
                Ok(value)
            }
        }
    };
}

macro_rules! unsigned {
    ($($t:ty),*) => {$(
        impl<C> Snapshot<C> for $t {
            fn write(&self, w: &mut Writer<C>) {
                w.uint(*self as u128);
            }
            fn read(r: &mut Reader<'_, C>) -> Result<Self, DecodeError> {
                let n = r.uint()?;
                <$t>::try_from(n).map_err(|_| r.error(concat!("out of range for ", stringify!($t))))
            }
        }
    )*};
}
unsigned!(u8, u16, u32, u64, u128);

impl<C> Snapshot<C> for i64 {
    fn write(&self, w: &mut Writer<C>) {
        w.int(*self as i128);
    }
    fn read(r: &mut Reader<'_, C>) -> Result<Self, DecodeError> {
        let n = r.int()?;
        i64::try_from(n).map_err(|_| r.error("out of range for i64"))
    }
}

impl<C> Snapshot<C> for bool {
    fn write(&self, w: &mut Writer<C>) {
        w.uint(u128::from(*self));
    }
    fn read(r: &mut Reader<'_, C>) -> Result<Self, DecodeError> {
        match r.uint()? {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err(r.error("expected 0 or 1")),
        }
    }
}

impl<C> Snapshot<C> for String {
    fn write(&self, w: &mut Writer<C>) {
        w.text(self);
    }
    fn read(r: &mut Reader<'_, C>) -> Result<Self, DecodeError> {
        r.text()
    }
}

impl<C, T: Snapshot<C>> Snapshot<C> for Vec<T> {
    fn write(&self, w: &mut Writer<C>) {
        w.open("list");
        for item in self {
            item.write(w);
        }
        w.close();
    }
    fn read(r: &mut Reader<'_, C>) -> Result<Self, DecodeError> {
        r.open("list")?;
        let mut items = Vec::new();
        while !r.at_close() {
            items.push(T::read(r)?);
        }
        r.close()?;
        Ok(items)
    }
}

impl<C, T: Snapshot<C>> Snapshot<C> for Option<T> {
    fn write(&self, w: &mut Writer<C>) {
        match self {
            None => {
                w.open("none");
                w.close();
            }
            Some(value) => {
                w.open("some");
                value.write(w);
                w.close();
            }
        }
    }
    fn read(r: &mut Reader<'_, C>) -> Result<Self, DecodeError> {
        let value = match r.open_any()? {
            "none" => None,
            "some" => Some(T::read(r)?),
            other => return Err(r.error(format!("expected option, found {other}"))),
        };
        r.close()?;
        Ok(value)
    }
}

impl<C, A: Snapshot<C>, B: Snapshot<C>> Snapshot<C> for (A, B) {
    fn write(&self, w: &mut Writer<C>) {
        w.open("pair");
        self.0.write(w);
        self.1.write(w);
        w.close();
    }
    fn read(r: &mut Reader<'_, C>) -> Result<Self, DecodeError> {
        r.open("pair")?;
        let value = (A::read(r)?, B::read(r)?);
        r.close()?;
        Ok(value)
    }
}

impl<C, T: Snapshot<C>> Snapshot<C> for Box<T> {
    fn write(&self, w: &mut Writer<C>) {
        (**self).write(w);
    }
    fn read(r: &mut Reader<'_, C>) -> Result<Self, DecodeError> {
        T::read(r).map(Box::new)
    }
}

impl<C, T> Snapshot<C> for PhantomData<T> {
    fn write(&self, _: &mut Writer<C>) {}
    fn read(_: &mut Reader<'_, C>) -> Result<Self, DecodeError> {
        Ok(PhantomData)
    }
}

impl<C> Snapshot<C> for ContentHash {
    fn write(&self, w: &mut Writer<C>) {
        w.bytes(&self.0);
    }
    fn read(r: &mut Reader<'_, C>) -> Result<Self, DecodeError> {
        let bytes = r.bytes()?;
        Ok(ContentHash(
            bytes
                .try_into()
                .map_err(|_| r.error("content hash is not 32 bytes"))?,
        ))
    }
}

/// Encoder-side observation of the stable identities a value names. Contexts
/// that only carry tables keep the empty default.
pub trait Observe {
    fn def_path(&mut self, _path: &DefPath) {}
    /// The text written in place of `path`, for an encoding that identifies
    /// definitions by something other than their names. Such text is written
    /// as a `(Ref "...")` node and is never decoded.
    fn reference(&mut self, _path: &DefPath) -> Option<String> {
        None
    }
}
impl Observe for () {}

impl<C: Observe> Snapshot<C> for DefPath {
    fn write(&self, w: &mut Writer<C>) {
        w.context.def_path(self);
        if let Some(reference) = w.context.reference(self) {
            w.open("Ref");
            w.text(&reference);
            w.close();
            return;
        }
        let p = &self.0;
        w.open("DefPath");
        w.text(&p.package.name);
        w.text(&p.package.version);
        p.modules.write(w);
        w.text(&p.name);
        w.close();
    }
    fn read(r: &mut Reader<'_, C>) -> Result<Self, DecodeError> {
        r.open("DefPath")?;
        let name = r.text()?;
        let version = r.text()?;
        let modules = Vec::<String>::read(r)?;
        let item = r.text()?;
        r.close()?;
        Ok(DefPath(ItemPath {
            package: PackageId { name, version },
            modules,
            name: item,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, PartialEq)]
    struct Point {
        x: u32,
        label: String,
    }
    snapshot_struct!(Point { x, label });

    #[derive(Debug, PartialEq)]
    enum Expr {
        Lit(i64),
        Pair(Vec<u32>, u8),
        Named { name: String, value: u32 },
        Empty,
    }
    snapshot_enum!(Expr {
        Lit(v),
        Pair(items, tag),
        Named { name, value },
        Empty,
    });

    #[test]
    fn primitives_round_trip() {
        assert_eq!(from_text::<u8>(&to_text(&255u8)).unwrap(), 255);
        assert_eq!(from_text::<u64>(&to_text(&u64::MAX)).unwrap(), u64::MAX);
        assert_eq!(from_text::<u128>(&to_text(&u128::MAX)).unwrap(), u128::MAX);
        assert_eq!(from_text::<i64>(&to_text(&-42i64)).unwrap(), -42);
        assert_eq!(from_text::<i64>(&to_text(&i64::MIN)).unwrap(), i64::MIN);
        assert!(from_text::<bool>(&to_text(&true)).unwrap());
        assert!(!from_text::<bool>(&to_text(&false)).unwrap());
        assert_eq!(to_text(&7u32), "7");
        assert_eq!(to_text(&-7i64), "-7");
    }

    // Go encoding/hex/hex_test.go:21-51: canonical vectors and append semantics.
    #[test]
    fn hex_matches_reference_vectors_and_every_byte() {
        for (expected, bytes) in [
            ("", &[][..]),
            ("0001020304050607", &[0, 1, 2, 3, 4, 5, 6, 7][..]),
            ("08090a0b0c0d0e0f", &[8, 9, 10, 11, 12, 13, 14, 15][..]),
            (
                "f0f1f2f3f4f5f6f7",
                &[0xf0, 0xf1, 0xf2, 0xf3, 0xf4, 0xf5, 0xf6, 0xf7][..],
            ),
            (
                "f8f9fafbfcfdfeff",
                &[0xf8, 0xf9, 0xfa, 0xfb, 0xfc, 0xfd, 0xfe, 0xff][..],
            ),
            ("67", b"g".as_slice()),
            ("e3a1", &[0xe3, 0xa1][..]),
        ] {
            let mut appended = String::from("lead");
            push_hex(&mut appended, bytes);
            assert_eq!(appended, format!("lead{expected}"));
        }
        let bytes: Vec<u8> = (0..=255).collect();
        let expected: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
        let mut writer = Writer::default();
        writer.bytes(&bytes);
        writer.bytes(&[]);
        let (text, ()) = writer.finish();
        assert_eq!(text, format!("#{expected} #"));
        let mut reader = Reader::new(&text);
        assert_eq!(reader.bytes().unwrap(), bytes);
        assert!(reader.bytes().unwrap().is_empty());
        reader.finish().unwrap();
    }

    #[test]
    fn integer_writer_matches_fmt_at_width_and_decimal_boundaries() {
        let mut values = vec![0, 1, u64::MAX as u128, u64::MAX as u128 + 1, u128::MAX];
        let mut power = 10u128;
        loop {
            values.extend([power - 1, power, power + 1]);
            let Some(next) = power.checked_mul(10) else {
                break;
            };
            power = next;
        }
        for value in values {
            let mut writer = Writer::default();
            writer.uint(value);
            let (text, ()) = writer.finish();
            assert_eq!(text, format!("{value}"));
            assert_eq!(from_text::<u128>(&text).unwrap(), value);
            if let Ok(signed) = i128::try_from(value) {
                for signed in [signed, -signed] {
                    let mut writer = Writer::default();
                    writer.int(signed);
                    let (text, ()) = writer.finish();
                    assert_eq!(text, format!("{signed}"));
                }
            }
        }
        for signed in [i128::MIN, i128::MIN + 1, i128::MAX] {
            let mut writer = Writer::default();
            writer.int(signed);
            let (text, ()) = writer.finish();
            assert_eq!(text, format!("{signed}"));
        }
    }

    #[test]
    fn indentation_matches_canonical_text_across_multiple_runs() {
        let mut writer = Writer::default();
        let mut expected = String::new();
        for depth in 0..100 {
            if depth != 0 {
                expected.push('\n');
                expected.push_str(&"  ".repeat(depth));
            }
            expected.push_str("(node");
            writer.open("node");
        }
        writer.uint(0);
        expected.push_str(" 0");
        for _ in 0..100 {
            writer.close();
            expected.push(')');
        }
        let (text, ()) = writer.finish();
        assert_eq!(text, expected);
        let mut reader = Reader::new(&text);
        for _ in 0..100 {
            reader.open("node").unwrap();
        }
        assert_eq!(reader.uint().unwrap(), 0);
        for _ in 0..100 {
            reader.close().unwrap();
        }
        reader.finish().unwrap();
    }

    #[test]
    fn primitive_decode_errors() {
        assert_eq!(
            from_text::<u8>("999").unwrap_err().message,
            "out of range for u8"
        );
        assert_eq!(
            from_text::<bool>("2").unwrap_err().message,
            "expected 0 or 1"
        );
        assert!(from_text::<u32>("-1").is_err());
        assert!(from_text::<u32>("x").is_err());
    }

    #[test]
    fn strings_escape_and_round_trip() {
        let s = "é \"quoted\" \\ \u{1F600} plain~";
        let text = to_text(&s.to_string());
        assert_eq!(text, "\"\\u{e9} \\\"quoted\\\" \\\\ \\u{1f600} plain~\"");
        assert_eq!(from_text::<String>(&text).unwrap(), s);
        assert!(from_text::<String>("\"unterminated").is_err());
        assert!(from_text::<String>("\"bad \\q\"").is_err());
    }

    #[test]
    fn bytes_and_content_hash_round_trip() {
        let mut w = Writer::default();
        w.bytes(&[0xde, 0xad, 0xbe, 0xef]);
        let (text, ()) = w.finish();
        assert_eq!(text, "#deadbeef");
        let mut r = Reader::new(&text);
        assert_eq!(r.bytes().unwrap(), vec![0xde, 0xad, 0xbe, 0xef]);
        assert!(from_text::<ContentHash>("#abc").is_err()); // odd hex
        assert!(from_text::<ContentHash>("#00").is_err()); // not 32 bytes
        let hash = ContentHash::of_bytes(b"fixture");
        assert_eq!(from_text::<ContentHash>(&to_text(&hash)).unwrap(), hash);
    }

    #[test]
    fn composites_round_trip() {
        let value: (Vec<u32>, Option<String>) = (vec![1, 2, 3], Some("x".into()));
        let text = to_text(&value);
        assert!(text.starts_with("(pair"));
        let back: (Vec<u32>, Option<String>) = from_text(&text).unwrap();
        assert_eq!(back, value);
        assert_eq!(to_text(&PhantomData::<u8>), "");
        assert_eq!(from_text::<PhantomData<u8>>("").unwrap(), PhantomData);
        let none: Option<u32> = None;
        assert_eq!(to_text(&none), "(none)");
        assert_eq!(from_text::<Option<u32>>("(none)").unwrap(), None);
        assert_eq!(
            from_text::<Option<u32>>("(bogus)").unwrap_err().message,
            "expected option, found bogus"
        );
    }

    #[test]
    fn struct_and_enum_macros_round_trip() {
        let point = Point {
            x: 3,
            label: "p".into(),
        };
        assert_eq!(to_text(&point), "(Point 3 \"p\")");
        assert_eq!(from_text::<Point>(&to_text(&point)).unwrap(), point);

        let expr = Expr::Named {
            name: "n".into(),
            value: 9,
        };
        let text = to_text(&expr);
        assert!(text.contains("(Named \"n\" 9)"), "{text}");
        assert_eq!(from_text::<Expr>(&text).unwrap(), expr);
        let nested = Expr::Pair(vec![1, 2], 2);
        assert_eq!(from_text::<Expr>(&to_text(&nested)).unwrap(), nested);
        assert_eq!(from_text::<Expr>("(Empty)").unwrap(), Expr::Empty);
        let error = from_text::<Expr>("(Bogus)").unwrap_err();
        assert!(error.message.contains("unknown Expr variant"), "{error:?}");
    }

    // A context the encoder consults: names encode as text, indexes decode
    // through the table.
    #[derive(Debug)]
    struct Names(Vec<String>);

    #[derive(Debug, PartialEq)]
    struct Alias {
        name_ix: NameIx,
    }
    snapshot_struct!(Alias { name_ix } in Names);

    #[derive(Debug, PartialEq)]
    enum AliasKind {
        Plain(NameIx),
        Other,
    }
    snapshot_enum!(AliasKind { Plain(n), Other } in Names);

    #[derive(Debug, PartialEq)]
    struct NameIx(u32);
    impl Snapshot<Names> for NameIx {
        fn write(&self, w: &mut Writer<Names>) {
            let name = w.context.0[self.0 as usize].clone();
            w.text(&name);
        }
        fn read(r: &mut Reader<'_, Names>) -> Result<Self, DecodeError> {
            let name = r.text()?;
            r.context
                .0
                .iter()
                .position(|n| *n == name)
                .map(|i| NameIx(i as u32))
                .ok_or_else(|| r.error("unknown name"))
        }
    }

    #[test]
    fn discard_writer_preserves_observations_without_allocating_text() {
        #[derive(Default)]
        struct Paths(Vec<DefPath>);
        impl Observe for Paths {
            fn def_path(&mut self, path: &DefPath) {
                self.0.push(path.clone());
            }
        }
        let value = vec![DefPath::core("B"), DefPath::core("A"), DefPath::core("B")];
        let mut emit = Writer::with_context(Paths::default());
        value.write(&mut emit);
        let mut discard = Writer::discarding(Paths::default());
        value.write(&mut discard);
        discard.uint(u128::MAX);
        discard.int(i128::MIN);
        discard.text("\u{0}\n🦀");
        discard.bytes(&[0, 255]);
        assert_eq!(discard.context.0, emit.context.0);
        assert_eq!(discard.context.0, value);
        assert_eq!(discard.out.capacity(), 0);
        assert!(discard.finish().0.is_empty());
        assert!(!emit.finish().0.is_empty());
    }

    #[test]
    fn context_macros_and_round_trip() {
        let names = || Names(vec!["a".into(), "b".into()]);
        let value = Alias { name_ix: NameIx(1) };
        let (text, returned) = to_text_with(&value, names());
        assert_eq!(text, "(Alias \"b\")");
        assert_eq!(returned.0, vec!["a".to_string(), "b".to_string()]);

        let (back, _) = from_text_with::<Names, Alias>(&text, names()).unwrap();
        assert_eq!(back, value);

        let kind = AliasKind::Plain(NameIx(0));
        let (text, _) = to_text_with(&kind, names());
        assert_eq!(text, "(Plain \"a\")");
        let (back, _) = from_text_with::<Names, AliasKind>(&text, names()).unwrap();
        assert_eq!(back, kind);

        let error = from_text_with::<Names, Alias>("(Alias \"z\")", names()).unwrap_err();
        assert_eq!(error.message, "unknown name");
    }

    #[test]
    fn reader_finish_rejects_trailing_input_and_returns_context() {
        let mut r = Reader::with_context("(x) tail", 7u8);
        r.open("x").unwrap();
        r.close().unwrap();
        assert!(r.finish().is_err_and(|e| e.message == "trailing input"));

        let mut r = Reader::with_context("(x)  ", 7u8);
        r.open("x").unwrap();
        r.close().unwrap();
        assert_eq!(r.finish().unwrap(), 7);
    }

    #[test]
    fn line_column_is_one_based_on_scalars() {
        assert_eq!(line_column("ab\ncd\nef", 0), (1, 1));
        assert_eq!(line_column("ab\ncd\nef", 4), (2, 2));
        assert_eq!(line_column("ab\ncd\nef", 6), (3, 1));
        // 'é' is two bytes and one scalar: the 'x' after it is column 2.
        assert_eq!(line_column("é\néx", 5), (2, 2));
        assert_eq!(line_column("é\néx", 3), (2, 1));
    }

    #[test]
    fn absent_optional_trailer_keeps_legacy_bytes_and_present_empty_is_distinct() {
        #[derive(Debug, PartialEq)]
        struct Additive {
            base: u32,
            contract: Option<Vec<u32>>,
        }
        snapshot_struct!(Additive { base } trailing_optional contract);
        let ordinary = Additive {
            base: 7,
            contract: None,
        };
        assert_eq!(to_text(&ordinary), "(Additive 7)");
        assert_eq!(from_text::<Additive>("(Additive 7)").unwrap(), ordinary);
        let empty = Additive {
            base: 7,
            contract: Some(vec![]),
        };
        let text = to_text(&empty);
        assert_ne!(text, to_text(&ordinary));
        assert_eq!(from_text::<Additive>(&text).unwrap(), empty);
        let extra = format!("{} (None))", text.strip_suffix(')').unwrap());
        assert!(from_text::<Additive>(&extra).is_err());
    }

    #[test]
    fn digest_binds_domain_label_and_text() {
        let value = 5u32;
        let expected = ContentHash::of_parts("dom", [b"label".as_slice(), b"5"]);
        assert_eq!(digest("dom", "label", &value), expected);
        assert_ne!(digest("dom", "other", &value), expected);
        assert_ne!(digest("other", "label", &value), expected);
    }
}
