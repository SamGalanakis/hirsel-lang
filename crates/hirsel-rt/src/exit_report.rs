//! Compiler-described PROG-3 return values. These bytes are host diagnostics, never Io.
use crate::TaskCtx;
use std::fmt::Write;

/// Immutable compiler metadata. `fields` offsets are byte offsets, except in a
/// Variant node where they select ordinals. See the C header for node kinds.
#[repr(C)]
pub struct Node {
    pub kind: u64,
    pub a: u64,
    pub b: u64,
    pub c: u64,
    pub text: *const u8,
    pub text_len: u64,
    pub fields: *const Field,
    pub field_count: u64,
}
#[repr(C)]
pub struct Field {
    pub offset: u64,
    pub node: *const Node,
    pub label: *const u8,
    pub label_len: u64,
}

unsafe fn text<'a>(p: *const u8, n: u64) -> &'a str {
    if n == 0 {
        return "";
    }
    unsafe {
        std::str::from_utf8(std::slice::from_raw_parts(p, n as usize))
            .unwrap_or_else(|_| crate::fault("exit report text is not UTF-8"))
    }
}
unsafe fn word(p: *const u8, n: u64) -> u128 {
    let mut bytes = [0; 16];
    if n > 16 {
        crate::fault("exit report scalar width");
    }
    unsafe {
        bytes[..n as usize].copy_from_slice(std::slice::from_raw_parts(p, n as usize));
    }
    u128::from_le_bytes(bytes)
}

/// A borrowed returned Result, retained until the launcher's host report and cleanup.
pub(crate) struct Returned {
    pub value: *mut u8,
    pub layout: u32,
    pub payload_offset: u64,
    pub node: *const Node,
}

/// Register the returned Error for host reporting and consumption. No formatting
/// or Io happens until launch has flushed the program's buffered output.
///
/// # Safety
/// `ctx` is live; `value` remains initialized and live through launch's cleanup,
/// `layout` is its descriptor index and `node` describes its Error payload.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_exit_report(
    ctx: *mut TaskCtx,
    value: *mut u8,
    layout: u32,
    payload_offset: u64,
    node: *const Node,
) {
    unsafe {
        if (*(*(*ctx).run_ptr()).entry_result.get()).is_some() {
            crate::fault("an entry registered two returned Results");
        }
        (*(*(*ctx).run_ptr()).entry_result.get()) = Some(Returned {
            value,
            layout,
            payload_offset,
            node,
        });
    }
}

/// Format only the selected initialized payload, while the returned owner is live.
pub(crate) unsafe fn render(returned: &Returned) -> String {
    let value = unsafe { returned.value.add(returned.payload_offset as usize) };
    let node = returned.node;
    enum Work {
        Value(*const u8, *const Node, bool),
        Text(String),
    }
    let mut out = String::from("error: ");
    let mut work = vec![Work::Value(value, node, true)];
    // SAFETY: the compiler describes only initialized selected data. An explicit
    // worklist keeps host stack usage independent of source value depth.
    unsafe {
        while let Some(item) = work.pop() {
            let Work::Value(value, node, top) = item else {
                if let Work::Text(s) = item {
                    out.push_str(&s);
                }
                continue;
            };
            let node = &*node;
            let fields = if node.field_count == 0 {
                &[][..]
            } else {
                std::slice::from_raw_parts(node.fields, node.field_count as usize)
            };
            match node.kind {
                0 => out.push_str(text(node.text, node.text_len)),
                1 => out.push_str(if word(value, 1) == 0 { "false" } else { "true" }),
                2 => {
                    let _ = write!(out, "{}", word(value, node.a));
                }
                3 => {
                    let shift = 128 - node.a * 8;
                    let signed = ((word(value, node.a) << shift) as i128) >> shift;
                    let _ = write!(out, "{signed}");
                }
                4 if node.a == 4 => {
                    let _ = write!(out, "{:?}", f32::from_bits(word(value, 4) as u32));
                }
                4 => {
                    let _ = write!(out, "{:?}", f64::from_bits(word(value, 8) as u64));
                }
                5 | 6 => {
                    let owner = value.cast::<*const u8>().read_unaligned();
                    let start = word(value.add(8), 8) as usize;
                    let len = word(value.add(16), 8) as usize;
                    let bytes = std::slice::from_raw_parts(owner.add(16 + start), len);
                    if node.kind == 6 {
                        let _ = write!(out, "{bytes:?}");
                    } else {
                        let s = std::str::from_utf8(bytes)
                            .unwrap_or_else(|_| crate::fault("exit report String is not UTF-8"));
                        if top {
                            out.push_str(s);
                        } else {
                            let _ = write!(out, "{s:?}");
                        }
                    }
                }
                7 => {
                    out.push_str(text(node.text, node.text_len));
                    let close = match node.a {
                        0 => "",
                        1 if fields.len() == 1 && node.b == 1 => ",)",
                        1 => ")",
                        2 => " }",
                        3 => "]",
                        _ => crate::fault("exit report delimiter"),
                    };
                    work.push(Work::Text(close.into()));
                    for (i, f) in fields.iter().enumerate().rev() {
                        work.push(Work::Value(value.add(f.offset as usize), f.node, false));
                        if f.label_len != 0 {
                            work.push(Work::Text(format!("{}: ", text(f.label, f.label_len))));
                        }
                        if i != 0 {
                            work.push(Work::Text(", ".into()));
                        }
                    }
                }
                8 => {
                    let p = value.add(node.a as usize);
                    let ordinal = match node.b {
                        0 => word(p, 4) as u64,
                        1 if word(p, 8) == 0 => node.c & 0xffff_ffff,
                        1 => node.c >> 32,
                        2 => (word(p, 8) as u64 >> node.c) & 1,
                        _ => crate::fault("exit report tag form"),
                    };
                    let f = fields
                        .iter()
                        .find(|f| f.offset == ordinal)
                        .unwrap_or_else(|| crate::fault("exit report variant ordinal"));
                    work.push(Work::Value(value, f.node, top));
                }
                9 => {
                    out.push_str(text(node.text, node.text_len));
                    let block = value.cast::<*const u8>().read_unaligned();
                    work.push(Work::Value(
                        block.add(node.a as usize),
                        fields[0].node,
                        false,
                    ));
                }
                10 | 11 => {
                    let (base, count) = if node.kind == 10 {
                        let block = value.cast::<*const u8>().read_unaligned();
                        (
                            block.add(node.a as usize),
                            word(block.add(node.c as usize), 8) as u64,
                        )
                    } else {
                        (value, node.a)
                    };
                    out.push('[');
                    work.push(Work::Text("]".into()));
                    for i in (0..count).rev() {
                        work.push(Work::Value(
                            base.add((i * node.b) as usize),
                            fields[0].node,
                            false,
                        ));
                        if i != 0 {
                            work.push(Work::Text(", ".into()));
                        }
                    }
                }
                12 => work.push(Work::Value(value, fields[0].node, top)),
                _ => crate::fault("exit report node kind"),
            }
        }
        out.push('\n');
        out
    }
}
