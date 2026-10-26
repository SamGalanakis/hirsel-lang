//! Borrowed structural value operations. The emitted program is never parsed
//! into owned metadata, and traversal scratch is bounded by admitted depth.
use super::{
    borrowed::{self as b, Children, Location, Program, Projection},
    *,
};
use crate::kernels::siphash::SipHasher13;
use std::{cmp::Ordering, mem::MaybeUninit};
const CAPACITY: usize = hirsel_core_contract::depth::STRUCTURAL_LIMIT as usize + 1;

enum View<'a> {
    Scalar { code: u64, at: *const u8 },
    Bytes(&'a [u8]),
    Path(u64),
    Children(Children<'a>),
}
unsafe fn view<'a>(p: Program<'a>, mut projection: Projection) -> View<'a> {
    unsafe {
        loop {
            let node = p.node(projection.node);
            if node[0] == NODE_CHECKED {
                projection.node = node[2] as usize;
                continue;
            }
            if let Location::Address(at) = projection.location {
                match node[0] {
                    NODE_BOX => {
                        projection = Projection::at(node[4] as usize, payload(pointer(at)));
                        continue;
                    }
                    NODE_FRAMED => {
                        projection = Projection::at(node[1] as usize, at.add(8));
                        continue;
                    }
                    NODE_SCALAR => return View::Scalar { code: node[1], at },
                    NODE_PATH => return View::Path(word(at)),
                    NODE_TEXT => {
                        return View::Bytes(text_bytes(
                            &Text {
                                utf8: node[1] != 0,
                                owner: node[2],
                                start: node[3],
                                length: node[4],
                                index: node[5] as u32,
                            },
                            at,
                        ));
                    }
                    NODE_FLAT_TEXT => {
                        return View::Bytes(std::slice::from_raw_parts(
                            at.add(8),
                            word(at) as usize,
                        ));
                    }
                    _ => (),
                }
            }
            return View::Children(b::children(p, projection));
        }
    }
}
fn scalar(code: u64) -> Scalar {
    match code {
        0 => Scalar::Bool,
        1 | 6 => Scalar::Integer(1),
        2 | 7 => Scalar::Integer(2),
        3 | 8 => Scalar::Integer(4),
        4 | 9 => Scalar::Integer(8),
        5 | 10 => Scalar::Integer(16),
        11 => Scalar::F32,
        12 => Scalar::F64,
        13 => Scalar::Char,
        _ => fault("borrowed access scalar"),
    }
}

#[derive(Clone, Copy)]
struct Ordered<'a> {
    cursor: Children<'a>,
    previous: Option<Projection>,
}
impl<'a> Ordered<'a> {
    fn new(cursor: Children<'a>) -> Self {
        Self {
            cursor,
            previous: None,
        }
    }
    unsafe fn next(
        &mut self,
        p: Program<'a>,
        ctx: *mut TaskCtx,
        work: &mut [MaybeUninit<Pair<'a>>],
    ) -> Option<Projection> {
        unsafe {
            if !self.cursor.set {
                return self.cursor.next(p);
            }
            // Borrowed Set storage cannot be sorted in place. Select successive
            // keys in canonical order without width-sized temporary storage.
            // Nested key comparisons share the same bounded scratch suffix.
            let mut scan = self.cursor;
            let mut best = None;
            while let Some(key) = scan.next(p) {
                let total = |a, b, work: &mut [MaybeUninit<Pair<'a>>]| {
                    order(p, ctx, a, b, work)
                        .unwrap_or_else(|| fault("unordered structural Set key"))
                };
                if self
                    .previous
                    .is_some_and(|previous| total(key, previous, work) != Ordering::Greater)
                {
                    continue;
                }
                if best.is_none_or(|best| total(key, best, work) == Ordering::Less) {
                    best = Some(key);
                }
            }
            if let Some(best) = best {
                self.previous = Some(best);
            }
            best
        }
    }
}
#[derive(Clone, Copy)]
struct Pair<'a> {
    left: Ordered<'a>,
    right: Ordered<'a>,
}

/// Every recursive entry is only a Set's key comparison and uses its caller's
/// unused scratch suffix. Ordinary recursive values use the iterative cursor.
#[inline(never)]
unsafe fn order<'a>(
    p: Program<'a>,
    ctx: *mut TaskCtx,
    mut a: Projection,
    mut c: Projection,
    work: &mut [MaybeUninit<Pair<'a>>],
) -> Option<Ordering> {
    let mut used = 0;
    unsafe {
        loop {
            let result = match (view(p, a), view(p, c)) {
                (View::Scalar { code, at: a }, View::Scalar { at: c, .. }) => {
                    scalar_order(scalar(code), code, a, c)
                }
                (View::Bytes(a), View::Bytes(c)) => Some(a.cmp(c)),
                (View::Path(a), View::Path(c)) => {
                    Some(crate::io::hirsel_rt_v2_path_compare(ctx, a, c).cmp(&0))
                }
                (View::Children(ac), View::Children(cc)) => {
                    if ac.ordinal != cc.ordinal {
                        return Some(ac.ordinal.cmp(&cc.ordinal));
                    }
                    let mut pair = Pair {
                        left: Ordered::new(ac),
                        right: Ordered::new(cc),
                    };
                    let (left, right) = {
                        let scratch = work
                            .get_mut(used + 1..)
                            .unwrap_or_else(|| fault("structural order depth invariant"));
                        (
                            pair.left.next(p, ctx, scratch),
                            pair.right.next(p, ctx, scratch),
                        )
                    };
                    if let (Some(left), Some(right)) = (left, right) {
                        // A unary physical wrapper adds no continuation. Set
                        // cursors rescan, so retain them until they report end.
                        if pair.left.cursor.has_more()
                            || pair.right.cursor.has_more()
                            || ac.count != cc.count
                        {
                            work[used].write(pair);
                            used += 1;
                        }
                        a = left;
                        c = right;
                        continue;
                    }
                    Some(ac.count.cmp(&cc.count))
                }
                _ => fault("structural value view mismatch"),
            };
            if result != Some(Ordering::Equal) {
                return result;
            }
            loop {
                if used == 0 {
                    return Some(Ordering::Equal);
                }
                let mut pair = work[used - 1].assume_init_read();
                let (left, right) = {
                    let scratch = &mut work[used..];
                    (
                        pair.left.next(p, ctx, scratch),
                        pair.right.next(p, ctx, scratch),
                    )
                };
                if let (Some(left), Some(right)) = (left, right) {
                    work[used - 1].write(pair);
                    a = left;
                    c = right;
                    break;
                }
                let result = pair.left.cursor.count.cmp(&pair.right.cursor.count);
                if result != Ordering::Equal {
                    return Some(result);
                }
                used -= 1;
            }
        }
    }
}

pub(crate) unsafe fn compare(words: &[u64], ctx: *mut TaskCtx, a: *const u8, c: *const u8) -> u8 {
    let p = Program::new(words);
    let mut work = [const { MaybeUninit::uninit() }; CAPACITY];
    match unsafe {
        order(
            p,
            ctx,
            Projection::at(p.root(), a),
            Projection::at(p.root(), c),
            &mut work,
        )
    } {
        Some(Ordering::Equal) => 0,
        Some(Ordering::Less) => 1,
        Some(Ordering::Greater) => 2,
        None => 3,
    }
}

pub(crate) unsafe fn hash(words: &[u64], ctx: *mut TaskCtx, at: *const u8, seed: u64) -> u64 {
    let p = Program::new(words);
    let mut hash = SipHasher13::new(seed, crate::HASH_KEY_1);
    let mut work = [const { MaybeUninit::<Ordered<'_>>::uninit() }; CAPACITY];
    let mut comparisons = [const { MaybeUninit::uninit() }; CAPACITY];
    let mut used = 0;
    let mut current = Projection::at(p.root(), at);
    unsafe {
        loop {
            match view(p, current) {
                View::Scalar { code, at } => match scalar(code) {
                    Scalar::Bool => hash.write(&[u8::from(at.read() != 0)]),
                    Scalar::Integer(width) => hash.write(std::slice::from_raw_parts(at, width)),
                    Scalar::Char => hash.write(std::slice::from_raw_parts(at, 4)),
                    Scalar::F32 => {
                        let n = at.cast::<f32>().read_unaligned();
                        hash.write(&if n == 0.0 { 0 } else { n.to_bits() }.to_le_bytes());
                    }
                    Scalar::F64 => {
                        let n = at.cast::<f64>().read_unaligned();
                        hash.write(&if n == 0.0 { 0 } else { n.to_bits() }.to_le_bytes());
                    }
                },
                View::Bytes(bytes) => {
                    hash.write(&(bytes.len() as u64).to_le_bytes());
                    hash.write(bytes);
                }
                View::Path(id) => {
                    let names = crate::paths::names(ctx);
                    let text = names.path_text(id);
                    hash.write(&(text.len() as u64).to_le_bytes());
                    hash.write(text.as_bytes());
                }
                View::Children(cursor) => {
                    if let Some(tag) = cursor.ordinal {
                        hash.write(&tag.to_le_bytes());
                    }
                    if let Some(count) = cursor.count {
                        hash.write(&count.to_le_bytes());
                    }
                    let mut cursor = Ordered::new(cursor);
                    if let Some(child) = cursor.next(p, ctx, &mut comparisons) {
                        if cursor.cursor.has_more() {
                            if used == work.len() {
                                fault("structural hash depth invariant");
                            }
                            work[used].write(cursor);
                            used += 1;
                        }
                        current = child;
                        continue;
                    }
                }
            }
            loop {
                if used == 0 {
                    return hash.finish();
                }
                let mut cursor = work[used - 1].assume_init_read();
                if let Some(child) = cursor.next(p, ctx, &mut comparisons) {
                    work[used - 1].write(cursor);
                    current = child;
                    break;
                }
                used -= 1;
            }
        }
    }
}

unsafe fn bits(at: *const u8, width: usize) -> u128 {
    let mut bytes = [0u8; 16];
    unsafe {
        std::ptr::copy_nonoverlapping(at, bytes.as_mut_ptr(), width);
    }
    u128::from_le_bytes(bytes)
}
pub(super) unsafe fn scalar_order(
    kind: Scalar,
    code: u64,
    a: *const u8,
    b: *const u8,
) -> Option<Ordering> {
    unsafe {
        Some(match kind {
            Scalar::Bool => (a.read() != 0).cmp(&(b.read() != 0)),
            Scalar::Char => bits(a, 4).cmp(&bits(b, 4)),
            Scalar::Integer(width) => {
                let (a, b) = (bits(a, width), bits(b, width));
                if (6..=10).contains(&code) {
                    let shift = 128 - width * 8;
                    (((a << shift) as i128) >> shift).cmp(&(((b << shift) as i128) >> shift))
                } else {
                    a.cmp(&b)
                }
            }
            Scalar::F32 => {
                return f32::from_bits(bits(a, 4) as u32)
                    .partial_cmp(&f32::from_bits(bits(b, 4) as u32));
            }
            Scalar::F64 => {
                return f64::from_bits(bits(a, 8) as u64)
                    .partial_cmp(&f64::from_bits(bits(b, 8) as u64));
            }
        })
    }
}
