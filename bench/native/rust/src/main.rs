//! Safe Box/Rc ownership and scoped bumpalo storage. Trees use two child
//! links; AST nodes use an indexed destruction worklist. Arena nodes and
//! byte buffers have no resource drops.
use bumpalo::{Bump, collections::Vec as BVec};
use std::ops::{Deref, DerefMut};

#[cfg(not(feature = "mimalloc"))]
#[global_allocator]
static ALLOC: std::alloc::System = std::alloc::System;

#[cfg(feature = "mimalloc")]
#[global_allocator]
static ALLOC: mimalloc::MiMalloc = mimalloc::MiMalloc;
#[cfg(feature = "stats")]
use std::cell::Cell;
#[cfg(feature = "stats")]
thread_local! { static COUNTS: Cell<[u64; 9]> = const { Cell::new([0; 9]) }; }
#[inline]
fn count(k: usize, n: u64) {
    #[cfg(feature = "stats")]
    COUNTS.with(|c| {
        let mut v = c.get();
        v[k] += n;
        c.set(v);
    });
    #[cfg(not(feature = "stats"))]
    let _ = (k, n);
}
fn peak(n: u64) {
    #[cfg(feature = "stats")]
    COUNTS.with(|c| {
        let mut v = c.get();
        v[8] = v[8].max(n);
        c.set(v);
    });
    #[cfg(not(feature = "stats"))]
    let _ = n;
}
fn digest(b: &[u8]) -> u64 {
    b.iter().fold(14695981039346656037, |h, x| {
        (h ^ u64::from(*x)).wrapping_mul(1099511628211)
    })
}
fn add(a: u64, b: u64) -> u64 {
    a.checked_add(b).expect("checked addition")
}
fn mul(a: u64, b: u64) -> u64 {
    a.checked_mul(b).expect("checked multiplication")
}

enum Buf<'a, T> {
    Heap(Vec<T>),
    Arena(BVec<'a, T>),
}
impl<'a, T> Buf<'a, T> {
    fn new(a: Option<&'a Bump>, cap: usize) -> Self {
        count(0, 1);
        count(1, (cap * std::mem::size_of::<T>()) as u64);
        match a {
            Some(a) => Self::Arena(BVec::with_capacity_in(cap, a)),
            None => Self::Heap(Vec::with_capacity(cap)),
        }
    }
    fn push(&mut self, v: T) {
        assert!(self.len() < self.capacity());
        match self {
            Self::Heap(b) => b.push(v),
            Self::Arena(b) => b.push(v),
        }
    }
    fn capacity(&self) -> usize {
        match self {
            Self::Heap(b) => b.capacity(),
            Self::Arena(b) => b.capacity(),
        }
    }
    fn bytes(a: Option<&'a Bump>, b: &[u8]) -> Buf<'a, u8> {
        let mut v = Buf::new(a, b.len());
        for &x in b {
            v.push(x);
        }
        count(2, b.len() as u64);
        v
    }
}
impl<T> Deref for Buf<'_, T> {
    type Target = [T];
    fn deref(&self) -> &[T] {
        match self {
            Self::Heap(b) => b,
            Self::Arena(b) => b,
        }
    }
}
impl<T> DerefMut for Buf<'_, T> {
    fn deref_mut(&mut self) -> &mut [T] {
        match self {
            Self::Heap(b) => b,
            Self::Arena(b) => b,
        }
    }
}
impl<T> Drop for Buf<'_, T> {
    fn drop(&mut self) {
        count(3, 1);
    }
}

enum Owned<'a, T> {
    Heap(Box<T>),
    Arena(&'a mut T),
}
impl<T> Deref for Owned<'_, T> {
    type Target = T;
    fn deref(&self) -> &T {
        match self {
            Self::Heap(b) => b,
            Self::Arena(b) => b,
        }
    }
}
impl<T> DerefMut for Owned<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        match self {
            Self::Heap(b) => b,
            Self::Arena(b) => b,
        }
    }
}
impl<T> Drop for Owned<'_, T> {
    fn drop(&mut self) {
        count(3, 1);
    }
}
struct Node<'a> {
    tag: u8,
    value: u64,
    a: usize,
    b: usize,
    rc: u64,
    bytes: Option<Buf<'a, u8>>,
}
struct Pool<'a> {
    arena: Option<&'a Bump>,
    nodes: Buf<'a, Option<Owned<'a, Node<'a>>>>,
}
impl<'a> Pool<'a> {
    fn new(a: Option<&'a Bump>, n: usize) -> Self {
        let mut nodes = Buf::new(a, n + 1);
        nodes.push(None);
        Self { arena: a, nodes }
    }
    fn node(&mut self, tag: u8, value: u64, a: usize, b: usize, bytes: Option<&[u8]>) -> usize {
        let bytes = bytes.map(|b| Buf::<u8>::bytes(self.arena, b));
        let v = Node {
            tag,
            value,
            a,
            b,
            rc: 1,
            bytes,
        };
        count(0, 1);
        count(1, std::mem::size_of::<Node>() as u64);
        let v = match self.arena {
            Some(a) => Owned::Arena(a.alloc(v)),
            None => Owned::Heap(Box::new(v)),
        };
        self.nodes.push(Some(v));
        self.nodes.len() - 1
    }
    fn get(&self, i: usize) -> &Node<'a> {
        self.nodes[i].as_ref().unwrap()
    }
    fn get_mut(&mut self, i: usize) -> &mut Node<'a> {
        self.nodes[i].as_mut().unwrap()
    }
    fn release(&mut self, mut i: usize) {
        while i != 0 {
            count(4, 1);
            let v = self.get_mut(i);
            v.rc -= 1;
            if v.rc != 0 {
                break;
            }
            let next = v.a;
            self.nodes[i] = None;
            i = next;
        }
    }
}
// These representations contain only the two child links. The heap form uses
// idiomatic Box ownership; the arena form uses dropless immutable references.
struct HeapTree {
    left: Option<Box<HeapTree>>,
    right: Option<Box<HeapTree>>,
}
impl Drop for HeapTree {
    fn drop(&mut self) {
        count(3, 1);
    }
}
fn heap_tree(d: usize) -> Box<HeapTree> {
    let (left, right) = if d == 0 {
        (None, None)
    } else {
        (Some(heap_tree(d - 1)), Some(heap_tree(d - 1)))
    };
    count(0, 1);
    count(1, std::mem::size_of::<HeapTree>() as u64);
    Box::new(HeapTree { left, right })
}
fn check_heap(t: &HeapTree) -> u64 {
    add(
        1,
        add(
            t.left.as_ref().map_or(0, |t| check_heap(t)),
            t.right.as_ref().map_or(0, |t| check_heap(t)),
        ),
    )
}
fn drop_tree(t: Box<HeapTree>, pending: &mut Vec<Box<HeapTree>>) {
    assert!(pending.is_empty());
    pending.push(t);
    while let Some(mut t) = pending.pop() {
        for child in [t.left.take(), t.right.take()].into_iter().flatten() {
            assert!(pending.len() < pending.capacity());
            pending.push(child);
        }
        // Both child links are now empty: Box destruction cannot recurse.
    }
}
struct ArenaTree<'a> {
    left: Option<&'a ArenaTree<'a>>,
    right: Option<&'a ArenaTree<'a>>,
}
const _: () = {
    assert!(std::mem::size_of::<HeapTree>() == 16);
    assert!(std::mem::size_of::<ArenaTree>() == 16);
};
fn arena_tree(a: &Bump, d: usize) -> &ArenaTree<'_> {
    let (left, right) = if d == 0 {
        (None, None)
    } else {
        (Some(arena_tree(a, d - 1)), Some(arena_tree(a, d - 1)))
    };
    count(0, 1);
    count(1, std::mem::size_of::<ArenaTree>() as u64);
    a.alloc(ArenaTree { left, right })
}
fn check_arena(t: &ArenaTree<'_>) -> u64 {
    add(
        1,
        add(
            t.left.map_or(0, check_arena),
            t.right.map_or(0, check_arena),
        ),
    )
}
fn binary(a: Option<&Bump>, d: usize) -> u64 {
    assert!((4..=20).contains(&d));
    if let Some(a) = a {
        let long = arena_tree(a, d);
        let mut temp = Bump::new();
        let mut total = check_arena(arena_tree(&temp, d + 1));
        peak((temp.allocated_bytes() + a.allocated_bytes()) as u64);
        count(7, temp.allocated_bytes() as u64);
        temp.reset();
        for depth in (4..=d).step_by(2) {
            for _ in 0..(1 << (d - depth)) {
                total = add(total, check_arena(arena_tree(&temp, depth)));
                peak((temp.allocated_bytes() + a.allocated_bytes()) as u64);
                count(7, temp.allocated_bytes() as u64);
                temp.reset();
            }
        }
        add(total, check_arena(long))
    } else {
        count(0, 1);
        count(1, ((d + 3) * std::mem::size_of::<Box<HeapTree>>()) as u64);
        let mut pending = Vec::with_capacity(d + 3);
        let long = heap_tree(d);
        let stretch = heap_tree(d + 1);
        let mut total = check_heap(&stretch);
        drop_tree(stretch, &mut pending);
        for depth in (4..=d).step_by(2) {
            for _ in 0..(1 << (d - depth)) {
                let t = heap_tree(depth);
                total = add(total, check_heap(&t));
                drop_tree(t, &mut pending);
            }
        }
        total = add(total, check_heap(&long));
        drop_tree(long, &mut pending);
        count(3, 1);
        total
    }
}
fn parse_expr(p: &mut Pool<'_>, b: &[u8], i: &mut usize) -> usize {
    let c = b[*i];
    *i += 1;
    if c == b'(' {
        let a = parse_expr(p, b, i);
        let op = b[*i];
        *i += 1;
        assert!(op == b'+' || op == b'*');
        let r = parse_expr(p, b, i);
        assert_eq!(b[*i], b')');
        *i += 1;
        p.node(if op == b'+' { 2 } else { 3 }, 0, a, r, None)
    } else {
        assert!(c == b'x' || c.is_ascii_digit());
        p.node(
            if c == b'x' { 1 } else { 0 },
            if c == b'x' { 3 } else { u64::from(c - b'0') },
            0,
            0,
            None,
        )
    }
}
fn eval(p: &Pool<'_>, i: usize) -> u64 {
    let v = p.get(i);
    match v.tag {
        0 | 1 => v.value,
        2 => add(eval(p, v.a), eval(p, v.b)),
        3 => mul(eval(p, v.a), eval(p, v.b)),
        _ => unreachable!(),
    }
}
fn quoted<'a>(b: &'a [u8], i: &mut usize) -> &'a [u8] {
    let start = *i;
    assert_eq!(b[*i], b'"');
    *i += 1;
    while b[*i] != b'"' {
        let c = b[*i];
        assert!(c >= 32);
        *i += 1;
        if c == b'\\' {
            let e = b[*i];
            *i += 1;
            if e == b'u' {
                for _ in 0..4 {
                    assert!(b[*i].is_ascii_hexdigit());
                    *i += 1;
                }
            } else {
                assert!(b"\"\\/bfnrt".contains(&e));
            }
        }
    }
    *i += 1;
    &b[start..*i]
}
fn parse_json(p: &mut Pool<'_>, b: &[u8], i: &mut usize) -> usize {
    match b[*i] {
        b'"' => {
            let s = quoted(b, i);
            p.node(0, 0, 0, 0, Some(s))
        }
        b'[' | b'{' => {
            let obj = b[*i] == b'{';
            *i += 1;
            let close = if obj { b'}' } else { b']' };
            let mut first = 0;
            let mut last = 0;
            while b[*i] != close {
                if first != 0 {
                    assert_eq!(b[*i], b',');
                    *i += 1;
                }
                let child = if obj {
                    let key = quoted(b, i);
                    assert_eq!(b[*i], b':');
                    *i += 1;
                    let val = parse_json(p, b, i);
                    p.node(3, 0, val, 0, Some(key))
                } else {
                    parse_json(p, b, i)
                };
                if first == 0 {
                    first = child
                } else {
                    p.get_mut(last).b = child
                }
                last = child;
            }
            *i += 1;
            p.node(if obj { 2 } else { 1 }, 0, first, 0, None)
        }
        _ => {
            let start = *i;
            while *i < b.len() && !b",]}".contains(&b[*i]) {
                *i += 1;
            }
            let s = &b[start..*i];
            assert!(!s.is_empty());
            assert!(
                s == b"true"
                    || s == b"false"
                    || s == b"null"
                    || std::str::from_utf8(s).unwrap().parse::<f64>().is_ok()
            );
            p.node(0, 0, 0, 0, Some(s))
        }
    }
}
fn append(out: &mut Buf<'_, u8>, b: &[u8]) {
    for &x in b {
        out.push(x);
    }
}
fn emit_json(p: &Pool<'_>, i: usize, out: &mut Buf<'_, u8>) {
    let v = p.get(i);
    match v.tag {
        0 => append(out, v.bytes.as_ref().unwrap()),
        1 | 2 => {
            out.push(if v.tag == 1 { b'[' } else { b'{' });
            let mut child = v.a;
            let mut first = true;
            while child != 0 {
                if !first {
                    out.push(b',')
                }
                first = false;
                emit_json(p, child, out);
                child = p.get(child).b;
            }
            out.push(if v.tag == 1 { b']' } else { b'}' });
        }
        3 => {
            append(out, v.bytes.as_ref().unwrap());
            out.push(b':');
            emit_json(p, v.a, out);
        }
        _ => unreachable!(),
    }
}
fn number(out: &mut Buf<'_, u8>, mut v: u64) {
    let mut b = [0u8; 20];
    let mut i = 20;
    loop {
        i -= 1;
        b[i] = b'0' + (v % 10) as u8;
        v /= 10;
        if v == 0 {
            break;
        }
    }
    append(out, &b[i..]);
    out.push(b'\n');
}
#[inline(never)]
fn transform(x: u64, offset: u64) -> u64 {
    add(x, offset)
}
#[inline(never)]
fn multiply(x: u64, factor: u64) -> u64 {
    mul(x, factor)
}
struct Stage {
    call: fn(u64, u64) -> u64,
    env: u64,
}
// Standard-library ownership for persistent heap lists. Tail links are taken
// before releasing each node, keeping cleanup independent of the native stack.
struct ListNode {
    value: u64,
    next: Option<std::rc::Rc<ListNode>>,
}
impl Drop for ListNode {
    fn drop(&mut self) {
        count(3, 1);
    }
}
fn list_node(value: u64, next: Option<std::rc::Rc<ListNode>>) -> std::rc::Rc<ListNode> {
    count(0, 1);
    count(
        1,
        (std::mem::size_of::<ListNode>() + 2 * std::mem::size_of::<usize>()) as u64,
    );
    std::rc::Rc::new(ListNode { value, next })
}
fn drop_std_list(mut root: Option<std::rc::Rc<ListNode>>) {
    while let Some(node) = root.take() {
        count(4, 1);
        match std::rc::Rc::try_unwrap(node) {
            Ok(mut node) => root = node.next.take(),
            Err(node) => {
                drop(node);
                break;
            }
        }
    }
}
fn std_list(n: usize, shared: bool, out: &mut Buf<'_, u8>) {
    let mut root = None;
    for i in (0..n).rev() {
        root = Some(list_node(i as u64, root));
    }
    let original = if shared {
        count(4, 1);
        root.clone()
    } else {
        None
    };
    let mut mapped = None;
    if shared {
        let mut cur = root.as_ref();
        while let Some(node) = cur {
            count(6, 1);
            mapped = Some(list_node(add(node.value, 1), mapped));
            cur = node.next.as_ref();
        }
    } else {
        while let Some(mut cur) = root.take() {
            count(5, 1);
            let node = std::rc::Rc::get_mut(&mut cur).expect("unique lineage");
            root = node.next.take();
            node.value = add(node.value, 1);
            node.next = mapped.take();
            mapped = Some(cur);
        }
    }
    let head = std::rc::Rc::get_mut(mapped.as_mut().unwrap()).unwrap();
    head.value = add(head.value, 7);
    let mut cur = mapped.as_ref();
    while let Some(node) = cur {
        number(out, node.value);
        cur = node.next.as_ref();
    }
    drop_std_list(mapped);
    if shared {
        append(out, b"---\n");
        let mut cur = root.as_ref();
        while let Some(node) = cur {
            number(out, node.value);
            cur = node.next.as_ref();
        }
        drop_std_list(root);
        drop_std_list(original);
    }
}
fn run_case<'a>(case: &str, b: &[u8], a: Option<&'a Bump>) -> Buf<'a, u8> {
    let cap = if case.starts_with("persistent-list-map/") {
        std::str::from_utf8(b)
            .unwrap()
            .trim()
            .parse::<usize>()
            .unwrap()
            * 44
            + 256
    } else {
        b.len() * 2 + 256
    };
    let mut out = Buf::new(a, cap);
    if case == "expr-compiler" {
        let tokens = Buf::<u8>::bytes(a, b);
        let mut p = Pool::new(a, b.len());
        let mut i = 0;
        let mut sum = 0;
        while i < tokens.len() {
            let root = parse_expr(&mut p, &tokens, &mut i);
            sum = add(sum, eval(&p, root));
            assert_eq!(tokens[i], b'\n');
            i += 1;
        }
        number(&mut out, sum);
        return out;
    }
    if case == "json-roundtrip" {
        let mut p = Pool::new(a, b.len());
        let mut i = 0;
        let root = parse_json(&mut p, b, &mut i);
        assert_eq!(i, b.len());
        emit_json(&p, root, &mut out);
        assert_eq!(&*out, b);
        return out;
    }
    if case == "wordfreq" {
        // Seeded FNV buckets, separate chaining, insertion-ordered node table.
        let mut p = Pool::new(a, b.len());
        let mut buckets = Buf::new(a, 256);
        for _ in 0..256 {
            buckets.push(0usize);
        }
        let text = std::str::from_utf8(b).unwrap();
        for word in text.split_ascii_whitespace() {
            let token = Buf::<u8>::bytes(a, word.as_bytes());
            let h = (digest(&token) ^ 0x4d3142) as usize % 256;
            let mut i = buckets[h];
            while i != 0 && p.get(i).bytes.as_ref().unwrap().deref() != &*token {
                i = p.get(i).a;
            }
            if i == 0 {
                let id = p.node(0, 1, buckets[h], 0, Some(&token));
                buckets[h] = id;
            } else {
                let v = p.get_mut(i);
                v.value = add(v.value, 1);
            }
        }
        for i in 1..p.nodes.len() {
            let v = p.get(i);
            append(&mut out, v.bytes.as_ref().unwrap());
            out.push(b'\t');
            number(&mut out, v.value);
        }
        return out;
    }
    let n: usize = std::str::from_utf8(b)
        .unwrap()
        .lines()
        .next()
        .unwrap()
        .parse()
        .unwrap();
    assert!(n > 0 && n <= 1000000);
    let mut sum = 0;
    match case {
        "binarytrees" => sum = binary(a, n),
        "batch-validation" => {
            let mut p = Pool::new(a, n);
            let mut head = 0;
            let mut lines = std::str::from_utf8(b).unwrap().lines().skip(1);
            for _ in 0..n {
                let (code, amount) = lines.next().unwrap().split_once(',').unwrap();
                let code = code.parse::<u8>().unwrap();
                let amount = amount.parse::<u64>().unwrap();
                head = p.node(code, amount, head, 0, None);
            }
            assert!(lines.next().is_none());
            let mut i = head;
            while i != 0 {
                let v = p.get(i);
                sum = add(
                    sum,
                    if v.tag < 25 && v.value <= 900 {
                        mul(v.value, u64::from(v.tag) + 1)
                    } else {
                        10000 + u64::from(v.tag)
                    },
                );
                i = v.a;
            }
        }
        "records-messages/scalar" => {
            let mut rows = Buf::new(a, n);
            for i in 0..n {
                rows.push([i as u64, (i % 7) as u64, (i % 13) as u64, 1]);
            }
            for _ in 0..16 {
                for r in rows.iter_mut() {
                    r[0] = add(r[0], r[3]);
                    r[1] = add(r[1], 2);
                    r[2] = add(r[2], r[0] % 11);
                }
            }
            for r in rows.iter() {
                for x in r {
                    sum = add(sum, *x);
                }
            }
        }
        "records-messages/flat" => {
            let mut flat = Buf::new(a, n * 151);
            for i in 0..n {
                let len = 24 + i % 104;
                append(&mut flat, &(i as u64).to_le_bytes());
                append(&mut flat, &((i % 7) as u64).to_le_bytes());
                append(&mut flat, &(len as u64).to_le_bytes());
                for j in 0..len {
                    flat.push((97 + (i + j * 7) % 26) as u8);
                }
            }
            let mut pos = 0;
            let mut index = 0;
            while pos < flat.len() {
                let id = u64::from_le_bytes(flat[pos..pos + 8].try_into().unwrap());
                let kind = u64::from_le_bytes(flat[pos + 8..pos + 16].try_into().unwrap());
                let len = u64::from_le_bytes(flat[pos + 16..pos + 24].try_into().unwrap()) as usize;
                let text = &flat[pos + 24..pos + 24 + len];
                sum = sum.wrapping_add(id ^ kind ^ digest(text));
                if index % 16 == 0 {
                    let mut copy = Buf::<u8>::bytes(a, &flat[pos..pos + 24 + len]);
                    let id = id ^ 0x123;
                    copy[..8].copy_from_slice(&id.to_le_bytes());
                    sum = sum.wrapping_add(id ^ kind ^ digest(&copy[24..]));
                }
                pos += 24 + len;
                index += 1;
            }
        }
        "persistent-list-map/unique" | "persistent-list-map/shared" => {
            if a.is_none() {
                std_list(n, case.ends_with("shared"), &mut out);
                return out;
            }
            assert!(
                a.is_none() || case.ends_with("unique"),
                "shared persistent lifetime has no arena variant"
            );
            let mut p = Pool::new(a, n * 2);
            let mut root = 0;
            for i in (0..n).rev() {
                root = p.node(0, i as u64, root, 0, None);
            }
            let shared = case.ends_with("shared");
            if shared {
                p.get_mut(root).rc += 1;
                count(4, 1);
            }
            let mut cur = root;
            let mut mapped = 0;
            while cur != 0 {
                let (value, next) = {
                    let v = p.get(cur);
                    (add(v.value, 1), v.a)
                };
                if shared {
                    count(6, 1);
                    mapped = p.node(0, value, mapped, 0, None);
                } else {
                    count(5, 1);
                    let v = p.get_mut(cur);
                    v.value = value;
                    v.a = mapped;
                    mapped = cur;
                }
                cur = next;
            }
            p.get_mut(mapped).value = add(p.get(mapped).value, 7);
            let mut cur = mapped;
            while cur != 0 {
                let v = p.get(cur);
                number(&mut out, v.value);
                cur = v.a;
            }
            p.release(mapped);
            if shared {
                append(&mut out, b"---\n");
                let mut cur = root;
                while cur != 0 {
                    let v = p.get(cur);
                    number(&mut out, v.value);
                    cur = v.a;
                }
                p.release(root);
                p.release(root);
            }
            return out;
        }
        "saxpy-checked/f32" => {
            let mut x = Buf::new(a, n);
            let mut y = Buf::new(a, n);
            for i in 0..n {
                x.push((i % 127) as f32 * 0.25);
                y.push((i % 31) as f32 * 0.125);
            }
            for i in 0..n {
                y[i] = 1.5 * x[i] + y[i];
            }
            for &v in y.iter() {
                sum = sum.wrapping_add(u64::from(v.to_bits()));
            }
        }
        "saxpy-checked/f64" => {
            let mut x = Buf::new(a, n);
            let mut y = Buf::new(a, n);
            for i in 0..n {
                x.push((i % 127) as f64 * 0.25);
                y.push((i % 31) as f64 * 0.125);
            }
            for i in 0..n {
                y[i] = 1.5 * x[i] + y[i];
            }
            for &v in y.iter() {
                sum = sum.wrapping_add(v.to_bits());
            }
        }
        "saxpy-checked/integer" => {
            let mut x = Buf::new(a, n);
            let mut y = Buf::new(a, n);
            for i in 0..n {
                x.push(if i == n / 2 {
                    u32::MAX
                } else {
                    (i % 1000) as u32
                });
                y.push((i % 97) as u32);
            }
            for i in 0..n {
                match x[i].checked_mul(3).and_then(|v| v.checked_add(y[i])) {
                    Some(v) => {
                        y[i] = v;
                        sum = add(sum, u64::from(v));
                    }
                    None => {
                        sum = add(sum, 1000000000 + i as u64);
                        break;
                    }
                }
            }
        }
        "slots-churn" => {
            let mut slots = Buf::new(a, n / 255 + 3);
            slots.push((1u8, Some(10u64), false));
            slots.push((1, Some(20), false));
            slots.push((1, None, false));
            let mut active = 2;
            for i in 0..n {
                let generation = slots[active].0;
                assert!(!slots[active].2);
                slots[active].1 = Some(i as u64 + 1);
                let old = (active, generation);
                let mut h = i as u64;
                for (id, s) in slots.iter().enumerate() {
                    if let Some(value) = s.1 {
                        h = h.wrapping_mul(131).wrapping_add(id as u64);
                        h = h.wrapping_mul(131).wrapping_add(u64::from(s.0));
                        h = h.wrapping_mul(131).wrapping_add(value);
                    }
                }
                sum = sum.wrapping_add(h);
                slots[active].1 = None;
                if generation == 255 {
                    slots[active].2 = true;
                    sum = sum.wrapping_add(1);
                    slots.push((1, None, false));
                    active += 1;
                } else {
                    slots[active].0 += 1;
                }
                let probe = slots
                    .get(old.0)
                    .filter(|s| s.0 == old.1 && !s.2)
                    .and_then(|s| s.1);
                assert!(probe.is_none());
                sum = sum.wrapping_add(1);
            }
        }
        "closure-iter/scoped" => {
            let f = |x| add(x, 3);
            let g = |x| mul(x, 2);
            for i in 0..n {
                let x = f(i as u64);
                if x % 2 == 0 {
                    sum = add(sum, g(x));
                }
            }
        }
        "closure-iter/erased" => {
            let mut stages = Buf::new(a, 2);
            stages.push(Stage {
                call: transform,
                env: 3,
            });
            stages.push(Stage {
                call: multiply,
                env: 2,
            });
            for i in 0..n {
                let x = (std::hint::black_box(stages[0].call))(i as u64, stages[0].env);
                if x % 2 == 0 {
                    sum = add(
                        sum,
                        (std::hint::black_box(stages[1].call))(x, stages[1].env),
                    );
                }
            }
        }
        "resource-failure/throughput" | "resource-failure/heavy" => {
            let modulus = if case.ends_with("heavy") { 2 } else { 17 };
            for i in 0..n {
                let mut resources = Buf::new(a, 2);
                let source = std::array::from_fn::<_, 16, _>(|j| (i % 127 + j) as u8);
                let mut transaction = source;
                count(2, 16);
                for v in &mut transaction {
                    *v = v.checked_add(1).unwrap();
                }
                let mut destination = [0u8; 16];
                if i % modulus != 0 {
                    destination = transaction;
                    count(2, 16);
                }
                resources.push((1u8, false));
                resources.push((2u8, false));
                let mut h = i as u64;
                for e in [1, 2, 3, 4, if i % modulus == 0 { 6 } else { 5 }] {
                    h = h.wrapping_mul(131).wrapping_add(e);
                }
                for r in resources.iter_mut().rev() {
                    assert!(!r.1);
                    r.1 = true;
                    h = h
                        .wrapping_mul(131)
                        .wrapping_add(if r.0 == 2 { 7 } else { 8 });
                }
                assert!(resources.iter().all(|r| r.1));
                sum = sum.wrapping_add(h ^ digest(&destination));
            }
        }
        _ => panic!("unknown case"),
    }
    number(&mut out, sum);
    out
}
fn main() {
    let args: Vec<_> = std::env::args().collect();
    assert!(
        args.len() == 5 || args.len() == 6,
        "CASE INPUT ROUNDS VARIANT [--emit]"
    );
    let b = std::fs::read(&args[2]).unwrap();
    let rounds: usize = args[3].parse().unwrap();
    assert!(rounds > 0);
    assert!(matches!(args[4].as_str(), "system" | "mimalloc" | "arena"));
    let use_arena = args[4] == "arena";
    assert!(
        !use_arena
            || matches!(
                args[1].as_str(),
                "binarytrees"
                    | "expr-compiler"
                    | "json-roundtrip"
                    | "wordfreq"
                    | "records-messages/flat"
                    | "batch-validation"
                    | "persistent-list-map/unique"
            )
    );
    let emit = args.get(5).is_some_and(|s| s == "--emit");
    if emit {
        assert_eq!(rounds, 1);
    }
    let mut bump = Bump::new();
    let mut checksum = 0u64;
    for _ in 0..rounds {
        {
            let out = run_case(&args[1], &b, use_arena.then_some(&bump));
            checksum = checksum.wrapping_add(digest(&out));
            if emit {
                use std::io::Write;
                std::io::stdout().write_all(&out).unwrap();
            }
        }
        peak(bump.allocated_bytes() as u64);
        count(7, bump.allocated_bytes() as u64);
        bump.reset();
    }
    if !emit {
        println!("{checksum:016x}");
    }
    #[cfg(feature="stats")] COUNTS.with(|c|{let v=c.get();eprintln!("{{\"allocation_requests\":{},\"requested_bytes\":{},\"copied_bytes\":{},\"release_requests\":{},\"count_operations\":{},\"reuse_hits\":{},\"reuse_misses\":{},\"arena_reserved_bytes_sum\":{},\"arena_peak_reserved_bytes\":{}}}",v[0],v[1],v[2],v[3],v[4],v[5],v[6],v[7],v[8]);});
}
