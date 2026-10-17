//! `rc-trace v1` protocol vectors (proofs/rc/README.md) executed on this
//! crate's ledger. The Lean model's `lake exe rc-trace` is one implementation of
//! the protocol; this is an independent second one: counts, saturation, holder
//! cells, dying blocks, raw tokens, region storage and forget records are the
//! heap executor's own, and after every operation its ideal-holder recount must
//! agree with every block's count. The printed state follows the vector format,
//! so a vector's output is compared byte for byte with its checked `.expected`.
//!
//! A model address is a storage slot the executor's allocations occupy in turn:
//! allocation takes the lowest free slot, and a birth at a slot bumps the slot's
//! generation. A model field `k` is the pointer cell at offset `16k` of a block
//! whose payload is `16 (arity + 1)` bytes, so distinct arities fall in distinct
//! size classes and the executor's exact capacity rule is the model's.
use super::*;
use crate::build::{Layouts, path};
use crate::ledger::{AllocId, Region as LedgerRegion};
use crate::program::{Config, NoOperations, Program};
use hirsel_lir::layout_types::{FieldLayout, FinalizerRef, LayoutTable, ScanStep, TargetSpec};
use hirsel_lir::stage::Ready;

const STRIDE: u64 = 16;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Op {
    Alloc(u64, u64, bool),
    Static(u64),
    Dup(u64, u64),
    Drop(u64),
    Write(u64, u64, u64),
    Take(u64, u64, u64),
    Load(u64, u64, u64),
    Borrow(u64, u64),
    BorrowField(u64, u64, u64),
    BorrowLocal(u64, u64, u64),
    EndBorrow(u64),
    Reset(u64, u64),
    Reuse(u64, u64, u64, bool),
    Dispose(u64),
    Finalize(bool),
    Drain,
    Forget(u64),
    RegionOpen(u64),
    RegionAlloc(u64, u64, u64),
    RegionStore(u64, u64, u64),
    LocalCopy(u64, u64),
    LocalDrop(u64),
    RegionClose(u64),
}

fn ident(prefix: char, word: &str) -> Result<u64, String> {
    word.strip_prefix(prefix)
        .and_then(|n| n.parse().ok())
        .ok_or_else(|| format!("expected {prefix}<number>, got '{word}'"))
}

fn num(word: &str) -> Result<u64, String> {
    word.parse()
        .map_err(|_| format!("expected a number, got '{word}'"))
}

fn fin(rest: &[&str]) -> Result<bool, String> {
    match rest {
        [] => Ok(false),
        ["fin"] => Ok(true),
        other => Err(format!("unexpected '{}'", other.join(" "))),
    }
}

fn parse_op(words: &[&str]) -> Result<Op, String> {
    Ok(match words {
        ["alloc", r, n, rest @ ..] => Op::Alloc(ident('r', r)?, num(n)?, fin(rest)?),
        ["static", r] => Op::Static(ident('r', r)?),
        ["dup", a, b] => Op::Dup(ident('r', a)?, ident('r', b)?),
        ["drop", r] => Op::Drop(ident('r', r)?),
        ["write", b, k, r] => Op::Write(ident('r', b)?, num(k)?, ident('r', r)?),
        ["take", b, k, r] => Op::Take(ident('r', b)?, num(k)?, ident('r', r)?),
        ["load", p, k, r] => Op::Load(ident('b', p)?, num(k)?, ident('r', r)?),
        ["borrow", b, r] => Op::Borrow(ident('b', b)?, ident('r', r)?),
        ["borrow-field", b, p, k] => Op::BorrowField(ident('b', b)?, ident('b', p)?, num(k)?),
        ["borrow-local", b, l, k] => Op::BorrowLocal(ident('b', b)?, ident('l', l)?, num(k)?),
        ["end-borrow", b] => Op::EndBorrow(ident('b', b)?),
        ["reset", b, t] => Op::Reset(ident('r', b)?, ident('t', t)?),
        ["reuse", t, r, n, rest @ ..] => {
            Op::Reuse(ident('t', t)?, ident('r', r)?, num(n)?, fin(rest)?)
        }
        ["dispose", t] => Op::Dispose(ident('t', t)?),
        ["finalize", "ok"] => Op::Finalize(true),
        ["finalize", "fail"] => Op::Finalize(false),
        ["drain"] => Op::Drain,
        ["forget", r] => Op::Forget(ident('r', r)?),
        ["region-open", g] => Op::RegionOpen(ident('g', g)?),
        ["region-alloc", g, l, n] => Op::RegionAlloc(ident('g', g)?, ident('l', l)?, num(n)?),
        ["region-store", l, k, r] => Op::RegionStore(ident('l', l)?, num(k)?, ident('r', r)?),
        ["local-copy", a, b] => Op::LocalCopy(ident('l', a)?, ident('l', b)?),
        ["local-drop", l] => Op::LocalDrop(ident('l', l)?),
        ["region-close", g] => Op::RegionClose(ident('g', g)?),
        other => return Err(format!("unknown operation '{}'", other.join(" "))),
    })
}

/// The bound and the operations with their 1-based source lines.
fn parse(text: &str) -> Result<(u64, Vec<(usize, Op)>), String> {
    let mut lines = text.lines().enumerate().filter_map(|(i, line)| {
        let code = line.split('#').next().unwrap_or("");
        let words: Vec<&str> = code.split_whitespace().collect();
        (!words.is_empty()).then_some((i + 1, words))
    });
    let max = match (lines.next(), lines.next()) {
        (Some((_, v)), Some((_, m)))
            if v == ["rc-trace", "v1"] && m.len() == 2 && m[0] == "max" =>
        {
            num(m[1])?
        }
        _ => return Err("a vector starts with 'rc-trace v1' and 'max N'".into()),
    };
    if max < 2 {
        return Err("max must be at least 2".into());
    }
    let ops = lines
        .map(|(i, words)| {
            parse_op(&words)
                .map(|op| (i, op))
                .map_err(|e| format!("line {i}: {e}"))
        })
        .collect::<Result<_, _>>()?;
    Ok((max, ops))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
enum Holder {
    Root(u64),
    Field(AllocId, u64),
    Forgot(u64),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Parent {
    None,
    Borrow(u64),
    Local(u64),
}

#[derive(Clone, Copy, Debug)]
struct View {
    guard: Holder,
    parent: Parent,
    target: Ptr,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Kind {
    Heap,
    Static,
    Region(u64),
}

struct Runner<'p> {
    m: Machine<'p, Ready>,
    objects: &'p BTreeMap<(u64, bool), LayoutId>,
    /// Model address -> the allocation occupying it.
    slots: Vec<AllocId>,
    /// Births per address: the model generation.
    births: Vec<u64>,
    address: BTreeMap<AllocId, usize>,
    /// (allocation, executor generation) -> model generation of that birth.
    generation: BTreeMap<(AllocId, u32), u64>,
    shape: BTreeMap<AllocId, (Kind, u64, bool)>,
    roots: BTreeMap<u64, Ptr>,
    forgot: BTreeMap<u64, Ptr>,
    views: BTreeMap<u64, View>,
    locals: BTreeMap<u64, Ptr>,
    tokens: BTreeMap<u64, Option<TokenId>>,
    resets: BTreeMap<AllocId, u64>,
    /// Dying blocks, head first.
    work: Vec<AllocId>,
    /// Open regions, innermost first: (model id, ledger region).
    regions: Vec<(u64, usize)>,
    events: Vec<(AllocId, u64, bool)>,
    /// When each region field credit was stored: the model releases them most recent first.
    stored: BTreeMap<(AllocId, u64), u64>,
    clock: u64,
}

type Step = Result<(), String>;

impl<'p> Runner<'p> {
    fn object(&self, arity: u64, finalizer: bool) -> LayoutId {
        self.objects[&(arity, finalizer)]
    }

    fn free_address(&self) -> usize {
        self.slots
            .iter()
            .position(|a| self.m.ledger.get(*a).state == AllocState::Dead)
            .unwrap_or(self.slots.len())
    }

    /// Record a birth of `ptr` at `address`.
    fn born(&mut self, address: usize, ptr: Ptr, shape: (Kind, u64, bool)) {
        if address == self.slots.len() {
            self.slots.push(ptr.alloc);
            self.births.push(0);
        }
        self.slots[address] = ptr.alloc;
        self.births[address] += 1;
        self.address.insert(ptr.alloc, address);
        self.generation
            .insert((ptr.alloc, ptr.generation), self.births[address]);
        self.shape.insert(ptr.alloc, shape);
        if shape.2 {
            let rid = ResourceId(self.m.ledger.resources.len() as u32);
            let layout = self.object(shape.1, true);
            self.m.ledger.resources.push(Resource {
                layout,
                finalizer: true,
                state: ResourceState::Live,
                forgotten: false,
            });
            self.m
                .ledger
                .get_mut(ptr.alloc)
                .extent
                .resources
                .insert(Vec::new(), rid);
        }
    }

    fn at(&self, a: AllocId) -> usize {
        self.address[&a]
    }

    fn name(&self, p: Ptr) -> String {
        format!(
            "@{}#{}",
            self.at(p.alloc),
            self.generation[&(p.alloc, p.generation)]
        )
    }

    fn guarded(&self, h: Holder) -> bool {
        self.views.values().any(|v| v.guard == h)
    }

    fn field(&self, a: AllocId, k: u64) -> Option<Ptr> {
        match self.m.ledger.get(a).extent.cells.get(&(k * STRIDE)) {
            Some(Cell::Data {
                ptr, counted: true, ..
            }) => Some(*ptr),
            _ => None,
        }
    }

    fn resource(&self, a: AllocId) -> Option<&Resource> {
        let r = self.m.ledger.get(a).extent.resources.get(&Vec::new())?;
        Some(&self.m.ledger.resources[r.0 as usize])
    }

    /// The model's `fin && !finDone`: a declared finalizer has not run.
    fn finalizer_due(&self, a: AllocId) -> bool {
        self.resource(a)
            .is_some_and(|r| r.state == ResourceState::Live)
    }

    fn checked<T>(&self, r: Checked<T>) -> Result<T, String> {
        r.map_err(|v| format!("heap executor violation: {v}"))
    }

    /// Release one credit: the block dies onto the head of the worklist.
    fn release(&mut self, p: Ptr) -> Step {
        let died = self.m.count_down(p);
        if self.checked(died)? {
            self.work.insert(0, p.alloc);
        }
        Ok(())
    }

    fn take_cell(&mut self, a: AllocId, k: u64) -> Ptr {
        let word = self.m.word;
        let e =
            self.m
                .ledger
                .get_mut(a)
                .extent
                .take(k * STRIDE, word, UNINIT_POISON, Some((&[], &[])));
        match e.cells.get(&0) {
            Some(Cell::Data { ptr, .. }) => *ptr,
            _ => unreachable!("only initialized fields are taken"),
        }
    }

    /// Move a credit into field `k` of `a`: the cell becomes counted.
    fn put_cell(&mut self, a: AllocId, k: u64, child: Ptr) -> Step {
        let at = Ptr {
            alloc: a,
            generation: self.m.ledger.get(a).generation,
            offset: 0,
            path: 0,
        };
        let at = self
            .m
            .ledger
            .project(at, k * STRIDE, ProjectionStep::Field(k as u32));
        let reference = self.objects[&(u64::MAX, false)];
        let written = self.m.write_value(
            at,
            reference,
            Moved {
                val: Val::Ptr(child),
                credited: true,
                borrow: None,
            },
        );
        self.checked(written)
    }

    fn alloc(&mut self, r: u64, arity: u64, finalizer: bool) -> Step {
        let layout = self.object(arity, finalizer);
        let size = (arity + 1) * STRIDE;
        let address = self.free_address();
        let p = self.m.heap_alloc(layout, size, 8);
        let p = self.checked(p)?;
        self.born(address, p, (Kind::Heap, arity, finalizer));
        self.put_root(r, p);
        Ok(())
    }

    fn step(&mut self, op: Op) -> Result<Step, String> {
        macro_rules! refuse { ($($t:tt)*) => { return Ok(Err(format!($($t)*))) }; }
        match op {
            Op::Alloc(r, n, f) => {
                if self.roots.contains_key(&r) {
                    refuse!("T01 alloc: root r{r} already holds a value");
                }
                self.alloc(r, n, f)?;
            }
            Op::Static(r) => {
                if self.roots.contains_key(&r) {
                    refuse!("T02 static: root r{r} already holds a value");
                }
                let address = self.free_address();
                let layout = self.object(0, false);
                let p = self.m.static_alloc(layout, &[], format!("static r{r}"));
                let p = self.checked(p)?;
                self.born(address, p, (Kind::Static, 0, false));
                self.put_root(r, p);
            }
            Op::Dup(src, dst) => {
                let Some(&p) = self.roots.get(&src) else {
                    refuse!("T03 dup: root r{src} holds no value");
                };
                if self.roots.contains_key(&dst) {
                    refuse!("T03 dup: root r{dst} already holds a value");
                }
                if self.shape[&p.alloc].2 {
                    refuse!("T03 dup: @{} is a resource (noncopyable)", self.at(p.alloc));
                }
                let dup = self.m.dup(p);
                self.checked(dup)?;
                self.put_root(dst, p);
            }
            Op::Drop(r) => {
                let Some(&p) = self.roots.get(&r) else {
                    refuse!("T04 release: root r{r} holds no value");
                };
                if self.guarded(Holder::Root(r)) {
                    refuse!("T04 release: root r{r} still guards a live borrow");
                }
                self.take_root(r);
                self.release(p)?;
            }
            Op::Write(b, k, r) => {
                let Some(&pb) = self.roots.get(&b) else {
                    refuse!("T05 write: root r{b} holds no value");
                };
                let a = pb.alloc;
                if self.m.ledger.get(a).count != Some(Count::Finite(1)) {
                    refuse!("T05 write: @{} is not unique", self.at(a));
                }
                if self.shape[&a].1 <= k {
                    refuse!("T05 write: @{} has no field {k}", self.at(a));
                }
                if self.guarded(Holder::Root(b)) {
                    refuse!("T05 write: root r{b} is borrowed");
                }
                if r == b {
                    refuse!("T05 write: a block cannot hold itself");
                }
                let Some(&pr) = self.roots.get(&r) else {
                    refuse!("T05 write: root r{r} holds no value");
                };
                if self.guarded(Holder::Root(r)) {
                    refuse!("T05 write: root r{r} is borrowed");
                }
                if self.field(a, k).is_some() {
                    let displaced = self.take_cell(a, k);
                    self.release(displaced)?;
                }
                self.take_root(r);
                self.put_cell(a, k, pr)?;
            }
            Op::Take(b, k, r) => {
                let Some(&pb) = self.roots.get(&b) else {
                    refuse!("T06 take: root r{b} holds no value");
                };
                let a = pb.alloc;
                if self.m.ledger.get(a).count != Some(Count::Finite(1)) {
                    refuse!("T06 take: @{} is not unique", self.at(a));
                }
                if self.guarded(Holder::Root(b)) {
                    refuse!("T06 take: root r{b} is borrowed");
                }
                if self.roots.contains_key(&r) {
                    refuse!("T06 take: root r{r} already holds a value");
                }
                if self.field(a, k).is_none() {
                    refuse!("T06 take: field {k} of @{} is not initialized", self.at(a));
                }
                let child = self.take_cell(a, k);
                self.put_root(r, child);
            }
            Op::Load(p, k, r) => {
                let Some(view) = self.views.get(&p).copied() else {
                    refuse!("T07 load: borrow b{p} is not live");
                };
                let a = view.target.alloc;
                let Some(child) = self.field(a, k) else {
                    refuse!("T07 load: field {k} of @{} is not initialized", self.at(a));
                };
                if self.roots.contains_key(&r) {
                    refuse!("T07 load: root r{r} already holds a value");
                }
                if self.shape[&child.alloc].2 {
                    refuse!(
                        "T07 load: @{} is a resource (noncopyable)",
                        self.at(child.alloc)
                    );
                }
                let dup = self.m.dup(child);
                self.checked(dup)?;
                self.put_root(r, child);
            }
            Op::Borrow(id, r) => {
                if self.views.contains_key(&id) {
                    refuse!("T08 borrow: borrow b{id} is already live");
                }
                let Some(&p) = self.roots.get(&r) else {
                    refuse!("T08 borrow: root r{r} holds no value");
                };
                self.views.insert(
                    id,
                    View {
                        guard: Holder::Root(r),
                        parent: Parent::None,
                        target: p,
                    },
                );
            }
            Op::BorrowField(id, p, k) => {
                if self.views.contains_key(&id) {
                    refuse!("T09 borrow-field: borrow b{id} is already live");
                }
                let Some(view) = self.views.get(&p).copied() else {
                    refuse!("T09 borrow-field: borrow b{p} is not live");
                };
                let a = view.target.alloc;
                let Some(child) = self.field(a, k) else {
                    refuse!(
                        "T09 borrow-field: field {k} of @{} is not initialized",
                        self.at(a)
                    );
                };
                self.views.insert(
                    id,
                    View {
                        guard: Holder::Field(a, k),
                        parent: Parent::Borrow(p),
                        target: child,
                    },
                );
            }
            Op::BorrowLocal(id, l, k) => {
                if self.views.contains_key(&id) {
                    refuse!("T10 borrow-local: borrow b{id} is already live");
                }
                let Some(&local) = self.locals.get(&l) else {
                    refuse!("T10 borrow-local: local l{l} is not live");
                };
                let a = local.alloc;
                let Some(child) = self.field(a, k) else {
                    refuse!(
                        "T10 borrow-local: field {k} of @{} is not initialized",
                        self.at(a)
                    );
                };
                self.m.ledger.get_mut(a).published = true;
                self.views.insert(
                    id,
                    View {
                        guard: Holder::Field(a, k),
                        parent: Parent::Local(l),
                        target: child,
                    },
                );
            }
            Op::EndBorrow(id) => {
                if !self.views.contains_key(&id) {
                    refuse!("T11 end-borrow: borrow b{id} is not live");
                }
                if self.views.values().any(|v| v.parent == Parent::Borrow(id)) {
                    refuse!("T11 end-borrow: borrow b{id} still has a live sub-borrow");
                }
                self.views.remove(&id);
            }
            Op::Reset(b, t) => {
                let Some(&p) = self.roots.get(&b) else {
                    refuse!("T12 reset: root r{b} holds no value");
                };
                if self.guarded(Holder::Root(b)) {
                    refuse!("T12 reset: root r{b} still guards a live borrow");
                }
                if self.tokens.contains_key(&t) {
                    refuse!("T12 reset: token t{t} already exists");
                }
                let unique = self.m.ledger.get(p.alloc).count == Some(Count::Finite(1));
                self.take_root(b);
                self.m.ledger.counters.resets += 1;
                self.release(p)?;
                if unique {
                    let token = TokenId(self.m.ledger.tokens.len() as u32);
                    self.m.ledger.tokens.push(Token {
                        alloc: p.alloc,
                        generation: p.generation,
                        consumed: false,
                        native: Default::default(),
                    });
                    self.tokens.insert(t, Some(token));
                    self.resets.insert(p.alloc, t);
                } else {
                    self.tokens.insert(t, None);
                }
            }
            Op::Reuse(t, r, n, f) => {
                let Some(&token) = self.tokens.get(&t) else {
                    refuse!("T13 reuse: token t{t} does not exist");
                };
                if self.roots.contains_key(&r) {
                    refuse!("T13 reuse: root r{r} already holds a value");
                }
                match token {
                    None => {
                        self.tokens.remove(&t);
                        self.alloc(r, n, f)?;
                    }
                    Some(token) => {
                        let donor = self.m.ledger.tokens[token.0 as usize].alloc;
                        if self.m.ledger.get(donor).state != AllocState::Raw(token) {
                            refuse!(
                                "T13 reuse: donor @{} of token t{t} has not finished draining",
                                self.at(donor)
                            );
                        }
                        let layout = self.object(n, f);
                        match self.m.install_raw(token, layout, true) {
                            Ok(p) => {
                                self.tokens.remove(&t);
                                let address = self.at(donor);
                                self.born(address, p, (Kind::Heap, n, f));
                                self.put_root(r, p);
                            }
                            Err(v) if v.kind == V::IncompatibleReuse => refuse!(
                                "T13 reuse: token t{t} has capacity {}, not {n}",
                                self.shape[&donor].1
                            ),
                            Err(v) => return Err(format!("heap executor violation: {v}")),
                        }
                    }
                }
            }
            Op::Dispose(t) => {
                let Some(&token) = self.tokens.get(&t) else {
                    refuse!("T14 dispose: token t{t} does not exist");
                };
                if let Some(token) = token {
                    let donor = self.m.ledger.tokens[token.0 as usize].alloc;
                    if self.m.ledger.get(donor).state != AllocState::Raw(token) {
                        refuse!(
                            "T14 dispose: donor @{} of token t{t} has not finished draining",
                            self.at(donor)
                        );
                    }
                    let disposed = self.m.dispose(Some(token));
                    self.checked(disposed)?;
                }
                self.tokens.remove(&t);
            }
            Op::Finalize(ok) => {
                let Some(&a) = self.work.first() else {
                    refuse!("T15 finalize: the worklist is empty");
                };
                if !self.finalizer_due(a) {
                    refuse!("T15 finalize: @{} has no finalizer due", self.at(a));
                }
                let rid = self.m.ledger.get(a).extent.resources[&Vec::new()];
                self.m.ledger.resources[rid.0 as usize].state = ResourceState::FieldsPending;
                self.m.ledger.counters.finalizers += 1;
                self.m.ledger.trace.push(Event::Finalize { resource: rid });
                let generation = self.m.ledger.get(a).generation;
                self.events.push((a, self.generation[&(a, generation)], ok));
                if !ok && let Some(t) = self.resets.remove(&a) {
                    // A failing finalizer disposes the donor: no token escapes.
                    if let Some(Some(token)) = self.tokens.insert(t, None) {
                        self.m.ledger.tokens[token.0 as usize].consumed = true;
                    }
                }
            }
            Op::Drain => {
                let Some(&a) = self.work.first() else {
                    refuse!("T16 drain: the worklist is empty");
                };
                if self.finalizer_due(a) {
                    refuse!("T16 drain: @{} has a finalizer due", self.at(a));
                }
                let last = self
                    .m
                    .ledger
                    .get(a)
                    .extent
                    .cells
                    .iter()
                    .rev()
                    .find_map(|(o, c)| {
                        matches!(c, Cell::Data { counted: true, .. }).then_some(*o / STRIDE)
                    });
                match last {
                    Some(k) => {
                        let child = self.take_cell(a, k);
                        self.release(child)?;
                    }
                    None => {
                        self.work.remove(0);
                        match self.resets.remove(&a) {
                            Some(t) => {
                                let token = self.tokens[&t].expect("a pending token");
                                let size = self.m.ledger.get(a).extent.len();
                                let rest = self.m.ledger.get_mut(a).extent.take(
                                    0,
                                    size,
                                    FREE_POISON,
                                    None,
                                );
                                if !rest.cells.is_empty() {
                                    return Err(format!(
                                        "heap executor violation: reset of {a:?} left a holder"
                                    ));
                                }
                                let ended = self.m.ledger.end_resources(&rest);
                                self.checked(ended)?;
                                let block = self.m.ledger.get_mut(a);
                                block.state = AllocState::Raw(token);
                                block.layout = None;
                                block.count = None;
                            }
                            None => {
                                let freed = self.m.ledger.free(a);
                                self.checked(freed)?;
                            }
                        }
                    }
                }
            }
            Op::Forget(r) => {
                let Some(&p) = self.roots.get(&r) else {
                    refuse!("T17 forget: root r{r} holds no value");
                };
                if self.guarded(Holder::Root(r)) {
                    refuse!("T17 forget: root r{r} still guards a live borrow");
                }
                if self.forgot.contains_key(&r) {
                    refuse!("T17 forget: forget ledger entry {r} is in use");
                }
                self.take_root(r);
                self.forgot.insert(r, p);
                let forgotten = self.m.forget(Val::Ptr(p));
                self.checked(forgotten)?;
            }
            Op::RegionOpen(g) => {
                if self.regions.iter().any(|(id, _)| *id == g) {
                    refuse!("T18 region-open: region g{g} is already open");
                }
                let rt = self.m.ledger.regions.len();
                self.m.ledger.regions.push(LedgerRegion {
                    instance: RegionInstanceId(g as u32),
                    live: true,
                    allocations: Default::default(),
                    budget: None,
                });
                self.m.ledger.trace.push(Event::RegionEnter { region: rt });
                self.regions.insert(0, (g, rt));
            }
            Op::RegionAlloc(g, l, n) => {
                let Some(&(_, rt)) = self.regions.iter().find(|(id, _)| *id == g) else {
                    refuse!("T19 region-alloc: region g{g} is not open");
                };
                if self.locals.contains_key(&l) {
                    refuse!("T19 region-alloc: local l{l} is already live");
                }
                let address = self.free_address();
                let layout = self.object(n, false);
                let size = (n + 1) * STRIDE;
                let p = self
                    .m
                    .ledger
                    .alloc(Owner::Region(rt), Some(layout), size, size, 8, None);
                self.m.ledger.regions[rt].allocations.push(p.alloc);
                self.born(address, p, (Kind::Region(g), n, false));
                self.locals.insert(l, p);
            }
            Op::RegionStore(l, k, r) => {
                let Some(&local) = self.locals.get(&l) else {
                    refuse!("T20 region-store: local l{l} is not live");
                };
                let a = local.alloc;
                if self.m.ledger.get(a).published {
                    refuse!("T20 region-store: @{} is published", self.at(a));
                }
                if self.shape[&a].1 <= k {
                    refuse!("T20 region-store: @{} has no field {k}", self.at(a));
                }
                if self.field(a, k).is_some() {
                    refuse!(
                        "T20 region-store: field {k} of @{} is already initialized",
                        self.at(a)
                    );
                }
                let Some(&pr) = self.roots.get(&r) else {
                    refuse!("T20 region-store: root r{r} holds no value");
                };
                if self.guarded(Holder::Root(r)) {
                    refuse!("T20 region-store: root r{r} is borrowed");
                }
                self.take_root(r);
                self.put_cell(a, k, pr)?;
                self.clock += 1;
                self.stored.insert((a, k), self.clock);
            }
            Op::LocalCopy(l, l2) => {
                let Some(&local) = self.locals.get(&l) else {
                    refuse!("T21 local-copy: local l{l} is not live");
                };
                if self.locals.contains_key(&l2) {
                    refuse!("T21 local-copy: local l{l2} is already live");
                }
                self.m.ledger.get_mut(local.alloc).published = true;
                self.locals.insert(l2, local);
            }
            Op::LocalDrop(l) => {
                if !self.locals.contains_key(&l) {
                    refuse!("T22 local-drop: local l{l} is not live");
                }
                if self.views.values().any(|v| v.parent == Parent::Local(l)) {
                    refuse!("T22 local-drop: local l{l} still has a live borrow");
                }
                self.locals.remove(&l);
            }
            Op::RegionClose(g) => {
                let Some(&(innermost, rt)) = self.regions.first() else {
                    refuse!("T23 region-close: no region is open");
                };
                if innermost != g {
                    refuse!("T23 region-close: region g{g} is not the innermost open region");
                }
                let blocks = self.m.ledger.regions[rt].allocations.clone();
                if self.locals.values().any(|p| blocks.contains(&p.alloc)) {
                    refuse!("T23 region-close: a local reference into region g{g} is still live");
                }
                // Each outgoing heap credit is released once, most recently stored first.
                let mut outgoing: Vec<(u64, AllocId, u64)> = self
                    .stored
                    .iter()
                    .filter(|((a, _), _)| blocks.contains(a))
                    .map(|((a, k), t)| (*t, *a, *k))
                    .collect();
                outgoing.sort_by_key(|x| std::cmp::Reverse(x.0));
                for (_, a, k) in outgoing {
                    self.stored.remove(&(a, k));
                    if self.field(a, k).is_some() {
                        let child = self.take_cell(a, k);
                        self.release(child)?;
                    }
                }
                for a in blocks {
                    let freed = self.m.ledger.free(a);
                    self.checked(freed)?;
                }
                self.m.ledger.regions[rt].live = false;
                self.m.ledger.trace.push(Event::RegionExit { region: rt });
                self.regions.remove(0);
            }
        }
        Ok(Ok(()))
    }

    fn put_root(&mut self, id: u64, ptr: Ptr) {
        self.m.set_root(
            id,
            Some(Held {
                val: Val::Ptr(ptr),
                credits: 1,
                borrow: None,
            }),
        );
        self.roots.insert(id, ptr);
    }

    fn take_root(&mut self, id: u64) {
        self.m.set_root(id, None);
        self.roots.remove(&id);
    }

    /// The executor's own invariant: every block's count equals its ideal holders.
    fn audit(&mut self) -> Result<(), String> {
        self.m.steps += 1;
        let counts = self.m.check_counts();
        self.checked(counts)
    }

    fn holder(&self, h: Holder) -> String {
        match h {
            Holder::Root(r) => format!("r{r}"),
            Holder::Field(a, k) => format!("@{}.{k}", self.at(a)),
            Holder::Forgot(n) => format!("forget {n}"),
        }
    }

    fn render(&self) -> String {
        let mut out = String::from("heap\n");
        for (address, &a) in self.slots.iter().enumerate() {
            let block = self.m.ledger.get(a);
            let generation = self.births[address];
            let (kind, arity, finalizer) = self.shape[&a];
            let line = match block.state {
                AllocState::Dead => format!("@{address} free gen {generation}"),
                AllocState::Raw(_) => format!("@{address} raw gen {generation} capacity {arity}"),
                state => {
                    let state = if state == AllocState::Dying {
                        "dying"
                    } else {
                        "live"
                    };
                    let count = match (kind, block.count) {
                        (Kind::Region(_), _) => "uncounted".to_string(),
                        (_, Some(Count::Finite(0)) | None) => "count dead".to_string(),
                        (_, Some(Count::Finite(n))) => format!("count fin {n}"),
                        (_, Some(Count::Immortal)) => "count immortal".to_string(),
                        (_, Some(Count::Root(_) | Count::Frozen)) => {
                            unreachable!("the vector machine has no Shared roots")
                        }
                    };
                    let region = matches!(kind, Kind::Region(_));
                    let kind = match kind {
                        Kind::Heap => "heap".to_string(),
                        Kind::Static => "static".to_string(),
                        Kind::Region(g) => format!("region g{g}"),
                    };
                    let mut flags = String::new();
                    if finalizer {
                        flags += " fin";
                    }
                    if self
                        .resource(a)
                        .is_some_and(|r| r.state != ResourceState::Live)
                    {
                        flags += " finalized";
                    }
                    if let Some(t) = self.resets.get(&a) {
                        flags += &format!(" reset t{t}");
                    }
                    // The model's flag is MEM-10's region publication; the
                    // executor also marks static storage published (never written).
                    if block.published && region {
                        flags += " published";
                    }
                    format!(
                        "@{address} {state} gen {generation} {count} {kind} arity {arity}{flags}"
                    )
                }
            };
            out += &format!("  {line}\n");
        }
        let mut credits: Vec<(Holder, Ptr)> = Vec::new();
        credits.extend(self.roots.iter().map(|(r, p)| (Holder::Root(*r), *p)));
        for &a in &self.slots {
            let block = self.m.ledger.get(a);
            if matches!(block.state, AllocState::Live | AllocState::Dying) {
                for (o, c) in &block.extent.cells {
                    if let Cell::Data {
                        ptr, counted: true, ..
                    } = c
                    {
                        credits.push((Holder::Field(a, o / STRIDE), *ptr));
                    }
                }
            }
        }
        credits.extend(self.forgot.iter().map(|(n, p)| (Holder::Forgot(*n), *p)));
        let key = |h: &Holder| match *h {
            Holder::Root(r) => (0, r, 0),
            Holder::Field(a, k) => (1, self.at(a) as u64, k),
            Holder::Forgot(n) => (2, n, 0),
        };
        credits.sort_by_key(|(h, _)| key(h));
        out += "credits\n";
        for (h, p) in credits {
            out += &format!("  {} -> {}\n", self.holder(h), self.name(p));
        }
        out += "borrows\n";
        for (id, v) in &self.views {
            let parent = match v.parent {
                Parent::None => "-".to_string(),
                Parent::Borrow(p) => format!("b{p}"),
                Parent::Local(l) => format!("l{l}"),
            };
            out += &format!(
                "  b{id} guard {} parent {parent} -> {}\n",
                self.holder(v.guard),
                self.name(v.target)
            );
        }
        out += "locals\n";
        for (id, p) in &self.locals {
            out += &format!("  l{id} -> {}\n", self.name(*p));
        }
        out += "tokens\n";
        for (id, token) in &self.tokens {
            let value = match token {
                None => "none".to_string(),
                Some(token) => {
                    let donor = self.m.ledger.tokens[token.0 as usize].alloc;
                    match self.m.ledger.get(donor).state {
                        AllocState::Raw(_) => format!(
                            "raw @{}#{} capacity {}",
                            self.at(donor),
                            self.births[self.at(donor)],
                            self.shape[&donor].1
                        ),
                        _ => format!("pending @{}", self.at(donor)),
                    }
                }
            };
            out += &format!("  t{id} {value}\n");
        }
        out += "work\n";
        for a in &self.work {
            out += &format!("  @{}\n", self.at(*a));
        }
        out += "regions\n";
        for (g, rt) in &self.regions {
            let mut blocks: Vec<usize> = self.m.ledger.regions[*rt]
                .allocations
                .iter()
                .map(|a| self.at(*a))
                .collect();
            blocks.sort();
            let blocks: Vec<String> = blocks.iter().map(|a| format!("@{a}")).collect();
            out += &format!("  g{g} blocks [{}]\n", blocks.join(", "));
        }
        out += "events\n";
        for (a, generation, ok) in &self.events {
            out += &format!(
                "  finalize @{}#{generation} {}\n",
                self.at(*a),
                if *ok { "ok" } else { "fail" }
            );
        }
        out
    }
}

/// The layouts a vector needs: one object per (arity, finalizer) it names, and
/// the pointer cell layout under the key `(u64::MAX, false)`.
fn layouts(ops: &[(usize, Op)]) -> (TargetSpec, LayoutTable, BTreeMap<(u64, bool), LayoutId>) {
    let mut l = Layouts::new(TargetSpec::x86_64_unknown_linux_gnu());
    let any = l.unit();
    let reference = l.block(any);
    let mut shapes: Vec<(u64, bool)> = vec![(0, false)];
    for (_, op) in ops {
        match *op {
            Op::Alloc(_, n, f) | Op::Reuse(_, _, n, f) => shapes.push((n, f)),
            Op::RegionAlloc(_, _, n) => shapes.push((n, false)),
            _ => {}
        }
    }
    shapes.sort();
    shapes.dedup();
    let mut objects = BTreeMap::new();
    objects.insert((u64::MAX, false), reference);
    for (arity, finalizer) in shapes {
        let id = l.record(&[]);
        let record = l.get_mut(id);
        record.kind = if finalizer {
            LayoutKind::Resource
        } else {
            LayoutKind::Aggregate
        };
        record.size = SizeSpec::Fixed((arity + 1) * STRIDE);
        record.align = 8;
        record.fields = (0..arity)
            .map(|k| FieldLayout {
                ordinal: k as u32,
                offset: k * STRIDE,
                layout: reference,
                unaligned: false,
            })
            .collect();
        record.scan = (0..arity)
            .map(|k| ScanStep::Counted { offset: k * STRIDE })
            .collect();
        record.drop = finalizer
            .then_some(DropStep::Finalize)
            .into_iter()
            .chain(
                (0..arity)
                    .rev()
                    .map(|k| DropStep::Release { offset: k * STRIDE }),
            )
            .collect();
        record.finalizer = finalizer.then(|| FinalizerRef {
            definition: path("vector.finalizer"),
            body: 0,
            unsafe_ffi_package: false,
        });
        objects.insert((arity, finalizer), id);
    }
    let (target, table) = l.finish();
    (target, table, objects)
}

/// Run an `rc-trace v1` vector on the heap executor's ledger and print the
/// outcome and final (or stuck) state in the vector output format. `Err` is a
/// malformed vector or a violation of the executor's own invariants.
pub fn rc_trace(text: &str) -> Result<String, String> {
    rc_trace_with_config(text, &Config::default())
}

pub fn rc_trace_with_config(text: &str, options: &Config) -> Result<String, String> {
    let (max, ops) = parse(text)?;
    let (target, table, objects) = layouts(&ops);
    let program = Program::<Ready> {
        target,
        layouts: table,
        units: Vec::new(),
        statics: Vec::new(),
    };
    let config = Config {
        count_max: max - 1,
        ..options.clone()
    };
    let mut ops_model = NoOperations;
    let mut r = Runner {
        m: Machine::new(&program, &config, &mut ops_model),
        objects: &objects,
        slots: Vec::new(),
        births: Vec::new(),
        address: BTreeMap::new(),
        generation: BTreeMap::new(),
        shape: BTreeMap::new(),
        roots: BTreeMap::new(),
        forgot: BTreeMap::new(),
        views: BTreeMap::new(),
        locals: BTreeMap::new(),
        tokens: BTreeMap::new(),
        resets: BTreeMap::new(),
        work: Vec::new(),
        regions: Vec::new(),
        events: Vec::new(),
        stored: BTreeMap::new(),
        clock: 0,
    };
    let header = format!("rc-trace v1 max {max}\n");
    for (index, (line, op)) in ops.iter().enumerate() {
        if let Err(message) = r.step(*op)? {
            return Ok(format!(
                "{header}error at step {} (line {line}): {message}\n{}",
                index + 1,
                r.render()
            ));
        }
        r.audit()
            .map_err(|e| format!("step {} (line {line}): {e}", index + 1))?;
    }
    Ok(format!(
        "{header}ok after {} steps\n{}",
        ops.len(),
        r.render()
    ))
}
