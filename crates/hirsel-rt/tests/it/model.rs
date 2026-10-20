//! Runtime correspondence with the Lean RC model (`proofs/rc`): every
//! `rc-trace v1` vector in `native-tests/model/` runs on the runtime through its C entries, and
//! the runtime's final state, printed in the model's exact form, must equal the vector's checked
//! `.expected` output. Task H's executor is held to the same files, so J and H agree through D.
//!
//! The adapter keeps only what the model names and the runtime does not store: model addresses
//! (the model's rule, lowest free address first), generations, initialized-field masks, roots,
//! borrows, locals and the forget ledger. Counts, liveness, raw tokens, field contents, region
//! storage and finalizer order are read back from the runtime. `finalize` and `drain` steps are
//! no-ops: the runtime drains inside the release that kills a block, and its finalizer takes the
//! vector's `finalize` outcomes in order, so a different order shows up as different events.
//!
//! Vectors whose expected state is mid-drain are outside the runtime's observable states (its
//! release is atomic). An illegal operation is either rejected by the runtime in diagnostic
//! mode (checked in a child process) or excluded before the runtime, as each case states.

use crate::support;

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::ptr;

use hirsel_rt::alloc::{hirsel_rt_v2_alloc, hirsel_rt_v2_dispose, hirsel_rt_v2_reuse};
use hirsel_rt::count::hirsel_rt_v2_dup;
use hirsel_rt::failure::{hirsel_rt_v2_failure_begin, hirsel_rt_v2_failure_clear};
use hirsel_rt::region::{
    hirsel_rt_v2_region_enter, hirsel_rt_v2_region_exit, hirsel_rt_v2_region_hold,
    hirsel_rt_v2_region_reserve,
};
use hirsel_rt::release::{hirsel_rt_v2_release, hirsel_rt_v2_reset};
use hirsel_rt::{
    COUNT_IMMORTAL, Config, DOMAIN_HEAP, Header, Layout, Region, STATE_FREE, STATE_LIVE, STATE_RAW,
    TaskCtx,
};
use support::*;

// ----- vectors ------------------------------------------------------------------------------

#[derive(Clone, Copy, Debug)]
enum Op {
    Alloc {
        r: u32,
        arity: u64,
        fin: bool,
    },
    Static {
        r: u32,
    },
    Dup {
        src: u32,
        dst: u32,
    },
    Drop {
        r: u32,
    },
    Write {
        b: u32,
        k: u64,
        r: u32,
    },
    Take {
        b: u32,
        k: u64,
        r: u32,
    },
    Load {
        p: u32,
        k: u64,
        r: u32,
    },
    Borrow {
        id: u32,
        r: u32,
    },
    BorrowField {
        id: u32,
        p: u32,
        k: u64,
    },
    BorrowLocal {
        id: u32,
        l: u32,
        k: u64,
    },
    EndBorrow {
        id: u32,
    },
    Reset {
        b: u32,
        t: u32,
    },
    Reuse {
        t: u32,
        r: u32,
        arity: u64,
        fin: bool,
    },
    Dispose {
        t: u32,
    },
    Finalize(bool),
    Drain,
    Forget {
        r: u32,
    },
    RegionOpen {
        g: u32,
    },
    RegionAlloc {
        g: u32,
        l: u32,
        arity: u64,
    },
    RegionStore {
        l: u32,
        k: u64,
        r: u32,
    },
    LocalCopy {
        l: u32,
        copy: u32,
    },
    LocalDrop {
        l: u32,
    },
    RegionClose {
        g: u32,
    },
}

struct Vector {
    max: u64,
    ops: Vec<Op>,
}

fn ident(word: &str, sort: char) -> u32 {
    word.strip_prefix(sort)
        .and_then(|n| n.parse().ok())
        .unwrap_or_else(|| panic!("expected {sort}<number>, got {word}"))
}

fn num(word: &str) -> u64 {
    word.parse()
        .unwrap_or_else(|_| panic!("expected a number, got {word}"))
}

fn parse(text: &str) -> Vector {
    let mut lines = text
        .lines()
        .map(|line| line.split('#').next().unwrap())
        .map(|line| line.split_whitespace().collect::<Vec<_>>())
        .filter(|words| !words.is_empty());
    assert_eq!(
        lines.next().unwrap(),
        ["rc-trace", "v1"],
        "a vector this runtime knows"
    );
    let max = match lines.next().unwrap().as_slice() {
        ["max", m] => num(m),
        other => panic!("expected max, got {other:?}"),
    };
    let ops = lines
        .map(|w| {
            let fin = |rest: &[&str]| match rest {
                [] => false,
                ["fin"] => true,
                other => panic!("unexpected {other:?}"),
            };
            match w.as_slice() {
                ["alloc", r, n, rest @ ..] => Op::Alloc {
                    r: ident(r, 'r'),
                    arity: num(n),
                    fin: fin(rest),
                },
                ["static", r] => Op::Static { r: ident(r, 'r') },
                ["dup", a, b] => Op::Dup {
                    src: ident(a, 'r'),
                    dst: ident(b, 'r'),
                },
                ["drop", r] => Op::Drop { r: ident(r, 'r') },
                ["write", b, k, r] => Op::Write {
                    b: ident(b, 'r'),
                    k: num(k),
                    r: ident(r, 'r'),
                },
                ["take", b, k, r] => Op::Take {
                    b: ident(b, 'r'),
                    k: num(k),
                    r: ident(r, 'r'),
                },
                ["load", p, k, r] => Op::Load {
                    p: ident(p, 'b'),
                    k: num(k),
                    r: ident(r, 'r'),
                },
                ["borrow", b, r] => Op::Borrow {
                    id: ident(b, 'b'),
                    r: ident(r, 'r'),
                },
                ["borrow-field", b, p, k] => Op::BorrowField {
                    id: ident(b, 'b'),
                    p: ident(p, 'b'),
                    k: num(k),
                },
                ["borrow-local", b, l, k] => Op::BorrowLocal {
                    id: ident(b, 'b'),
                    l: ident(l, 'l'),
                    k: num(k),
                },
                ["end-borrow", b] => Op::EndBorrow { id: ident(b, 'b') },
                ["reset", b, t] => Op::Reset {
                    b: ident(b, 'r'),
                    t: ident(t, 't'),
                },
                ["reuse", t, r, n, rest @ ..] => Op::Reuse {
                    t: ident(t, 't'),
                    r: ident(r, 'r'),
                    arity: num(n),
                    fin: fin(rest),
                },
                ["dispose", t] => Op::Dispose { t: ident(t, 't') },
                ["finalize", "ok"] => Op::Finalize(true),
                ["finalize", "fail"] => Op::Finalize(false),
                ["drain"] => Op::Drain,
                ["forget", r] => Op::Forget { r: ident(r, 'r') },
                ["region-open", g] => Op::RegionOpen { g: ident(g, 'g') },
                ["region-alloc", g, l, n] => Op::RegionAlloc {
                    g: ident(g, 'g'),
                    l: ident(l, 'l'),
                    arity: num(n),
                },
                ["region-store", l, k, r] => Op::RegionStore {
                    l: ident(l, 'l'),
                    k: num(k),
                    r: ident(r, 'r'),
                },
                ["local-copy", a, b] => Op::LocalCopy {
                    l: ident(a, 'l'),
                    copy: ident(b, 'l'),
                },
                ["local-drop", l] => Op::LocalDrop { l: ident(l, 'l') },
                ["region-close", g] => Op::RegionClose { g: ident(g, 'g') },
                other => panic!("unknown operation {other:?}"),
            }
        })
        .collect();
    Vector { max, ops }
}

// ----- finalizer outcomes -------------------------------------------------------------------

thread_local! {
    /// The vector's `finalize` outcomes, consumed in the order the runtime calls finalizers.
    static OUTCOMES: RefCell<VecDeque<bool>> = const { RefCell::new(VecDeque::new()) };
    /// `(value address, outcome)` of each finalizer call.
    static CALLS: RefCell<Vec<(u64, bool)>> = const { RefCell::new(Vec::new()) };
}

unsafe extern "C" fn model_finalizer(
    ctx: *mut TaskCtx,
    value: *mut u8,
    anchor: u64,
    _operation_site: u64,
) -> u8 {
    let ok = OUTCOMES
        .with(|o| o.borrow_mut().pop_front())
        .expect("the runtime ran more finalizers than the vector has finalize steps");
    CALLS.with(|c| c.borrow_mut().push((value as u64, ok)));
    if ok {
        0
    } else {
        unsafe { hirsel_rt_v2_failure_begin(ctx, 1, 15, anchor, _operation_site) }
    }
}

/// The descriptor of a model block: `arity` pointer fields, released highest first.
fn descriptor(arity: u64, fin: bool) -> &'static Layout {
    thread_local! {
        static CACHE: RefCell<HashMap<(u64, bool), &'static Layout>> = RefCell::new(HashMap::new());
    }
    CACHE.with(|cache| {
        let descriptor = *cache.borrow_mut().entry((arity, fin)).or_insert_with(|| {
            let steps = (0..arity)
                .rev()
                .map(|k| release_step(k as u32, 8 * k))
                .collect();
            layout(
                8 * arity,
                arity as u32,
                fin.then_some(model_finalizer as _),
                steps,
            )
        });
        register_fixture(descriptor);
        descriptor
    })
}

// ----- the adapter --------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq)]
enum St {
    Free,
    Live,
    Raw,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Kind {
    Heap,
    Static,
    Region(u32),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Holder {
    Root(u32),
    Field(usize, u64),
    Forgot(u32),
}

impl Holder {
    fn show(self) -> String {
        match self {
            Holder::Root(r) => format!("r{r}"),
            Holder::Field(a, k) => format!("@{a}.{k}"),
            Holder::Forgot(n) => format!("forget {n}"),
        }
    }
}

#[derive(Clone, Copy, Debug)]
enum Parent {
    None,
    Borrow(u32),
    Local(u32),
}

#[derive(Clone, Copy, Debug)]
struct Borrow {
    guard: Holder,
    parent: Parent,
    target: usize,
    generation: u64,
}

struct Slot {
    generation: u64,
    st: St,
    kind: Kind,
    arity: u64,
    fin: bool,
    published: bool,
    /// The block (heap, static and raw slots) or region storage holding the fields.
    block: *mut Header,
    fields: *mut u8,
    init: Vec<bool>,
}

struct Adapter {
    ctx: Ctx,
    slots: Vec<Slot>,
    /// Model address of each block reference and region storage address.
    address: HashMap<u64, usize>,
    roots: BTreeMap<u32, usize>,
    forgot: BTreeMap<u32, usize>,
    borrows: BTreeMap<u32, Borrow>,
    locals: BTreeMap<u32, usize>,
    tokens: BTreeMap<u32, Option<usize>>,
    /// The storage each consumed token named, so an illegal second use reaches the runtime.
    spent: BTreeMap<u32, *mut Header>,
    /// Open regions, innermost first.
    regions: Vec<(u32, Box<Region>, Vec<usize>)>,
    events: Vec<String>,
}

impl Adapter {
    fn new(max: u64) -> Adapter {
        Adapter {
            ctx: Ctx::with(Config {
                flags: hirsel_rt::context::CONFIG_DIAGNOSTIC,
                count_max: max,
                ..Config::new()
            }),
            slots: Vec::new(),
            address: HashMap::new(),
            roots: BTreeMap::new(),
            forgot: BTreeMap::new(),
            borrows: BTreeMap::new(),
            locals: BTreeMap::new(),
            tokens: BTreeMap::new(),
            spent: BTreeMap::new(),
            regions: Vec::new(),
            events: Vec::new(),
        }
    }

    /// The model's rule: the first free address below the frontier, else the frontier.
    fn free_address(&self) -> usize {
        self.slots
            .iter()
            .position(|s| s.st == St::Free)
            .unwrap_or(self.slots.len())
    }

    fn install(
        &mut self,
        a: usize,
        kind: Kind,
        arity: u64,
        fin: bool,
        block: *mut Header,
        fields: *mut u8,
    ) {
        let generation = self.slots.get(a).map_or(0, |s| s.generation) + 1;
        let slot = Slot {
            generation,
            st: St::Live,
            kind,
            arity,
            fin,
            published: false,
            block,
            fields,
            init: vec![false; arity as usize],
        };
        if a == self.slots.len() {
            self.slots.push(slot);
        } else {
            self.slots[a] = slot;
        }
        self.address.insert(fields as u64, a);
    }

    fn root(&self, r: u32) -> usize {
        self.roots[&r]
    }

    fn block(&self, a: usize) -> *mut Header {
        self.slots[a].block
    }

    /// The model address a pointer field of `a` holds.
    fn field(&self, a: usize, k: u64) -> usize {
        let child = unsafe {
            self.slots[a]
                .fields
                .add(8 * k as usize)
                .cast::<*mut Header>()
                .read()
        };
        self.address[&(support::payload(child) as u64)]
    }

    fn set_field(&mut self, a: usize, k: u64, child: usize) {
        let block = self.block(child);
        unsafe {
            self.slots[a]
                .fields
                .add(8 * k as usize)
                .cast::<*mut Header>()
                .write(block)
        };
        self.slots[a].init[k as usize] = true;
    }

    /// The live mask of `a` when a field is uninitialized and `a` is about to die.
    fn mask(&self, a: usize) -> Option<Vec<u64>> {
        let slot = &self.slots[a];
        if slot.init.iter().all(|&i| i) || unsafe { (*slot.block).count } != 1 {
            return None;
        }
        let mut words = vec![0u64; slot.init.len().div_ceil(64)];
        for (k, _) in slot.init.iter().enumerate().filter(|(_, i)| **i) {
            words[k / 64] |= 1 << (k % 64);
        }
        Some(words)
    }

    fn release(&mut self, a: usize) {
        let mask = self.mask(a);
        let block = self.block(a);
        let status = unsafe {
            hirsel_rt_v2_release(
                self.ctx.0,
                block,
                (*block).index(),
                mask.as_ref().map_or(ptr::null(), |m| m.as_ptr()),
                0,
                0,
            )
        };
        self.settle(status);
    }

    fn settle(&mut self, status: u8) {
        if status != 0 {
            unsafe { hirsel_rt_v2_failure_clear(self.ctx.0) };
        }
    }

    /// Read back what the operation did: freed and raw blocks, and finalizer events.
    fn sync(&mut self) {
        for slot in &mut self.slots {
            if slot.st != St::Free && slot.kind == Kind::Heap {
                // The block's header stays in the runtime's slab until the next allocation, and
                // every operation that frees allocates nothing afterwards.
                slot.st = match state(self.ctx.0, slot.block) {
                    STATE_FREE => St::Free,
                    STATE_RAW => St::Raw,
                    STATE_LIVE => St::Live,
                    other => panic!("a block left in state {other} after an operation"),
                };
            }
        }
        for (value, ok) in CALLS.with(|c| std::mem::take(&mut *c.borrow_mut())) {
            let a = self.address[&value];
            let outcome = if ok { "ok" } else { "fail" };
            self.events.push(format!(
                "finalize @{a}#{} {outcome}",
                self.slots[a].generation
            ));
        }
        // A dead field credit leaves with its holder.
        for slot in &mut self.slots {
            if slot.st != St::Live {
                slot.init.iter_mut().for_each(|i| *i = false);
            }
        }
    }

    fn region(&mut self, g: u32) -> *mut Region {
        let (_, region, _) = self.regions.iter_mut().find(|(id, _, _)| *id == g).unwrap();
        &mut **region
    }

    fn step(&mut self, op: Op) {
        let ctx = self.ctx.0;
        match op {
            Op::Alloc { r, arity, fin } => {
                let layout = descriptor(arity, fin);
                let block = unsafe {
                    hirsel_rt_v2_alloc(ctx, index(ctx, layout), 8 * arity, 8, DOMAIN_HEAP, 0)
                };
                let a = self.free_address();
                self.install(a, Kind::Heap, arity, fin, block, support::payload(block));
                self.roots.insert(r, a);
            }
            Op::Static { r } => {
                let block: &'static mut Header = Box::leak(Box::new(Header {
                    count: COUNT_IMMORTAL,
                    descriptor: index(ctx, descriptor(0, false)) | (2 << 29),
                }));
                let block: *mut Header = block;
                let a = self.free_address();
                self.install(a, Kind::Static, 0, false, block, support::payload(block));
                self.roots.insert(r, a);
            }
            Op::Dup { src, dst } => {
                let a = self.root(src);
                unsafe { hirsel_rt_v2_dup(ctx, self.block(a)) };
                self.roots.insert(dst, a);
            }
            Op::Drop { r } => {
                let a = self.roots.remove(&r).unwrap();
                self.release(a);
            }
            Op::Write { b, k, r } => {
                let a = self.root(b);
                assert_eq!(unsafe { (*self.block(a)).count }, 1);
                if self.slots[a].init[k as usize] {
                    let displaced = self.field(a, k);
                    self.slots[a].init[k as usize] = false;
                    self.release(displaced);
                }
                let child = self.roots.remove(&r).unwrap();
                self.set_field(a, k, child);
            }
            Op::Take { b, k, r } => {
                let a = self.root(b);
                let child = self.field(a, k);
                self.slots[a].init[k as usize] = false;
                self.roots.insert(r, child);
            }
            Op::Load { p, k, r } => {
                let parent = self.borrows[&p].target;
                let child = self.field(parent, k);
                unsafe { hirsel_rt_v2_dup(ctx, self.block(child)) };
                self.roots.insert(r, child);
            }
            Op::Borrow { id, r } => {
                let target = self.root(r);
                let generation = self.slots[target].generation;
                self.borrows.insert(
                    id,
                    Borrow {
                        guard: Holder::Root(r),
                        parent: Parent::None,
                        target,
                        generation,
                    },
                );
            }
            Op::BorrowField { id, p, k } => {
                let parent = self.borrows[&p].target;
                let target = self.field(parent, k);
                let generation = self.slots[target].generation;
                self.borrows.insert(
                    id,
                    Borrow {
                        guard: Holder::Field(parent, k),
                        parent: Parent::Borrow(p),
                        target,
                        generation,
                    },
                );
            }
            Op::BorrowLocal { id, l, k } => {
                let local = self.locals[&l];
                self.slots[local].published = true;
                let target = self.field(local, k);
                let generation = self.slots[target].generation;
                self.borrows.insert(
                    id,
                    Borrow {
                        guard: Holder::Field(local, k),
                        parent: Parent::Local(l),
                        target,
                        generation,
                    },
                );
            }
            Op::EndBorrow { id } => {
                self.borrows.remove(&id).unwrap();
            }
            Op::Reset { b, t } => {
                let a = self.roots.remove(&b).unwrap();
                let mask = self.mask(a);
                let block = self.block(a);
                let mut token = ptr::null_mut();
                let status = unsafe {
                    hirsel_rt_v2_reset(
                        ctx,
                        block,
                        (*block).index(),
                        mask.as_ref().map_or(ptr::null(), |m| m.as_ptr()),
                        0,
                        0,
                        &mut token,
                    )
                };
                self.settle(status);
                self.tokens.insert(
                    t,
                    (!token.is_null()).then(|| self.address[&(support::payload(token) as u64)]),
                );
            }
            Op::Reuse { t, r, arity, fin } => {
                let token = self.tokens.remove(&t).unwrap();
                let raw = token.map_or(ptr::null_mut(), |a| self.block(a));
                let mut block = ptr::null_mut();
                let layout = descriptor(arity, fin);
                assert_eq!(
                    unsafe {
                        hirsel_rt_v2_reuse(ctx, raw, index(ctx, layout), 8 * arity, 8, &mut block)
                    },
                    0
                );
                let a = token.unwrap_or_else(|| self.free_address());
                self.install(a, Kind::Heap, arity, fin, block, support::payload(block));
                self.roots.insert(r, a);
            }
            Op::Dispose { t } => {
                let raw = match self.tokens.remove(&t) {
                    Some(token) => token.map_or(ptr::null_mut(), |a| self.block(a)),
                    None => self.spent[&t],
                };
                self.spent.insert(t, raw);
                unsafe { hirsel_rt_v2_dispose(ctx, raw) };
            }
            Op::Finalize(_) | Op::Drain => {}
            Op::Forget { r } => {
                let a = self.roots.remove(&r).unwrap();
                self.forgot.insert(r, a);
            }
            Op::RegionOpen { g } => {
                let mut region = Box::new(Region {
                    segment: ptr::null_mut(),
                    cursor: ptr::null_mut(),
                    end: ptr::null_mut(),
                    cleanup: ptr::null_mut(),
                    parent: ptr::null_mut(),
                    state: 0,
                    remaining: u64::MAX,
                    bound: 0,
                    exhausted: 0,
                });
                unsafe { hirsel_rt_v2_region_enter(ctx, &mut *region) };
                self.regions.insert(0, (g, region, Vec::new()));
            }
            Op::RegionAlloc { g, l, arity } => {
                let region = self.region(g);
                let mut storage = ptr::null_mut();
                assert_eq!(
                    unsafe {
                        hirsel_rt_v2_region_reserve(
                            ctx,
                            region,
                            (8 * arity).max(8),
                            8,
                            &mut storage,
                        )
                    },
                    0
                );
                let a = self.free_address();
                self.install(
                    a,
                    Kind::Region(g),
                    arity,
                    false,
                    ptr::null_mut(),
                    storage.cast(),
                );
                self.regions
                    .iter_mut()
                    .find(|(id, _, _)| *id == g)
                    .unwrap()
                    .2
                    .push(a);
                self.locals.insert(l, a);
            }
            Op::RegionStore { l, k, r } => {
                let local = self.locals[&l];
                let Kind::Region(g) = self.slots[local].kind else {
                    unreachable!()
                };
                let child = self.roots.remove(&r).unwrap();
                self.set_field(local, k, child);
                let region = self.region(g);
                assert_eq!(
                    unsafe { hirsel_rt_v2_region_hold(ctx, region, self.block(child)) },
                    0
                );
            }
            Op::LocalCopy { l, copy } => {
                let a = self.locals[&l];
                self.slots[a].published = true;
                self.locals.insert(copy, a);
            }
            Op::LocalDrop { l } => {
                self.locals.remove(&l).unwrap();
            }
            Op::RegionClose { g } => {
                let region = self.region(g);
                let status = unsafe { hirsel_rt_v2_region_exit(ctx, region, 0, 0) };
                self.settle(status);
                let (_, _, blocks) = self.regions.remove(0);
                for a in blocks {
                    self.slots[a].st = St::Free;
                    self.slots[a].init.iter_mut().for_each(|i| *i = false);
                }
            }
        }
        self.sync();
    }

    /// The state in `rc-trace v1`'s `render` form.
    fn render(&self) -> String {
        let mut out = String::new();
        let mut section = |title: &str, rows: Vec<String>| {
            out.push_str(title);
            out.push('\n');
            for row in rows {
                out.push_str("  ");
                out.push_str(&row);
                out.push('\n');
            }
        };
        let heap = self
            .slots
            .iter()
            .enumerate()
            .map(|(a, s)| match s.st {
                St::Free => format!("@{a} free gen {}", s.generation),
                St::Raw => format!("@{a} raw gen {} capacity {}", s.generation, s.arity),
                St::Live => {
                    let count = match s.kind {
                        Kind::Region(_) => "uncounted".to_string(),
                        _ => match unsafe { (*s.block).count } {
                            COUNT_IMMORTAL => "count immortal".to_string(),
                            n => format!("count fin {n}"),
                        },
                    };
                    let kind = match s.kind {
                        Kind::Heap => "heap".to_string(),
                        Kind::Static => "static".to_string(),
                        Kind::Region(g) => format!("region g{g}"),
                    };
                    let fin = if s.fin { " fin" } else { "" };
                    let published = if s.published { " published" } else { "" };
                    format!(
                        "@{a} live gen {} {count} {kind} arity {}{fin}{published}",
                        s.generation, s.arity
                    )
                }
            })
            .collect();
        section("heap", heap);
        let mut credits: Vec<(Holder, usize)> = Vec::new();
        credits.extend(self.roots.iter().map(|(&r, &a)| (Holder::Root(r), a)));
        for (a, slot) in self.slots.iter().enumerate() {
            for k in (0..slot.arity).filter(|&k| slot.st == St::Live && slot.init[k as usize]) {
                credits.push((Holder::Field(a, k), self.field(a, k)));
            }
        }
        credits.extend(self.forgot.iter().map(|(&n, &a)| (Holder::Forgot(n), a)));
        credits.sort();
        section(
            "credits",
            credits
                .iter()
                .map(|(h, a)| format!("{} -> @{a}#{}", h.show(), self.slots[*a].generation))
                .collect(),
        );
        section(
            "borrows",
            self.borrows
                .iter()
                .map(|(id, b)| {
                    let parent = match b.parent {
                        Parent::None => "-".to_string(),
                        Parent::Borrow(p) => format!("b{p}"),
                        Parent::Local(l) => format!("l{l}"),
                    };
                    format!(
                        "b{id} guard {} parent {parent} -> @{}#{}",
                        b.guard.show(),
                        b.target,
                        b.generation
                    )
                })
                .collect(),
        );
        section(
            "locals",
            self.locals
                .iter()
                .map(|(l, &a)| format!("l{l} -> @{a}#{}", self.slots[a].generation))
                .collect(),
        );
        section(
            "tokens",
            self.tokens
                .iter()
                .map(|(t, token)| match token {
                    None => format!("t{t} none"),
                    Some(a) => {
                        let s = &self.slots[*a];
                        format!("t{t} raw @{a}#{} capacity {}", s.generation, s.arity)
                    }
                })
                .collect(),
        );
        section("work", Vec::new());
        section(
            "regions",
            self.regions
                .iter()
                .map(|(g, _, blocks)| {
                    let mut blocks = blocks.clone();
                    blocks.sort();
                    let blocks: Vec<_> = blocks.iter().map(|a| format!("@{a}")).collect();
                    format!("g{g} blocks [{}]", blocks.join(", "))
                })
                .collect(),
        );
        section("events", self.events.clone());
        out
    }
}

// ----- the vectors --------------------------------------------------------------------------

fn vectors_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../native-tests/model")
}

fn load(name: &str) -> (Vector, String) {
    let dir = vectors_dir();
    let trace = std::fs::read_to_string(dir.join(format!("{name}.trace"))).unwrap();
    let expected = std::fs::read_to_string(dir.join(format!("{name}.expected"))).unwrap();
    (parse(&trace), expected)
}

/// Run the first `steps` operations of a vector on the runtime.
fn run(vector: &Vector, steps: usize) -> Adapter {
    let outcomes = vector.ops[..steps]
        .iter()
        .filter_map(|op| match op {
            Op::Finalize(ok) => Some(*ok),
            _ => None,
        })
        .collect();
    OUTCOMES.with(|o| *o.borrow_mut() = outcomes);
    CALLS.with(|c| c.borrow_mut().clear());
    for op in &vector.ops {
        match *op {
            Op::Alloc { arity, fin, .. } | Op::Reuse { arity, fin, .. } => {
                descriptor(arity, fin);
            }
            Op::Static { .. } => {
                descriptor(0, false);
            }
            _ => {}
        }
    }
    let mut adapter = Adapter::new(vector.max);
    for &op in &vector.ops[..steps] {
        adapter.step(op);
    }
    assert!(
        OUTCOMES.with(|o| o.borrow().is_empty()),
        "the runtime ran fewer finalizers than the vector has finalize steps"
    );
    adapter
}

/// How an illegal operation is kept from the runtime, or rejected by it.
enum Illegal {
    /// The runtime aborts in diagnostic mode with this message.
    Rejected(&'static str),
    /// The emitted count comparison answers false, so generated code takes the copy path instead.
    NotUnique,
    /// Excluded before the runtime by the named static check.
    Excluded(&'static str),
}

/// What happens to each vector: `None` when its final state is comparable.
fn disposition(name: &str) -> Option<Result<Illegal, &'static str>> {
    use Illegal::*;
    Some(Ok(match name {
        "borrow-guard-error" | "end-borrow-child-error" | "region-borrow-local" => Excluded(
            "loans are static: TIR's EndLoan placement and the C validator's borrow replay",
        ),
        "region-escape-error" => {
            Excluded("a local into a region cannot outlive it (MEM-10, PROV-4)")
        }
        "region-published-error" => {
            Excluded("published region blocks are immutable in LIR (P3, C validator)")
        }
        "stale-handle-error" => Excluded(
            "use after release is OWN-2; the runtime sees only a reused address of the same class",
        ),
        "reuse-capacity-error" => Excluded(
            "P5's exact reuse key; the runtime keys on the class capacity, which arity 1 and 2 share",
        ),
        "write-shared-error" | "saturation-no-unique" => NotUnique,
        "dup-resource-error" => Rejected("a dup of a block whose layout has a finalizer"),
        "token-double-use-error" => Rejected("dispose of storage that is not a raw reuse token"),
        "region-order-error" => Rejected("a region exits before a region entered inside it"),
        "drain-finalizer-due-error" | "reuse-pending-error" | "tree-drain-midway" => {
            return Some(Err(
                "a mid-drain state: the runtime drains atomically inside the release that kills a block",
            ));
        }
        _ => return None,
    }))
}

fn all_vectors() -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(vectors_dir())
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "trace"))
        .map(|p| p.file_stem().unwrap().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

/// The `.expected` text after its two header lines.
fn expected_state(expected: &str) -> &str {
    let mut rest = expected;
    for _ in 0..2 {
        rest = &rest[rest.find('\n').unwrap() + 1..];
    }
    rest
}

/// The step (1-based) an error vector stops at.
fn error_step(expected: &str) -> Option<usize> {
    let line = expected.lines().nth(1)?;
    let rest = line.strip_prefix("error at step ")?;
    rest.split_whitespace().next()?.parse().ok()
}

#[test]
fn every_model_vector_agrees_with_the_runtime_or_is_accounted_for() {
    let names = all_vectors();
    assert!(names.len() >= 32, "D publishes the vectors");
    let mut compared = 0;
    for name in &names {
        let (vector, expected) = load(name);
        match disposition(name) {
            None => {
                assert!(
                    expected.lines().nth(1).unwrap().starts_with("ok after"),
                    "{name}"
                );
                let adapter = run(&vector, vector.ops.len());
                assert_eq!(adapter.render(), expected_state(&expected), "{name}");
                assert_eq!(
                    adapter.ctx.stats().live_blocks as usize,
                    live_heap(&adapter),
                    "{name}"
                );
                compared += 1;
            }
            Some(Err(reason)) => println!("{name}: not comparable, {reason}"),
            Some(Ok(illegal)) => {
                match &illegal {
                    Illegal::Rejected(message) => {
                        println!("{name}: the runtime rejects it ({message})")
                    }
                    Illegal::NotUnique => println!("{name}: the count comparison answers false"),
                    Illegal::Excluded(reason) => println!("{name}: excluded, {reason}"),
                }
                let step =
                    error_step(&expected).unwrap_or_else(|| panic!("{name} is an error vector"));
                // The state before the illegal operation agrees.
                let mut adapter = run(&vector, step - 1);
                assert_eq!(adapter.render(), expected_state(&expected), "{name}");
                if let Illegal::NotUnique = illegal {
                    let Op::Write { b, .. } = vector.ops[step - 1] else {
                        panic!("{name}: a not-unique case is a write")
                    };
                    let block = adapter.block(adapter.root(b));
                    assert_eq!(unsafe { u8::from((*block).count == 1) }, 0, "{name}");
                }
                adapter.roots.clear();
            }
        }
    }
    assert!(compared >= 15, "compared {compared} vectors");
}

/// Heap and raw blocks the adapter believes are allocated.
fn live_heap(adapter: &Adapter) -> usize {
    adapter
        .slots
        .iter()
        .filter(|s| s.kind == Kind::Heap && s.st != St::Free)
        .count()
}

/// Run an error vector through its illegal operation in a child: the runtime must abort.
fn rejected(test: &str, name: &str) {
    let Some(Ok(Illegal::Rejected(needle))) = disposition(name) else {
        panic!("{name} is not a runtime-rejected vector")
    };
    if in_child(test) {
        let (vector, expected) = load(name);
        run(&vector, error_step(&expected).unwrap());
        return;
    }
    assert_child_aborts(test, needle);
}

#[test]
fn the_runtime_rejects_a_dup_of_a_resource() {
    rejected(
        "the_runtime_rejects_a_dup_of_a_resource",
        "dup-resource-error",
    );
}

#[test]
fn the_runtime_rejects_a_token_used_twice() {
    rejected(
        "the_runtime_rejects_a_token_used_twice",
        "token-double-use-error",
    );
}

#[test]
fn the_runtime_rejects_a_region_closed_out_of_order() {
    rejected(
        "the_runtime_rejects_a_region_closed_out_of_order",
        "region-order-error",
    );
}
