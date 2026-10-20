//! A canonical codec envelope as text, for tools that show recorded values (a runner call's
//! outcome in `hirsel tapes trial`). The envelope's own descriptor names every record field,
//! variant arm and nominal type (docs/spec/serialization.md, Concrete schema descriptor),
//! so no program is needed: `Ok(Receipt(from: 1, to: 2, moved: 200))`. Every read is
//! bounds-checked and nesting is bounded, so malformed or hostile bytes give an error,
//! never a panic.

/// How deep values nest, and how long their text grows, before rendering refuses them.
const DEPTH: usize = 128;
const TEXT: usize = 1 << 20;

enum Node {
    Scalar(u8),
    Tuple(Vec<u32>),
    Record(Vec<(String, u32)>),
    Variant(Vec<Arm>),
    Box(u32),
    Builtin(u8, Vec<Argument>),
    Nominal(String, u32),
}
/// A variant arm: its name and its fields, each with its label if it has one.
type Arm = (String, Vec<(Option<String>, u32)>);
enum Argument {
    Type(u32),
    Size(u64),
}

struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}
impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], String> {
        let end = self
            .at
            .checked_add(n)
            .filter(|&end| end <= self.bytes.len());
        let end = end.ok_or("truncated")?;
        let taken = &self.bytes[self.at..end];
        self.at = end;
        Ok(taken)
    }
    fn byte(&mut self) -> Result<u8, String> {
        Ok(self.take(1)?[0])
    }
    fn u32(&mut self) -> Result<u32, String> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    fn u64(&mut self) -> Result<u64, String> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }
    fn length(&mut self) -> Result<usize, String> {
        usize::try_from(self.u64()?).map_err(|_| "length".into())
    }
    fn text(&mut self) -> Result<String, String> {
        let n = self.length()?;
        String::from_utf8(self.take(n)?.to_vec()).map_err(|_| "utf-8".into())
    }
    fn done(&self) -> bool {
        self.at == self.bytes.len()
    }
}

/// The value of one complete envelope as text.
pub fn render(envelope: &[u8]) -> Result<String, String> {
    let mut r = Reader {
        bytes: envelope,
        at: 0,
    };
    if r.take(4)? != b"HSC1" || r.u32()? != 1 {
        return Err("not a version 1 codec envelope".into());
    }
    let n = r.length()?;
    let nodes = descriptor(r.take(n)?)?;
    let n = r.length()?;
    let mut payload = Reader {
        bytes: r.take(n)?,
        at: 0,
    };
    if !r.done() {
        return Err("trailing envelope bytes".into());
    }
    let mut out = String::new();
    value(&nodes, 0, &mut payload, &mut out, 0)?;
    if !payload.done() {
        return Err("trailing payload bytes".into());
    }
    Ok(out)
}

fn descriptor(bytes: &[u8]) -> Result<Vec<Node>, String> {
    let mut r = Reader { bytes, at: 0 };
    if r.u32()? != 1 || r.u32()? != 0 {
        return Err("not a version 1 descriptor".into());
    }
    let count = r.u32()?;
    let mut nodes = vec![];
    for _ in 0..count {
        let node = match r.byte()? {
            0 => Node::Scalar(r.byte()?),
            1 => Node::Tuple((0..r.u32()?).map(|_| r.u32()).collect::<Result<_, _>>()?),
            2 => Node::Record(
                (0..r.u32()?)
                    .map(|_| Ok((r.text()?, r.u32()?)))
                    .collect::<Result<_, String>>()?,
            ),
            3 => Node::Variant(
                (0..r.u32()?)
                    .map(|_| {
                        let name = r.text()?;
                        let fields = (0..r.u32()?)
                            .map(|_| {
                                let label = match r.byte()? {
                                    0 => None,
                                    1 => Some(r.text()?),
                                    _ => return Err("field tag".to_string()),
                                };
                                Ok((label, r.u32()?))
                            })
                            .collect::<Result<_, String>>()?;
                        Ok((name, fields))
                    })
                    .collect::<Result<_, String>>()?,
            ),
            4 => Node::Box(r.u32()?),
            5 => {
                let kind = r.byte()?;
                let arguments = (0..r.u32()?)
                    .map(|_| match r.byte()? {
                        0 => Ok(Argument::Type(r.u32()?)),
                        1 => Ok(Argument::Size(r.u64()?)),
                        _ => Err("argument tag".to_string()),
                    })
                    .collect::<Result<_, String>>()?;
                Node::Builtin(kind, arguments)
            }
            6 => {
                let _package = r.text()?;
                let _version = r.text()?;
                for _ in 0..r.u32()? {
                    r.text()?;
                }
                Node::Nominal(r.text()?, r.u32()?)
            }
            _ => return Err("node tag".into()),
        };
        nodes.push(node);
    }
    if !r.done() || nodes.is_empty() {
        return Err("malformed descriptor".into());
    }
    Ok(nodes)
}

fn value(
    nodes: &[Node],
    index: u32,
    r: &mut Reader<'_>,
    out: &mut String,
    depth: usize,
) -> Result<(), String> {
    if depth > DEPTH {
        return Err("nested too deeply".into());
    }
    let node = nodes.get(index as usize).ok_or("child index")?;
    let child =
        |index: u32, r: &mut Reader<'_>, out: &mut String| value(nodes, index, r, out, depth + 1);
    match node {
        Node::Scalar(kind) => scalar(*kind, r, out)?,
        Node::Tuple(children) => {
            out.push('(');
            for (i, &c) in children.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                child(c, r, out)?;
            }
            out.push(')');
        }
        Node::Record(fields) => {
            out.push('(');
            for (i, (name, c)) in fields.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                out.push_str(name);
                out.push_str(": ");
                child(*c, r, out)?;
            }
            out.push(')');
        }
        Node::Variant(arms) => {
            let (name, fields) = arms.get(r.u32()? as usize).ok_or("variant ordinal")?;
            out.push_str(name);
            out.push('(');
            for (i, (label, c)) in fields.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                if let Some(label) = label {
                    out.push_str(label);
                    out.push_str(": ");
                }
                child(*c, r, out)?;
            }
            out.push(')');
        }
        Node::Box(c) => child(*c, r, out)?,
        Node::Nominal(name, c) => match nodes.get(*c as usize) {
            // A variant's arms name themselves; a record or a distinct value takes its
            // type's name.
            Some(Node::Variant(_) | Node::Nominal(..) | Node::Box(_)) => child(*c, r, out)?,
            Some(Node::Record(_)) => {
                out.push_str(name);
                child(*c, r, out)?;
            }
            _ => {
                out.push_str(name);
                out.push('(');
                child(*c, r, out)?;
                out.push(')');
            }
        },
        Node::Builtin(kind, arguments) => {
            let types: Vec<u32> = arguments
                .iter()
                .filter_map(|a| match a {
                    Argument::Type(t) => Some(*t),
                    Argument::Size(_) => None,
                })
                .collect();
            let size = arguments.iter().find_map(|a| match a {
                Argument::Size(n) => Some(*n),
                Argument::Type(_) => None,
            });
            match (kind, types.as_slice()) {
                // Array, List: a count, then the elements.
                (0 | 1, [element]) => {
                    let n = r.u64()?;
                    sequence(n, ('[', ']'), r, out, |r, out| child(*element, r, out))?;
                }
                // Inline, Simd: exactly n elements.
                (2 | 3, [element]) => {
                    let n = size.ok_or("builtin size")?;
                    sequence(n, ('[', ']'), r, out, |r, out| child(*element, r, out))?;
                }
                // Packed: its variant.
                (4, [inner]) => child(*inner, r, out)?,
                // Map: count, then key and value.
                (5, [key, entry]) => {
                    let n = r.u64()?;
                    sequence(n, ('{', '}'), r, out, |r, out| {
                        child(*key, r, out)?;
                        out.push_str(": ");
                        child(*entry, r, out)
                    })?;
                }
                // Set: count, then elements.
                (6, [element]) => {
                    let n = r.u64()?;
                    sequence(n, ('{', '}'), r, out, |r, out| child(*element, r, out))?;
                }
                _ => return Err("builtin".into()),
            }
        }
    }
    Ok(())
}

fn sequence(
    n: u64,
    (open, close): (char, char),
    r: &mut Reader<'_>,
    out: &mut String,
    mut element: impl FnMut(&mut Reader<'_>, &mut String) -> Result<(), String>,
) -> Result<(), String> {
    out.push(open);
    for i in 0..n {
        if i > 0 {
            out.push_str(", ");
        }
        // Zero-size elements read nothing, so the text's length bounds the work.
        if out.len() > TEXT {
            return Err("too long to show".into());
        }
        element(r, out)?;
    }
    out.push(close);
    Ok(())
}

fn scalar(kind: u8, r: &mut Reader<'_>, out: &mut String) -> Result<(), String> {
    use std::fmt::Write;
    let int = |r: &mut Reader<'_>, n: usize| -> Result<[u8; 16], String> {
        let mut bytes = [0; 16];
        bytes[..n].copy_from_slice(r.take(n)?);
        Ok(bytes)
    };
    let signed = |bytes: [u8; 16], n: usize| {
        let shift = 128 - 8 * n as u32;
        (i128::from_le_bytes(bytes) << shift) >> shift
    };
    match kind {
        0 => out.push_str(match r.byte()? {
            0 => "false",
            1 => "true",
            _ => return Err("bool".into()),
        }),
        1..=5 => {
            let n = 1 << (kind - 1);
            let _ = write!(out, "{}", u128::from_le_bytes(int(r, n)?));
        }
        6..=10 => {
            let n = 1 << (kind - 6);
            let _ = write!(out, "{}", signed(int(r, n)?, n));
        }
        11 => {
            let _ = write!(
                out,
                "{:?}",
                f32::from_le_bytes(r.take(4)?.try_into().unwrap())
            );
        }
        12 => {
            let _ = write!(
                out,
                "{:?}",
                f64::from_le_bytes(r.take(8)?.try_into().unwrap())
            );
        }
        13 => {
            let n = r.length()?;
            let text = std::str::from_utf8(r.take(n)?).map_err(|_| "utf-8")?;
            let _ = write!(out, "{text:?}");
        }
        14 => {
            let n = r.length()?;
            out.push_str("b\"");
            for &b in r.take(n)? {
                let _ = write!(out, "{}", std::ascii::escape_default(b));
            }
            out.push('"');
        }
        15 => {
            let c = char::from_u32(r.u32()?).ok_or("char")?;
            let _ = write!(out, "{c:?}");
        }
        _ => return Err("scalar kind".into()),
    }
    Ok(())
}
