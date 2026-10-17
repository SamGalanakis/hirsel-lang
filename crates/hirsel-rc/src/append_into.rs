//! Results built only to be appended are built where they are appended.
//! A call whose `Bytes` or `String`
//! result is used only to be copied whole, byte by byte, onto the end of a
//! list `D` becomes a call that appends the same bytes to `D` itself, and its
//! result slot receives the static empty value, which the copy then skips:
//!
//! ```text
//! let line = number(v)                          number$into(v, mut output)
//! for j in 0u64..bytes.len(line) {       =>     let line = b""
//!   list.push(mut output, line[j])              for j in 0u64..0 { .. }
//! }
//! ```
//!
//! The copy is a *copy loop*, the range loop `for k in 0..bytes.len(V)`
//! whose only effect is `list.push(mut D, V[k])`, or a call to a *copy
//! function*, one whose whole body is a copy loop from a `Bytes` parameter
//! onto a `mut List[u8]` parameter. `V` is the result itself, or for a
//! `String` result its `string.as_bytes` view, which then views the static
//! empty bytes without the bridge call.
//!
//! Two kinds of producer have an appending form:
//! - std@1's `T.to_string(v)` appends as `T.format_into(v, mut D)`: std's
//!   contract is the same text, and no failure but allocation exhaustion.
//! - A *builder*: a function returning `bytes.from_list(take L)` of a local
//!   list `L` created empty and otherwise only appended to. Its derived
//!   `f$into(.., mut out: List[u8]) -> ()` is the same body with `L` replaced
//!   by the trailing parameter, the freeze and its result removed.
//!
//! Exactness. The bytes reach `D` in the same order; capacity is
//! unobservable. A failure (FAIL-1) runs cleanup, and a finalizer could then
//! read `D`, so a fused program must not fail where the original had not yet
//! appended: between the call and the copy every instruction is total and
//! leaves `D`'s root untouched, the producer's arguments do not name `D`, and
//! a builder raises no failure after its first append. Allocation
//! exhaustion ends the process (FAIL-1, environmental), so allocating
//! operations count as total.
//!
//! The rewrite is TIR to TIR over the `Program` before instance expansion:
//! every later pass, and C, sees an ordinary call and an ordinary function.
use crate::instance::Program;
use hirsel_syntax::{BinaryOp, ContentHash, DefPath, ItemPath, PassingMode, UnaryOp};
use hirsel_tir::{
    self as tir, Body, CallArgument, CallReturn, Callee, CleanupAction, CleanupId, CleanupNodeId,
    Constant, Definition, Instruction, InstructionKind as K, LoanId, Local, LocalId, Operand,
    Place, PlaceId, Rvalue, RvalueId, Storage, Terminator,
};
use hirsel_types::{
    BinderRef, BuiltinType, GenericArgument, Origin, OriginRoot, ParamId, Parameter, Primitive,
    Signature, SignatureKind, Type, TypeId, TypeKind, TypeStore, UseKind,
};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;

/// The suffix naming a builder's derived appending form.
pub const INTO_SUFFIX: &str = "$into";

/// `program` with every fusable call site rewritten and the appending forms
/// those sites call added. A program without such a site is returned as is.
pub fn fuse(program: &Program) -> Program {
    let mut p = program.clone();
    let mut fuser = Fuser::default();
    loop {
        let mut changed = false;
        let paths: Vec<DefPath> = p.definitions.keys().cloned().collect();
        for path in paths {
            if fuser.fuse_definition(&mut p, &path) {
                changed = true;
            }
        }
        if !changed {
            break;
        }
        // Bodies changed: forget what was concluded from them.
        fuser.builders.clear();
        fuser.totals.clear();
    }
    p
}

#[derive(Default)]
struct Fuser {
    /// Builders already given an appending form: `f` to `f$into`.
    derived: BTreeMap<DefPath, DefPath>,
    /// Memoized copy-function summaries: (list parameter, bytes parameter).
    copies: HashMap<DefPath, Option<(u32, u32)>>,
    /// Memoized: every path from entry to return is total.
    totals: HashMap<DefPath, bool>,
    /// Memoized builder recognition.
    builders: HashMap<DefPath, bool>,
}

/// std@1's `T.format_into` for std@1's `T.to_string`, when both are present.
fn std_appender(program: &Program, path: &DefPath) -> Option<DefPath> {
    let p = &path.0;
    if !p.package.package_is("std", "1") || p.name != "to_string" {
        return None;
    }
    let into = DefPath(ItemPath {
        package: p.package.clone(),
        modules: p.modules.clone(),
        name: "format_into".into(),
    });
    (program.definitions.contains_key(&into) && program.signatures.contains_key(&into))
        .then_some(into)
}

fn is_std_format_into(path: &DefPath) -> bool {
    let p = &path.0;
    p.package.package_is("std", "1") && p.name == "format_into"
}

fn into_path(path: &DefPath) -> DefPath {
    let p = &path.0;
    DefPath(ItemPath {
        package: p.package.clone(),
        modules: p.modules.clone(),
        name: format!("{}{INTO_SUFFIX}", p.name),
    })
}

fn kind(types: &TypeStore, ty: TypeId) -> Option<&TypeKind> {
    types.get(ty).map(|t| &t.kind)
}

fn is_primitive(types: &TypeStore, ty: TypeId, want: Primitive) -> bool {
    matches!(kind(types, ty), Some(TypeKind::Primitive(p)) if *p == want)
}

fn is_unit(types: &TypeStore, ty: TypeId) -> bool {
    matches!(kind(types, ty), Some(TypeKind::Tuple(items)) if items.is_empty())
}

fn is_byte_list(types: &TypeStore, ty: TypeId) -> bool {
    match kind(types, ty) {
        Some(TypeKind::Builtin {
            kind: BuiltinType::List,
            arguments,
        }) => {
            matches!(&arguments[..], [GenericArgument::Type(e)] if is_primitive(types, *e, Primitive::U8))
        }
        _ => false,
    }
}

/// A value whose destruction runs no user code.
fn silent(types: &TypeStore, ty: TypeId) -> bool {
    match kind(types, ty) {
        Some(TypeKind::Primitive(_)) => true,
        Some(TypeKind::Tuple(items)) => items.iter().all(|t| silent(types, *t)),
        Some(TypeKind::Box(t)) => silent(types, *t),
        Some(TypeKind::Builtin {
            kind: BuiltinType::List | BuiltinType::Array | BuiltinType::Inline,
            arguments,
        }) => arguments.iter().all(|a| match a {
            GenericArgument::Type(t) => silent(types, *t),
            GenericArgument::Size(_) => true,
        }),
        _ => false,
    }
}

/// Where a reference to a local occurs.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum At {
    Inst(usize, usize),
    Term(usize),
}

/// One reference to a local's storage.
#[derive(Clone, Copy, Debug)]
enum Ref {
    /// StorageLive, StorageDead, Drop (also in cleanups), BindOrigin, EndOrigin.
    Inert,
    /// The destination of `Assign`.
    Assigned {
        at: At,
        value: RvalueId,
        place: PlaceId,
    },
    /// A whole `Take` moved whole into another local.
    Moved,
    /// Any other operand.
    Operand {
        at: At,
        place: PlaceId,
        usage: UseKind,
    },
    /// A call argument.
    Argument {
        block: usize,
        place: PlaceId,
        parameter: u32,
        usage: UseKind,
    },
    /// A call's result destination.
    Result { block: usize },
    /// `BeginLoan` of a place rooted here.
    Loan {
        id: LoanId,
        place: PlaceId,
        mutable: bool,
    },
    /// The operand of a `Return`.
    Returned,
    /// Defuse, Replace, Snapshot, BindFields, match scrutinees.
    Other,
}

/// Every reference of every local, and the whole-move webs.
struct Refs {
    by_local: Vec<Vec<Ref>>,
    parent: Vec<usize>,
}

impl Refs {
    fn new(body: &Body) -> Self {
        let n = body.locals.len();
        let mut r = Self {
            by_local: vec![Vec::new(); n],
            parent: (0..n).collect(),
        };
        let root = |p: PlaceId| body.places[p.0 as usize].root.0 as usize;
        let whole = |p: PlaceId| body.places[p.0 as usize].projection.is_empty();
        let add = |r: &mut Self, p: PlaceId, x: Ref| r.by_local[root(p)].push(x);
        fn operand(r: &mut Refs, body: &Body, at: At, op: &Operand) {
            let l = body.places[op.place.0 as usize].root.0 as usize;
            r.by_local[l].push(Ref::Operand {
                at,
                place: op.place,
                usage: op.usage,
            });
        }
        for (b, block) in body.blocks.iter().enumerate() {
            for (i, inst) in block.instructions.iter().enumerate() {
                let at = At::Inst(b, i);
                match &inst.kind {
                    K::StorageLive(l) => r.by_local[l.0 as usize].push(Ref::Inert),
                    K::StorageDead { place, .. } | K::Drop { place, .. } | K::EndOrigin(place) => {
                        add(&mut r, *place, Ref::Inert)
                    }
                    K::BindOrigin { source, place, .. } => {
                        add(&mut r, *source, Ref::Inert);
                        add(&mut r, *place, Ref::Inert);
                    }
                    K::Assign { destination, value } => {
                        add(
                            &mut r,
                            *destination,
                            Ref::Assigned {
                                at,
                                value: *value,
                                place: *destination,
                            },
                        );
                        match &body.rvalues[value.0 as usize] {
                            Rvalue::Use(op)
                                if op.usage == UseKind::Take
                                    && whole(op.place)
                                    && whole(*destination) =>
                            {
                                add(&mut r, op.place, Ref::Moved);
                                let (a, b) = (root(op.place), root(*destination));
                                let (a, b) = (r.find(a), r.find(b));
                                r.parent[a] = b;
                            }
                            Rvalue::Use(op) => operand(&mut r, body, at, op),
                            Rvalue::Unary { value, .. } => operand(&mut r, body, at, value),
                            Rvalue::Binary { left, right, .. } => {
                                operand(&mut r, body, at, left);
                                operand(&mut r, body, at, right);
                            }
                            Rvalue::Construct { fields, .. } => {
                                for f in fields {
                                    operand(&mut r, body, at, &f.value);
                                }
                            }
                            Rvalue::Closure { captures, .. } => {
                                for c in captures {
                                    operand(&mut r, body, at, c);
                                }
                            }
                            _ => {}
                        }
                    }
                    K::BeginLoan {
                        id, place, mutable, ..
                    } => add(
                        &mut r,
                        *place,
                        Ref::Loan {
                            id: *id,
                            place: *place,
                            mutable: *mutable,
                        },
                    ),
                    K::Check { condition, .. } => operand(&mut r, body, at, condition),
                    K::Replace {
                        destination,
                        value,
                        displaced,
                        ..
                    } => {
                        add(&mut r, *destination, Ref::Other);
                        add(&mut r, *displaced, Ref::Other);
                        operand(&mut r, body, at, value);
                    }
                    K::Snapshot { destination, value } => {
                        add(&mut r, *destination, Ref::Other);
                        operand(&mut r, body, at, value);
                    }
                    K::Defuse { place } => add(&mut r, *place, Ref::Other),
                    K::BindFields { source, fields } => {
                        add(&mut r, *source, Ref::Other);
                        for f in fields {
                            add(&mut r, *f, Ref::Other);
                        }
                    }
                    K::EndScope(_) | K::EndLoan { .. } | K::RunCleanup(_) => {}
                }
            }
            let at = At::Term(b);
            match &block.terminator {
                Terminator::Branch { condition, .. } => operand(&mut r, body, at, condition),
                Terminator::Switch { value, .. } => operand(&mut r, body, at, value),
                Terminator::Match { table, .. } => {
                    let t = &body.matches[table.0 as usize];
                    add(&mut r, t.scrutinee, Ref::Other);
                    for p in &t.patterns {
                        add(&mut r, p.place, Ref::Other);
                    }
                }
                Terminator::Call {
                    callee,
                    arguments,
                    returns,
                    ..
                } => {
                    if let Callee::Indirect { value, .. } = callee {
                        operand(&mut r, body, at, value);
                    }
                    for a in arguments {
                        add(
                            &mut r,
                            a.value.place,
                            Ref::Argument {
                                block: b,
                                place: a.value.place,
                                parameter: a.parameter.0,
                                usage: a.value.usage,
                            },
                        );
                    }
                    if let Some(ret) = returns {
                        add(&mut r, ret.destination, Ref::Result { block: b });
                    }
                }
                Terminator::Return { value, .. } => add(&mut r, value.place, Ref::Returned),
                Terminator::Goto(_) | Terminator::Fail { .. } | Terminator::Unreachable => {}
            }
        }
        for node in &body.cleanup_nodes {
            if let CleanupAction::Drop { place, .. } = &node.action {
                add(&mut r, *place, Ref::Inert);
            } else if let CleanupAction::CloseArena(place) = &node.action {
                add(&mut r, *place, Ref::Other);
            } else if let CleanupAction::JoinTasks { scope, .. } = &node.action {
                add(&mut r, *scope, Ref::Other);
            }
        }
        r
    }

    fn find(&mut self, mut x: usize) -> usize {
        while self.parent[x] != x {
            self.parent[x] = self.parent[self.parent[x]];
            x = self.parent[x];
        }
        x
    }

    /// The locals joined to `l` by whole moves.
    fn web(&mut self, l: LocalId) -> Vec<LocalId> {
        let w = self.find(l.0 as usize);
        (0..self.parent.len())
            .filter(|&x| self.find(x) == w)
            .map(|x| LocalId(x as u32))
            .collect()
    }
}

fn place(body: &Body, p: PlaceId) -> &Place {
    &body.places[p.0 as usize]
}

fn root(body: &Body, p: PlaceId) -> LocalId {
    place(body, p).root
}

fn whole(body: &Body, p: PlaceId) -> bool {
    place(body, p).projection.is_empty()
}

fn callee_path(t: &Terminator) -> Option<&DefPath> {
    match t {
        Terminator::Call {
            callee: Callee::Definition { path, .. },
            ..
        } => Some(path),
        _ => None,
    }
}

fn argument(arguments: &[CallArgument], parameter: u32) -> Option<&Operand> {
    arguments
        .iter()
        .find(|a| a.parameter.0 == parameter)
        .map(|a| &a.value)
}

/// One recognized copy loop: `for k in 0..bytes.len(V) { list.push(mut D, V[k]) }`.
#[derive(Clone, Debug)]
struct CopyLoop {
    header: usize,
    blocks: [usize; 3],
    exit: usize,
    /// The block whose call measures the source.
    measure: usize,
    /// The source `V` and the loan of its measurement.
    source: LocalId,
    /// The `BeginLoan` place of the push's list, and its loan.
    dest: PlaceId,
    dest_loan: LoanId,
}

/// The single definitions of a local: each `Assign` destination occurrence.
fn defs(refs: &Refs, l: LocalId) -> Vec<(At, RvalueId)> {
    refs.by_local[l.0 as usize]
        .iter()
        .filter_map(|r| match r {
            Ref::Assigned {
                at,
                value,
                place: _,
            } => Some((*at, *value)),
            _ => None,
        })
        .collect()
}

/// `l`'s value, through whole moves, is the value `start` holds at the
/// start of the chain. Each local on the way is assigned exactly once.
fn moved_from(body: &Body, refs: &Refs, mut l: LocalId, sources: &[LocalId]) -> Option<LocalId> {
    for _ in 0..body.locals.len() {
        if sources.contains(&l) {
            return Some(l);
        }
        let d = defs(refs, l);
        let [(_, value)] = d[..] else { return None };
        match &body.rvalues[value.0 as usize] {
            Rvalue::Use(op)
                if matches!(op.usage, UseKind::Take | UseKind::Copy) && whole(body, op.place) =>
            {
                l = root(body, op.place);
            }
            _ => return None,
        }
    }
    None
}

fn copy_loop(body: &Body, refs: &Refs, header: usize) -> Option<CopyLoop> {
    let h = &body.blocks[header];
    let Terminator::Branch { condition, yes, no } = &h.terminator else {
        return None;
    };
    let (run, exit) = (yes.0 as usize, no.0 as usize);
    if condition.usage != UseKind::Take || !whole(body, condition.place) {
        return None;
    }
    let c = root(body, condition.place);
    let [
        Instruction {
            kind: K::StorageLive(live),
            ..
        },
        Instruction {
            kind: K::Assign { destination, value },
            ..
        },
    ] = &h.instructions[..]
    else {
        return None;
    };
    if *live != c || root(body, *destination) != c || !whole(body, *destination) {
        return None;
    }
    let Rvalue::Binary {
        op: BinaryOp::Lt,
        left,
        right,
    } = &body.rvalues[value.0 as usize]
    else {
        return None;
    };
    if left.usage != UseKind::Copy || right.usage != UseKind::Copy {
        return None;
    }
    if !whole(body, left.place) || !whole(body, right.place) {
        return None;
    }
    let (counter, end) = (root(body, left.place), root(body, right.place));
    // Run: transports of the counter, one indexed read, one push.
    let r = &body.blocks[run];
    let Terminator::Call {
        callee: Callee::Definition { path, .. },
        arguments,
        returns: Some(CallReturn { next, .. }),
        ..
    } = &r.terminator
    else {
        return None;
    };
    if !path.is_core("list.push") || arguments.len() != 2 {
        return None;
    }
    let (list, element) = (argument(arguments, 0)?, argument(arguments, 1)?);
    if list.usage != UseKind::Mut || element.usage != UseKind::Take {
        return None;
    }
    let dest_loan = place(body, list.place).loan?;
    let mut dest = None;
    let mut read = None;
    for inst in &r.instructions {
        match &inst.kind {
            K::StorageLive(_) | K::BindOrigin { .. } => {}
            K::BeginLoan {
                id,
                place: p,
                mutable: true,
                ..
            } if *id == dest_loan && dest.is_none() => dest = Some(*p),
            K::Assign { destination, value } if whole(body, *destination) => {
                match &body.rvalues[value.0 as usize] {
                    Rvalue::Use(op) if whole(body, op.place) => {}
                    Rvalue::Use(op) if op.usage == UseKind::Copy && read.is_none() => {
                        let p = place(body, op.place);
                        let (1, Some(tir::Projection::Index(i))) =
                            (p.projection.len(), p.projection.first())
                        else {
                            return None;
                        };
                        read = Some((p.root, *i, root(body, *destination)));
                    }
                    _ => return None,
                }
            }
            _ => return None,
        }
    }
    let (dest, (source, index, element_value)) = (dest?, read?);
    let dp = place(body, dest);
    if dp
        .projection
        .iter()
        .any(|s| matches!(s, tir::Projection::Index(_)))
        || root(body, dest) != root(body, list.place)
        || dp.projection != place(body, list.place).projection
    {
        return None;
    }
    // The pushed element is the read; the index is the counter.
    if moved_from(body, refs, root(body, element.place), &[element_value])? != element_value {
        return None;
    }
    if moved_from(body, refs, index, &[counter])? != counter {
        return None;
    }
    // The push's continuation ends the loan and the scope, then steps.
    let after = next.0 as usize;
    let p = &body.blocks[after];
    let Terminator::Goto(step) = p.terminator else {
        return None;
    };
    for inst in &p.instructions {
        match &inst.kind {
            K::EndLoan { loan, .. } if *loan == dest_loan => {}
            K::StorageLive(_) | K::StorageDead { .. } | K::BindOrigin { .. } | K::EndScope(_) => {}
            K::Assign { destination, value } if whole(body, *destination) => {
                match &body.rvalues[value.0 as usize] {
                    Rvalue::Use(op) if whole(body, op.place) => {}
                    _ => return None,
                }
            }
            K::RunCleanup(c) if body.cleanup_actions(*c).next().is_none() => {}
            _ => return None,
        }
    }
    // Step: counter += 1, then back to the header.
    let step = step.0 as usize;
    let s = &body.blocks[step];
    if s.terminator != Terminator::Goto(tir::BlockId(header as u32)) {
        return None;
    }
    let mut stepped = false;
    let mut one = None;
    let mut sum = None;
    for inst in &s.instructions {
        match &inst.kind {
            K::StorageLive(_) | K::StorageDead { .. } => {}
            K::Assign { destination, value } if whole(body, *destination) => {
                match &body.rvalues[value.0 as usize] {
                    Rvalue::Constant(Constant::Integer(1)) => one = Some(root(body, *destination)),
                    Rvalue::Binary {
                        op: BinaryOp::Add,
                        left,
                        right,
                    } if left.usage == UseKind::Copy
                        && whole(body, left.place)
                        && root(body, left.place) == counter
                        && right.usage == UseKind::Take
                        && Some(root(body, right.place)) == one =>
                    {
                        sum = Some(root(body, *destination))
                    }
                    _ => return None,
                }
            }
            K::Replace {
                destination, value, ..
            } if !stepped
                && whole(body, *destination)
                && root(body, *destination) == counter
                && value.usage == UseKind::Take
                && Some(root(body, value.place)) == sum =>
            {
                stepped = true
            }
            _ => return None,
        }
    }
    if !stepped {
        return None;
    }
    // The counter starts at zero and only the step writes it; the end is the
    // source's length, measured before the loop and never rewritten.
    let mut counter_defs = 0;
    for r in &refs.by_local[counter.0 as usize] {
        match r {
            Ref::Assigned { value, .. } => {
                counter_defs += 1;
                match &body.rvalues[value.0 as usize] {
                    Rvalue::Use(op)
                        if op.usage == UseKind::Take
                            && whole(body, op.place)
                            && initial_constant(body, refs, root(body, op.place)) == Some(0) => {}
                    _ => return None,
                }
            }
            Ref::Other => {} // the step's Replace
            Ref::Operand {
                usage: UseKind::Copy,
                place,
                ..
            } if whole(body, *place) => {}
            Ref::Inert => {}
            _ => return None,
        }
    }
    if counter_defs != 1 {
        return None;
    }
    if refs.by_local[counter.0 as usize]
        .iter()
        .filter(|r| matches!(r, Ref::Other))
        .count()
        != 1
    {
        return None;
    }
    let measured = result_of(body, refs, end)?;
    let measure_block = &body.blocks[measured];
    let Terminator::Call {
        callee: Callee::Definition { path, .. },
        arguments,
        ..
    } = &measure_block.terminator
    else {
        return None;
    };
    if !path.is_core("bytes.len") || arguments.len() != 1 {
        return None;
    }
    let measured_source = argument(arguments, 0)?;
    if measured_source.usage != UseKind::Borrow
        || !whole(body, measured_source.place)
        || root(body, measured_source.place) != source
    {
        return None;
    }
    // Nothing writes the source: it is only read, borrowed, moved out of the
    // loop's way or released.
    for r in &refs.by_local[source.0 as usize] {
        match r {
            Ref::Inert | Ref::Loan { mutable: false, .. } | Ref::Moved => {}
            Ref::Assigned { .. } | Ref::Result { .. } => {}
            Ref::Operand { usage, .. } | Ref::Argument { usage, .. }
                if matches!(usage, UseKind::Copy | UseKind::Borrow) => {}
            _ => return None,
        }
    }
    if source_written_in_loop(body, source, &[header, run, after, step]) {
        return None;
    }
    Some(CopyLoop {
        header,
        blocks: [run, after, step],
        exit,
        measure: measured,
        source,
        dest,
        dest_loan,
    })
}

/// The constant integer the chain into `l` starts from.
fn initial_constant(body: &Body, refs: &Refs, mut l: LocalId) -> Option<u128> {
    for _ in 0..body.locals.len() {
        let d = defs(refs, l);
        let [(_, value)] = d[..] else { return None };
        match &body.rvalues[value.0 as usize] {
            Rvalue::Constant(Constant::Integer(v)) => return Some(*v),
            Rvalue::Use(op) if op.usage == UseKind::Take && whole(body, op.place) => {
                l = root(body, op.place)
            }
            _ => return None,
        }
    }
    None
}

/// The block whose call result reaches `l` through whole moves.
fn result_of(body: &Body, refs: &Refs, mut l: LocalId) -> Option<usize> {
    for _ in 0..body.locals.len() {
        let results: Vec<usize> = refs.by_local[l.0 as usize]
            .iter()
            .filter_map(|r| match r {
                Ref::Result { block } => Some(*block),
                _ => None,
            })
            .collect();
        let d = defs(refs, l);
        match (&results[..], &d[..]) {
            ([b], []) => return Some(*b),
            ([], [(_, value)]) => match &body.rvalues[value.0 as usize] {
                Rvalue::Use(op) if op.usage == UseKind::Take && whole(body, op.place) => {
                    l = root(body, op.place)
                }
                _ => return None,
            },
            _ => return None,
        }
    }
    None
}

fn source_written_in_loop(body: &Body, source: LocalId, blocks: &[usize]) -> bool {
    blocks.iter().any(|&b| {
        body.blocks[b].instructions.iter().any(|i| match &i.kind {
            K::Assign { destination, .. } => root(body, *destination) == source,
            K::Replace { destination, .. } => root(body, *destination) == source,
            _ => false,
        })
    })
}

/// Every copy loop of a body, by header block.
fn copy_loops(body: &Body, refs: &Refs) -> BTreeMap<usize, CopyLoop> {
    (0..body.blocks.len())
        .filter_map(|h| copy_loop(body, refs, h).map(|l| (h, l)))
        .collect()
}

/// Whether evaluating this rvalue can fail (FAIL-1) other than by exhaustion.
fn rvalue_fails(body: &Body, rvalue: &Rvalue) -> bool {
    let indexed = |op: &Operand| {
        place(body, op.place)
            .projection
            .iter()
            .any(|s| matches!(s, tir::Projection::Index(_)))
    };
    match rvalue {
        Rvalue::Use(op) => indexed(op),
        Rvalue::Unary { op, value } => indexed(value) || matches!(op, UnaryOp::Neg),
        Rvalue::Binary { op, left, right } => {
            indexed(left)
                || indexed(right)
                || !matches!(
                    op,
                    BinaryOp::Eq
                        | BinaryOp::Ne
                        | BinaryOp::Lt
                        | BinaryOp::Le
                        | BinaryOp::Gt
                        | BinaryOp::Ge
                        | BinaryOp::BitAnd
                        | BinaryOp::BitOr
                        | BinaryOp::BitXor
                        | BinaryOp::And
                        | BinaryOp::Or
                )
        }
        Rvalue::Construct { fields, .. } => fields.iter().any(|f| indexed(&f.value)),
        Rvalue::Closure { captures, .. } => captures.iter().any(indexed),
        Rvalue::Constant(_)
        | Rvalue::ReadConstant { .. }
        | Rvalue::FunctionReference { .. }
        | Rvalue::Size(_)
        | Rvalue::Shape(_) => false,
    }
}

fn cleanup_fails(body: &Body, types: &TypeStore, c: CleanupId) -> bool {
    body.cleanup_actions(c).any(|a| match a {
        CleanupAction::Drop { place: p, .. } => !silent(types, place(body, *p).ty),
        CleanupAction::EndLoan { .. } => false,
        _ => true,
    })
}

fn instruction_fails(body: &Body, types: &TypeStore, inst: &Instruction) -> bool {
    match &inst.kind {
        K::Assign { value, .. } => rvalue_fails(body, &body.rvalues[value.0 as usize]),
        K::Replace { value, .. } => rvalue_fails(body, &Rvalue::Use(value.clone())),
        K::Check { .. } => true,
        K::Drop { place: p, .. } => !silent(types, place(body, *p).ty),
        K::RunCleanup(c) => cleanup_fails(body, types, *c),
        K::Snapshot { value, .. } => rvalue_fails(body, &Rvalue::Use(value.clone())),
        K::StorageLive(_)
        | K::StorageDead { .. }
        | K::EndScope(_)
        | K::BeginLoan { .. }
        | K::EndLoan { .. }
        | K::BindOrigin { .. }
        | K::EndOrigin(_)
        | K::BindFields { .. }
        | K::Defuse { .. } => false,
    }
}

impl Fuser {
    /// Calls that cannot fail but by exhaustion.
    fn total_callee(&mut self, program: &Program, path: &DefPath) -> bool {
        if [
            "list.push",
            "list.reserve",
            "list.len",
            "bytes.len",
            "bytes.from_list",
            "string.as_bytes",
        ]
        .iter()
        .any(|n| path.is_core(n))
            || is_std_format_into(path)
        {
            return true;
        }
        if let Some(&t) = self.totals.get(path) {
            return t;
        }
        // Recursion is conservatively not total.
        self.totals.insert(path.clone(), false);
        let total = match program.definitions.get(path) {
            Some(def) if def.bodies.len() == 1 && def.contracts.is_empty() => {
                let body = &def.bodies[def.entry.0 as usize];
                body.tail_sites.is_empty() && self.body_total(program, &def.types, body)
            }
            _ => false,
        };
        self.totals.insert(path.clone(), total);
        total
    }

    fn body_total(&mut self, program: &Program, types: &TypeStore, body: &Body) -> bool {
        let refs = Refs::new(body);
        let loops = copy_loops(body, &refs);
        let skipped: HashSet<usize> = loops.values().flat_map(|l| l.blocks).collect();
        for (b, block) in body.blocks.iter().enumerate() {
            if skipped.contains(&b) {
                continue;
            }
            if block
                .instructions
                .iter()
                .any(|i| instruction_fails(body, types, i))
            {
                return false;
            }
            let ok = match &block.terminator {
                Terminator::Fail { .. } => false,
                Terminator::Call {
                    callee: Callee::Definition { path, .. },
                    ..
                } => self.total_callee(program, path),
                Terminator::Call { .. } => false,
                Terminator::Return { cleanup, .. } => !cleanup_fails(body, types, *cleanup),
                _ => true,
            };
            if !ok {
                return false;
            }
        }
        true
    }

    /// A copy function's (list parameter, bytes parameter).
    fn copy_function(&mut self, program: &Program, path: &DefPath) -> Option<(u32, u32)> {
        if let Some(c) = self.copies.get(path) {
            return *c;
        }
        let summary = copy_function(program, path);
        self.copies.insert(path.clone(), summary);
        summary
    }

    /// Rewrite `path`'s fusable sites; true when it changed.
    fn fuse_definition(&mut self, program: &mut Program, path: &DefPath) -> bool {
        let def = program.definitions[path].clone();
        let entry = def.entry.0 as usize;
        let mut sites = Vec::new();
        {
            let body = &def.bodies[entry];
            let mut refs = Refs::new(body);
            let loops = copy_loops(body, &refs);
            for (b, block) in body.blocks.iter().enumerate() {
                let Terminator::Call {
                    callee:
                        Callee::Definition {
                            path: callee,
                            arguments: generics,
                        },
                    returns: Some(_),
                    ..
                } = &block.terminator
                else {
                    continue;
                };
                if !generics.is_empty() || callee == path {
                    continue;
                }
                let producer = if let Some(into) = std_appender(program, callee) {
                    Producer::Std(into)
                } else if self.derived.contains_key(callee) || self.is_builder(program, callee) {
                    Producer::Derived(callee.clone())
                } else {
                    continue;
                };
                if let Some(site) =
                    self.site(program, &def.types, body, &mut refs, &loops, b, producer)
                {
                    sites.push(site);
                }
            }
        }
        if sites.is_empty() {
            return false;
        }
        let mut def = (*def).clone();
        let mut changed = false;
        for site in sites {
            let into = match &site.producer {
                Producer::Std(into) => into.clone(),
                Producer::Derived(f) => match self.derive(program, f) {
                    Some(into) => into,
                    None => continue,
                },
            };
            rewrite_site(&mut def, &site, into);
            changed = true;
        }
        if changed {
            program.definitions.insert(path.clone(), Arc::new(def));
        }
        changed
    }

    fn is_builder(&mut self, program: &Program, path: &DefPath) -> bool {
        if let Some(&b) = self.builders.get(path) {
            return b;
        }
        let b = builder(program, path, self).is_some();
        self.builders.insert(path.clone(), b);
        b
    }

    /// The fusion of the call ending `block`, if its result is only copied.
    #[allow(clippy::too_many_arguments)]
    fn site(
        &mut self,
        program: &Program,
        types: &TypeStore,
        body: &Body,
        refs: &mut Refs,
        loops: &BTreeMap<usize, CopyLoop>,
        block: usize,
        producer: Producer,
    ) -> Option<Site> {
        let Terminator::Call {
            arguments,
            returns: Some(ret),
            ..
        } = &body.blocks[block].terminator
        else {
            return None;
        };
        let result = root(body, ret.destination);
        let result_ty = body.locals[result.0 as usize].ty;
        let string = is_primitive(types, result_ty, Primitive::String);
        if !string && !is_primitive(types, result_ty, Primitive::Bytes) {
            return None;
        }
        let results = refs.web(result);
        // The bytes the copy reads: the result, or its one as_bytes view.
        let mut view_call = None;
        for l in &results {
            for r in &refs.by_local[l.0 as usize] {
                match r {
                    Ref::Inert | Ref::Moved | Ref::Assigned { .. } => {}
                    Ref::Result { block: b } if *b == block => {}
                    Ref::Loan { .. } | Ref::Operand { .. } | Ref::Argument { .. } if !string => {}
                    Ref::Loan { mutable: false, .. } => {}
                    Ref::Argument {
                        block: b,
                        usage: UseKind::Borrow,
                        parameter: 0,
                        ..
                    } if string
                        && callee_path(&body.blocks[*b].terminator)
                            .is_some_and(|p| p.is_core("string.as_bytes"))
                        && view_call.replace(*b).is_none() => {}
                    _ => return None,
                }
            }
        }
        // Each local of the web is assigned only by a move within it.
        for l in &results {
            for (_, value) in defs(refs, *l) {
                match &body.rvalues[value.0 as usize] {
                    Rvalue::Use(op)
                        if op.usage == UseKind::Take && results.contains(&root(body, op.place)) => {
                    }
                    _ => return None,
                }
            }
        }
        let views = match (string, view_call) {
            (false, _) => results.clone(),
            (true, Some(v)) => {
                let Terminator::Call {
                    returns: Some(r), ..
                } = &body.blocks[v].terminator
                else {
                    return None;
                };
                let views = refs.web(root(body, r.destination));
                for l in &views {
                    for (_, value) in defs(refs, *l) {
                        match &body.rvalues[value.0 as usize] {
                            Rvalue::Use(op)
                                if op.usage == UseKind::Take
                                    && views.contains(&root(body, op.place)) => {}
                            _ => return None,
                        }
                    }
                }
                views
            }
            (true, None) => return None,
        };
        // The consumer: a copy loop over the bytes, or a copy function's call.
        let mut consumer = None;
        for l in &views {
            for r in &refs.by_local[l.0 as usize] {
                match r {
                    Ref::Inert | Ref::Moved | Ref::Assigned { .. } => {}
                    Ref::Result { .. } => {}
                    Ref::Loan { mutable: false, .. } => {}
                    Ref::Operand {
                        at: At::Inst(b, _),
                        usage: UseKind::Copy,
                        ..
                    } => {
                        let l = loops
                            .values()
                            .find(|c| c.blocks[0] == *b && views.contains(&c.source))?;
                        if consumer
                            .replace(Consumer::Loop(l.clone()))
                            .is_some_and(|c| !c.is_loop(l))
                        {
                            return None;
                        }
                    }
                    Ref::Argument {
                        block: b,
                        usage: UseKind::Borrow,
                        parameter,
                        ..
                    } => {
                        let callee = callee_path(&body.blocks[*b].terminator)?;
                        if callee.is_core("bytes.len") {
                            // A copy loop's measurement.
                            if !loops
                                .values()
                                .any(|c| c.measure == *b && views.contains(&c.source))
                            {
                                return None;
                            }
                            continue;
                        }
                        if string && Some(*b) == view_call {
                            continue;
                        }
                        let (list, bytes) = self.copy_function(program, callee)?;
                        let Terminator::Call { arguments, .. } = &body.blocks[*b].terminator else {
                            return None;
                        };
                        if *parameter != bytes {
                            return None;
                        }
                        let target = argument(arguments, list)?;
                        let loan = place(body, target.place).loan?;
                        let dest = begin_place(body, loan)?;
                        if consumer
                            .replace(Consumer::Call {
                                block: *b,
                                dest,
                                loan,
                            })
                            .is_some()
                        {
                            return None;
                        }
                    }
                    _ => return None,
                }
            }
        }
        let consumer = consumer?;
        let (dest, end_block) = match &consumer {
            Consumer::Loop(l) => (l.dest, l.header),
            Consumer::Call { dest, block, .. } => (*dest, *block),
        };
        let dest_root = root(body, dest);
        if place(body, dest)
            .projection
            .iter()
            .any(|s| matches!(s, tir::Projection::Index(_)))
        {
            return None;
        }
        // The producer's arguments do not name the list.
        for a in arguments {
            if root(body, a.value.place) == dest_root {
                return None;
            }
            if let Some(loan) = place(body, a.value.place).loan
                && begin_place(body, loan).is_some_and(|p| root(body, p) == dest_root)
            {
                return None;
            }
        }
        // Between the call and the copy: straight-line, total, the list untouched.
        let consumer_loan = match &consumer {
            Consumer::Loop(l) => l.dest_loan,
            Consumer::Call { loan, .. } => *loan,
        };
        let mut b = ret.next.0 as usize;
        for _ in 0..body.blocks.len() {
            for inst in &body.blocks[b].instructions {
                if instruction_fails(body, types, inst) {
                    return None;
                }
                if touches(body, inst, dest_root, consumer_loan) {
                    return None;
                }
            }
            if b == end_block {
                return Some(Site {
                    block,
                    view: view_call,
                    producer,
                    dest,
                    consumer_place: match &consumer {
                        Consumer::Loop(l) => l.dest,
                        Consumer::Call { dest, .. } => *dest,
                    },
                });
            }
            b = match &body.blocks[b].terminator {
                Terminator::Goto(n) => n.0 as usize,
                Terminator::Call {
                    callee: Callee::Definition { path, .. },
                    arguments,
                    returns: Some(r),
                    ..
                } if (path.is_core("string.as_bytes") || path.is_core("bytes.len"))
                    && arguments
                        .iter()
                        .all(|a| root(body, a.value.place) != dest_root) =>
                {
                    r.next.0 as usize
                }
                _ => return None,
            };
        }
        None
    }

    /// `f$into`, derived and added on first use.
    fn derive(&mut self, program: &mut Program, f: &DefPath) -> Option<DefPath> {
        if let Some(into) = self.derived.get(f) {
            return Some(into.clone());
        }
        let shape = builder(program, f, self)?;
        let into = into_path(f);
        let def = &program.definitions[f];
        let derived = derive_body(def, &into, &shape);
        let signature = &program.signatures[f];
        let derived_signature = derive_signature(signature, &into, &derived);
        let tag =
            |h: &ContentHash| ContentHash::of_parts("hirsel-rc.append-into.v1", [h.0.as_slice()]);
        let mut derived = derived;
        derived.signature_hash = derived_signature.fingerprint;
        program
            .signatures
            .insert(into.clone(), Arc::new(derived_signature));
        program.definitions.insert(into.clone(), Arc::new(derived));
        if let Some(r) = program.revisions.get(f).copied() {
            program.revisions.insert(into.clone(), tag(&r));
        }
        if let Some(b) = program.binders.get(f).cloned() {
            program.binders.insert(into.clone(), b);
        }
        let inherited: Vec<_> = program
            .requirements
            .iter()
            .filter(|r| r.owner == *f)
            .cloned()
            .map(|mut r| {
                r.owner = into.clone();
                r
            })
            .collect();
        program.requirements.extend(inherited);
        self.derived.insert(f.clone(), into.clone());
        Some(into)
    }
}

#[derive(Clone, Debug)]
enum Producer {
    /// std@1 `T.to_string`, appending as this `T.format_into`.
    Std(DefPath),
    /// A builder, appending as its derived form.
    Derived(DefPath),
}

#[derive(Clone, Debug)]
enum Consumer {
    Loop(CopyLoop),
    Call {
        block: usize,
        dest: PlaceId,
        loan: LoanId,
    },
}

impl Consumer {
    fn is_loop(&self, l: &CopyLoop) -> bool {
        matches!(self, Self::Loop(x) if x.header == l.header)
    }
}

/// A fusable call site.
#[derive(Clone, Debug)]
struct Site {
    /// The block whose call produces the copied result.
    block: usize,
    /// The block whose `string.as_bytes` call views a `String` result.
    view: Option<usize>,
    producer: Producer,
    /// The list's `BeginLoan` place.
    dest: PlaceId,
    /// The consumer's list argument place, the template of the new one.
    consumer_place: PlaceId,
}

/// The place a loan begins on.
fn begin_place(body: &Body, loan: LoanId) -> Option<PlaceId> {
    body.blocks
        .iter()
        .flat_map(|b| &b.instructions)
        .find_map(|i| match &i.kind {
            K::BeginLoan { id, place, .. } if *id == loan => Some(*place),
            _ => None,
        })
}

/// Whether an instruction names `root` or the consumer's own loan.
fn touches(body: &Body, inst: &Instruction, list: LocalId, loan: LoanId) -> bool {
    let named = |p: &PlaceId| root(body, *p) == list;
    match &inst.kind {
        K::StorageLive(l) => *l == list,
        K::StorageDead { place, .. } | K::Drop { place, .. } | K::EndOrigin(place) => named(place),
        K::BindOrigin { source, place, .. } => named(source) || named(place),
        K::Assign { destination, value } => {
            named(destination)
                || match &body.rvalues[value.0 as usize] {
                    Rvalue::Use(op) => named(&op.place),
                    Rvalue::Unary { value, .. } => named(&value.place),
                    Rvalue::Binary { left, right, .. } => named(&left.place) || named(&right.place),
                    Rvalue::Construct { fields, .. } => {
                        fields.iter().any(|f| named(&f.value.place))
                    }
                    Rvalue::Closure { captures, .. } => captures.iter().any(|c| named(&c.place)),
                    _ => false,
                }
        }
        K::BeginLoan { place, id, .. } => named(place) && *id != loan,
        K::EndLoan { loan: l, .. } => *l == loan,
        K::Replace {
            destination,
            value,
            displaced,
            ..
        } => named(destination) || named(&value.place) || named(displaced),
        K::Snapshot { destination, value } => named(destination) || named(&value.place),
        K::Defuse { place } => named(place),
        K::BindFields { source, fields } => named(source) || fields.iter().any(named),
        K::Check { condition, .. } => named(&condition.place),
        K::RunCleanup(_) | K::EndScope(_) => false,
    }
}

/// A function whose whole body is one copy loop from its `Bytes` parameter
/// onto its `mut List[u8]` parameter: (list parameter, bytes parameter).
fn copy_function(program: &Program, path: &DefPath) -> Option<(u32, u32)> {
    let def = program.definitions.get(path)?;
    let SignatureKind::Function(f) = &program.signatures.get(path)?.kind else {
        return None;
    };
    if !f.generics.is_empty() || def.bodies.len() != 1 || !def.contracts.is_empty() {
        return None;
    }
    let body = &def.bodies[def.entry.0 as usize];
    let types = &def.types;
    if !body.tail_sites.is_empty() || !is_unit(types, body.result) {
        return None;
    }
    let refs = Refs::new(body);
    let loops = copy_loops(body, &refs);
    let [l] = &loops.values().collect::<Vec<_>>()[..] else {
        return None;
    };
    let param = |x: LocalId| {
        body.parameters
            .iter()
            .position(|p| *p == x)
            .map(|i| i as u32)
    };
    let list = param(root(body, l.dest))?;
    let bytes = param(l.source)?;
    if !whole(body, l.dest)
        || f.parameters.get(list as usize)?.passing != PassingMode::Mut
        || !is_byte_list(types, body.locals[root(body, l.dest).0 as usize].ty)
        || !is_primitive(types, body.locals[l.source.0 as usize].ty, Primitive::Bytes)
    {
        return None;
    }
    // Every other block only measures, stores storage and returns unit.
    let inside: HashSet<usize> = l.blocks.iter().copied().chain([l.header]).collect();
    for (b, block) in body.blocks.iter().enumerate() {
        if inside.contains(&b) {
            continue;
        }
        for inst in &block.instructions {
            match &inst.kind {
                K::Assign { value, .. } => match &body.rvalues[value.0 as usize] {
                    Rvalue::Use(op) if whole(body, op.place) => {}
                    Rvalue::Constant(_) => {}
                    _ => return None,
                },
                K::RunCleanup(c) if cleanup_fails(body, types, *c) => return None,
                K::BeginLoan { mutable: true, .. }
                | K::Check { .. }
                | K::Replace { .. }
                | K::Defuse { .. }
                | K::Snapshot { .. } => return None,
                K::Drop { place: p, .. } if !silent(types, place(body, *p).ty) => return None,
                _ => {}
            }
        }
        match &block.terminator {
            Terminator::Goto(_) | Terminator::Return { .. } => {}
            Terminator::Call { .. } if b == l.measure => {}
            _ => return None,
        }
    }
    Some((list, bytes))
}

/// A builder's shape: its list web, the freezing call and the result web.
struct Shape {
    list: Vec<LocalId>,
    freeze: usize,
    result: Vec<LocalId>,
}

/// `path` is a builder: its result is `bytes.from_list` of a local list that
/// is created empty and otherwise only appended to, and nothing can fail
/// after its first append.
fn builder(program: &Program, path: &DefPath, fuser: &mut Fuser) -> Option<Shape> {
    let def = program.definitions.get(path)?;
    let SignatureKind::Function(f) = &program.signatures.get(path)?.kind else {
        return None;
    };
    if !f.generics.is_empty()
        || def.bodies.len() != 1
        || !def.contracts.is_empty()
        || !def.protocol_associations.is_empty()
        || path.0.name.ends_with(INTO_SUFFIX)
    {
        return None;
    }
    let body = &def.bodies[def.entry.0 as usize];
    let types = &def.types;
    if !body.tail_sites.is_empty() || !is_primitive(types, body.result, Primitive::Bytes) {
        return None;
    }
    let mut refs = Refs::new(body);
    let freezes: Vec<usize> = body
        .blocks
        .iter()
        .enumerate()
        .filter(|(_, b)| callee_path(&b.terminator).is_some_and(|p| p.is_core("bytes.from_list")))
        .map(|(i, _)| i)
        .collect();
    let [freeze] = freezes[..] else { return None };
    let Terminator::Call {
        arguments,
        returns: Some(ret),
        ..
    } = &body.blocks[freeze].terminator
    else {
        return None;
    };
    let frozen = argument(arguments, 0)?;
    if frozen.usage != UseKind::Take || !whole(body, frozen.place) {
        return None;
    }
    let list = refs.web(root(body, frozen.place));
    let result = refs.web(root(body, ret.destination));
    if list.iter().any(|l| {
        !matches!(
            body.locals[l.0 as usize].storage,
            Storage::Local | Storage::Temporary
        ) || !is_byte_list(types, body.locals[l.0 as usize].ty)
    }) {
        return None;
    }
    let loops = copy_loops(body, &refs);
    // The list: created empty once, moved, appended to through mutable loans.
    let mut created = 0;
    let mut appends: HashSet<LoanId> = HashSet::new();
    for l in &list {
        for r in &refs.by_local[l.0 as usize] {
            match r {
                Ref::Inert | Ref::Moved => {}
                Ref::Assigned {
                    value, place: p, ..
                } if whole(body, *p) => match &body.rvalues[value.0 as usize] {
                    Rvalue::Construct {
                        constructor: tir::Constructor::Sequence,
                        fields,
                        ..
                    } if fields.is_empty() => created += 1,
                    Rvalue::Use(op)
                        if op.usage == UseKind::Take && list.contains(&root(body, op.place)) => {}
                    _ => return None,
                },
                Ref::Loan {
                    id,
                    mutable: true,
                    place: p,
                    ..
                } if whole(body, *p) => {
                    appends.insert(*id);
                }
                Ref::Argument {
                    block,
                    usage: UseKind::Take,
                    parameter: 0,
                    ..
                } if *block == freeze => {}
                Ref::Argument {
                    block,
                    usage: UseKind::Mut,
                    parameter,
                    place: p,
                } => {
                    let callee = callee_path(&body.blocks[*block].terminator)?;
                    let appending = (*parameter == 0
                        && (callee.is_core("list.push") || callee.is_core("list.reserve")))
                        || (is_std_format_into(callee) && *parameter == 1)
                        || (callee.0.name.ends_with(INTO_SUFFIX)
                            && Some(*parameter as usize + 1)
                                == program.signatures.get(callee).and_then(|s| match &s.kind {
                                    SignatureKind::Function(f) => Some(f.parameters.len()),
                                    _ => None,
                                }))
                        || fuser
                            .copy_function(program, callee)
                            .is_some_and(|(list, _)| list == *parameter);
                    if !appending || place(body, *p).loan.is_none() {
                        return None;
                    }
                }
                _ => return None,
            }
        }
    }
    if created != 1 {
        return None;
    }
    // Every loan on the list is used only as an appending argument.
    for (b, block) in body.blocks.iter().enumerate() {
        for inst in &block.instructions {
            for p in operand_places(body, inst) {
                if place(body, p).loan.is_some_and(|l| appends.contains(&l)) {
                    return None;
                }
            }
        }
        let _ = b;
    }
    // The result: moved to every return, nothing else.
    for l in &result {
        for r in &refs.by_local[l.0 as usize] {
            match r {
                Ref::Inert | Ref::Moved | Ref::Returned => {}
                Ref::Result { block } if *block == freeze => {}
                Ref::Assigned { value, .. } => match &body.rvalues[value.0 as usize] {
                    Rvalue::Use(op)
                        if op.usage == UseKind::Take && result.contains(&root(body, op.place)) => {}
                    _ => return None,
                },
                _ => return None,
            }
        }
    }
    for block in &body.blocks {
        if let Terminator::Return { value, .. } = &block.terminator
            && !(value.usage == UseKind::Take
                && whole(body, value.place)
                && result.contains(&root(body, value.place)))
        {
            return None;
        }
    }
    // No failure after the first append: may-appended dataflow over blocks.
    let appends_at = |b: usize| -> bool {
        match &body.blocks[b].terminator {
            Terminator::Call { arguments, .. } => arguments.iter().any(|a| {
                a.value.usage == UseKind::Mut && list.contains(&root(body, a.value.place))
            }),
            _ => false,
        }
    };
    let mut entered = vec![false; body.blocks.len()];
    let mut seen = vec![false; body.blocks.len()];
    let mut work = vec![(body.entry.0 as usize, false)];
    while let Some((b, appended)) = work.pop() {
        if seen[b] && (entered[b] || !appended) {
            continue;
        }
        seen[b] = true;
        entered[b] |= appended;
        let appended = entered[b];
        if let Some(l) = loops.get(&b) {
            // A copy loop is a total append; it continues at its exit.
            let into_list = list.contains(&root(body, l.dest));
            work.push((l.exit, appended || into_list));
            continue;
        }
        let block = &body.blocks[b];
        if appended
            && block
                .instructions
                .iter()
                .any(|i| instruction_fails(body, types, i))
        {
            return None;
        }
        let after = appended || appends_at(b);
        match &block.terminator {
            Terminator::Goto(n) => work.push((n.0 as usize, after)),
            Terminator::Branch { yes, no, .. } => {
                work.push((yes.0 as usize, after));
                work.push((no.0 as usize, after));
            }
            Terminator::Switch {
                arms, otherwise, ..
            } => {
                for (_, t) in arms {
                    work.push((t.0 as usize, after));
                }
                work.push((otherwise.0 as usize, after));
            }
            Terminator::Match { table, .. } => {
                for arm in &body.matches[table.0 as usize].arms {
                    if let Some(g) = arm.guard {
                        work.push((g.0 as usize, after));
                    }
                    work.push((arm.target.0 as usize, after));
                }
            }
            Terminator::Call {
                callee, returns, ..
            } => {
                if appended {
                    let total = match callee {
                        Callee::Definition { path, .. } => fuser.total_callee(program, path),
                        Callee::Indirect { .. } => false,
                    };
                    if !total {
                        return None;
                    }
                }
                if let Some(r) = returns {
                    work.push((r.next.0 as usize, after));
                }
            }
            Terminator::Return { cleanup, .. } => {
                if appended && cleanup_fails(body, types, *cleanup) {
                    return None;
                }
            }
            Terminator::Fail { .. } => {
                if appended {
                    return None;
                }
            }
            Terminator::Unreachable => {}
        }
    }
    // Guards that resume a match at a later arm continue it: their targets
    // were pushed with the match's state above.
    Some(Shape {
        list,
        freeze,
        result,
    })
}

/// The places an instruction reads or writes as operands.
fn operand_places(body: &Body, inst: &Instruction) -> Vec<PlaceId> {
    match &inst.kind {
        K::Assign { value, .. } => match &body.rvalues[value.0 as usize] {
            Rvalue::Use(op) => vec![op.place],
            Rvalue::Unary { value, .. } => vec![value.place],
            Rvalue::Binary { left, right, .. } => vec![left.place, right.place],
            Rvalue::Construct { fields, .. } => fields.iter().map(|f| f.value.place).collect(),
            Rvalue::Closure { captures, .. } => captures.iter().map(|c| c.place).collect(),
            _ => vec![],
        },
        K::Replace { value, .. } | K::Snapshot { value, .. } => vec![value.place],
        K::Check { condition, .. } => vec![condition.place],
        _ => vec![],
    }
}

/// A unit type row in `types`.
fn unit_type(types: &mut TypeStore) -> TypeId {
    let found = types
        .iter()
        .find(|(_, t)| matches!(&t.kind, TypeKind::Tuple(items) if items.is_empty()))
        .map(|(id, _)| id);
    found.unwrap_or_else(|| types.push(Type::unit()))
}

fn next_loan(body: &Body) -> LoanId {
    let mut max = 0;
    for b in &body.blocks {
        for i in &b.instructions {
            match &i.kind {
                K::BeginLoan { id, .. } | K::EndLoan { loan: id, .. } => max = max.max(id.0 + 1),
                _ => {}
            }
        }
    }
    for p in &body.places {
        if let Some(l) = p.loan {
            max = max.max(l.0 + 1);
        }
    }
    for n in &body.cleanup_nodes {
        if let CleanupAction::EndLoan { loan, .. } = &n.action {
            max = max.max(loan.0 + 1);
        }
    }
    LoanId(max)
}

/// The cleanup that ends `loan`, then runs `rest`.
fn with_loan_end(body: &mut Body, loan: LoanId, rest: CleanupId) -> CleanupId {
    let parent = body.cleanups[rest.0 as usize].head;
    let node = CleanupNodeId(body.cleanup_nodes.len() as u32);
    body.cleanup_nodes.push(tir::CleanupNode {
        action: CleanupAction::EndLoan {
            loan,
            if_active: false,
        },
        parent,
    });
    let id = CleanupId(body.cleanups.len() as u32);
    body.cleanups.push(tir::Cleanup { head: Some(node) });
    id
}

/// Replace the site's call by its appending form: a fresh mutable loan of the
/// list is passed last, the unit result retired, and the original result slot
/// receives the static empty value its copy then skips.
fn rewrite_site(def: &mut Definition, site: &Site, into: DefPath) {
    let unit = unit_type(&mut def.types);
    let body = &mut def.bodies[def.entry.0 as usize];
    let Terminator::Call {
        arguments,
        returns: Some(ret),
        on_failure,
        ..
    } = body.blocks[site.block].terminator.clone()
    else {
        unreachable!("a site ends in a returning call")
    };
    let span = body.blocks[site.block].span;
    let result = root(body, ret.destination);
    let string = matches!(
        def.types
            .get(body.locals[result.0 as usize].ty)
            .map(|t| &t.kind),
        Some(TypeKind::Primitive(Primitive::String))
    );
    let loan = next_loan(body);
    let template = body.places[site.consumer_place.0 as usize].clone();
    let list = PlaceId(body.places.len() as u32);
    body.places.push(Place {
        loan: Some(loan),
        ..template
    });
    let scope = body.locals[result.0 as usize].scope;
    let done = LocalId(body.locals.len() as u32);
    body.locals.push(Local {
        ty: unit,
        storage: Storage::Temporary,
        scope,
        span,
    });
    let done_place = PlaceId(body.places.len() as u32);
    body.places.push(Place {
        loan: None,
        binding: None,
        root: done,
        projection: Default::default(),
        ty: unit,
    });
    let failure = with_loan_end(body, loan, on_failure);
    let instruction = |kind, on_failure| Instruction {
        kind,
        span,
        on_failure,
    };
    let call = &mut body.blocks[site.block];
    call.instructions.push(instruction(
        K::BeginLoan {
            id: loan,
            place: site.dest,
            mutable: true,
            if_inactive: false,
        },
        failure,
    ));
    call.instructions
        .push(instruction(K::StorageLive(done), failure));
    let parameter = arguments.len() as u32;
    let mut arguments = arguments;
    arguments.push(CallArgument {
        parameter: ParamId(parameter),
        value: Operand {
            place: list,
            usage: UseKind::Mut,
        },
    });
    let empty = RvalueId(body.rvalues.len() as u32);
    body.rvalues.push(Rvalue::Constant(if string {
        Constant::String(String::new())
    } else {
        Constant::Bytes(Vec::new())
    }));
    let resume = tir::BlockId(body.blocks.len() as u32);
    body.blocks.push(tir::Block {
        instructions: vec![
            instruction(
                K::EndLoan {
                    loan,
                    if_active: false,
                },
                on_failure,
            ),
            instruction(
                K::StorageDead {
                    place: done_place,
                    if_live: false,
                },
                on_failure,
            ),
            instruction(
                K::Assign {
                    destination: ret.destination,
                    value: empty,
                },
                on_failure,
            ),
        ],
        terminator: Terminator::Goto(ret.next),
        span,
    });
    // The empty text's view is the static empty bytes: no bridge call.
    if let Some(view) = site.view
        && let Terminator::Call {
            returns: Some(r), ..
        } = body.blocks[view].terminator.clone()
    {
        let bytes = RvalueId(body.rvalues.len() as u32);
        body.rvalues
            .push(Rvalue::Constant(Constant::Bytes(Vec::new())));
        let viewed = tir::BlockId(body.blocks.len() as u32);
        let span = body.blocks[view].span;
        let on_failure = body.blocks[view]
            .instructions
            .last()
            .map_or(on_failure, |i| i.on_failure);
        body.blocks.push(tir::Block {
            instructions: vec![Instruction {
                kind: K::Assign {
                    destination: r.destination,
                    value: bytes,
                },
                span,
                on_failure,
            }],
            terminator: Terminator::Goto(r.next),
            span,
        });
        body.blocks[view].terminator = Terminator::Goto(viewed);
    }
    body.blocks[site.block].terminator = Terminator::Call {
        callee: Callee::Definition {
            path: into,
            arguments: Vec::new(),
        },
        arguments,
        returns: Some(CallReturn {
            destination: done_place,
            next: resume,
        }),
        on_failure: failure,
    };
}

/// `f$into`: `f`'s body with its list replaced by a trailing `mut`
/// parameter, its freeze removed and its result unit.
fn derive_body(def: &Definition, into: &DefPath, shape: &Shape) -> Definition {
    let mut def = def.clone();
    def.path = into.clone();
    let unit = unit_type(&mut def.types);
    // A declared `mut` parameter's list reaches the caller's environment.
    let local_ty = def.bodies[def.entry.0 as usize].locals[shape.list[0].0 as usize].ty;
    let mut row = def.types.get(local_ty).expect("the list's type").clone();
    row.modes = hirsel_types::Modes {
        relation: hirsel_types::Relation::new([hirsel_types::Reach::Environment]),
        ..hirsel_types::Modes::global()
    };
    let list_ty = def.types.push(row);
    let body = &mut def.bodies[def.entry.0 as usize];
    let parameter = LocalId(body.locals.len() as u32);
    let ordinal = body.parameters.len() as u32;
    body.locals.push(Local {
        ty: list_ty,
        storage: Storage::Parameter,
        scope: tir::ScopeId(0),
        span: body.locals[shape.list[0].0 as usize].span,
    });
    body.parameters.push(parameter);
    let binding = Origin {
        root: OriginRoot::Parameter(BinderRef::body(body.parameter_scope, ParamId(ordinal))),
        fields: Default::default(),
    };
    let in_list = |l: LocalId| shape.list.contains(&l);
    let in_result = |l: LocalId| shape.result.contains(&l);
    let list_places: HashSet<u32> = (0..body.places.len() as u32)
        .filter(|p| in_list(body.places[*p as usize].root))
        .collect();
    let result_places: HashSet<u32> = (0..body.places.len() as u32)
        .filter(|p| in_result(body.places[*p as usize].root))
        .collect();
    for p in &list_places {
        let p = &mut body.places[*p as usize];
        p.root = parameter;
        p.binding = Some(binding.clone());
        if p.projection.is_empty() {
            p.ty = list_ty;
        }
    }
    for p in &result_places {
        body.places[*p as usize].ty = unit;
    }
    for l in &shape.result {
        body.locals[l.0 as usize].ty = unit;
    }
    let listed = |p: &PlaceId| list_places.contains(&p.0);
    let resulted = |p: &PlaceId| result_places.contains(&p.0);
    // Instructions: the list's creation, moves, storage, origins and drops go;
    // its loans stay, now on the parameter.
    for block in &mut body.blocks {
        block.instructions.retain(|i| match &i.kind {
            K::StorageLive(l) => !in_list(*l),
            K::StorageDead { place, .. } | K::EndOrigin(place) => !listed(place),
            K::Drop { place, .. } => !listed(place) && !resulted(place),
            K::BindOrigin { source, place, .. } => !listed(source) && !listed(place),
            K::Assign { destination, .. } => !listed(destination),
            _ => true,
        });
    }
    // The freeze becomes the unit its result now is.
    let freeze = shape.freeze;
    let Terminator::Call {
        returns: Some(ret), ..
    } = body.blocks[freeze].terminator.clone()
    else {
        unreachable!("a builder freezes by a returning call")
    };
    let span = body.blocks[freeze].span;
    let value = RvalueId(body.rvalues.len() as u32);
    body.rvalues.push(Rvalue::Constant(Constant::Unit));
    let resume = tir::BlockId(body.blocks.len() as u32);
    let on_failure = body.blocks[ret.next.0 as usize]
        .instructions
        .first()
        .map_or(CleanupId(0), |i| i.on_failure);
    body.blocks.push(tir::Block {
        instructions: vec![Instruction {
            kind: K::Assign {
                destination: ret.destination,
                value,
            },
            span,
            on_failure,
        }],
        terminator: Terminator::Goto(ret.next),
        span,
    });
    body.blocks[freeze].terminator = Terminator::Goto(resume);
    body.result = unit;
    compact_rvalues(body);
    // Cleanups lose the list's and the result's drops; shared suffixes stay shared.
    let old = std::mem::take(&mut body.cleanup_nodes);
    let mut mapped: Vec<Option<Option<CleanupNodeId>>> = vec![None; old.len()];
    fn map(
        n: Option<CleanupNodeId>,
        old: &[tir::CleanupNode],
        mapped: &mut Vec<Option<Option<CleanupNodeId>>>,
        out: &mut Vec<tir::CleanupNode>,
        dropped: &dyn Fn(&CleanupAction) -> bool,
    ) -> Option<CleanupNodeId> {
        let n = n?;
        if let Some(m) = mapped[n.0 as usize] {
            return m;
        }
        let node = &old[n.0 as usize];
        let parent = map(node.parent, old, mapped, out, dropped);
        let m = if dropped(&node.action) {
            parent
        } else {
            out.push(tir::CleanupNode {
                action: node.action.clone(),
                parent,
            });
            Some(CleanupNodeId(out.len() as u32 - 1))
        };
        mapped[n.0 as usize] = Some(m);
        m
    }
    let dropped = |a: &CleanupAction| matches!(a, CleanupAction::Drop { place, .. } if listed(place) || resulted(place));
    let mut out = Vec::new();
    for c in 0..body.cleanups.len() {
        let head = body.cleanups[c].head;
        body.cleanups[c].head = map(head, &old, &mut mapped, &mut out, &dropped);
    }
    body.cleanup_nodes = out;
    def.contracts.clear();
    def
}

/// Drop rvalues no instruction names any more, renumbering the rest.
fn compact_rvalues(body: &mut Body) {
    let mut used = vec![false; body.rvalues.len()];
    for b in &body.blocks {
        for i in &b.instructions {
            if let K::Assign { value, .. } = &i.kind {
                used[value.0 as usize] = true;
            }
        }
    }
    let mut map = vec![None; body.rvalues.len()];
    let old = std::mem::take(&mut body.rvalues);
    for (i, r) in old.into_iter().enumerate() {
        if used[i] {
            map[i] = Some(RvalueId(body.rvalues.len() as u32));
            body.rvalues.push(r);
        }
    }
    for b in &mut body.blocks {
        for i in &mut b.instructions {
            if let K::Assign { value, .. } = &mut i.kind {
                *value = map[value.0 as usize].expect("a used rvalue");
            }
        }
    }
}

/// A copy of `from`'s row `id` (a primitive, tuple or builtin of them) in `to`.
fn copy_type(from: &TypeStore, id: TypeId, to: &mut TypeStore) -> TypeId {
    let mut row = from.get(id).expect("a type row").clone();
    row.kind = match row.kind {
        TypeKind::Tuple(items) => TypeKind::Tuple(
            items
                .iter()
                .map(|t| copy_type(from, *t, to))
                .collect::<Vec<_>>()
                .into(),
        ),
        TypeKind::Builtin { kind, arguments } => TypeKind::Builtin {
            kind,
            arguments: arguments
                .iter()
                .map(|a| match a {
                    GenericArgument::Type(t) => GenericArgument::Type(copy_type(from, *t, to)),
                    size => size.clone(),
                })
                .collect::<Vec<_>>()
                .into(),
        },
        kind => kind,
    };
    to.push(row)
}

/// `f`'s signature with a trailing `mut out: List[u8]` and a unit result,
/// whose types are the derived body's own parameter and result rows.
fn derive_signature(signature: &Signature, into: &DefPath, derived: &Definition) -> Signature {
    let mut s = signature.clone();
    s.definition = into.clone();
    let SignatureKind::Function(f) = &mut s.kind else {
        unreachable!("a builder is a function")
    };
    let body = &derived.bodies[derived.entry.0 as usize];
    let parameter = *body.parameters.last().expect("the appended parameter");
    let list = copy_type(
        &derived.types,
        body.locals[parameter.0 as usize].ty,
        &mut s.types,
    );
    let unit = copy_type(&derived.types, body.result, &mut s.types);
    let mut parameters: Vec<Parameter> = f.parameters.iter().cloned().collect();
    parameters.push(Parameter {
        name: Some("out".into()),
        label: None,
        passing: PassingMode::Mut,
        ty: list,
    });
    f.parameters = parameters.into();
    f.result = unit;
    f.contracts = Vec::new().into();
    s.fingerprint = ContentHash::of_parts(
        "hirsel-rc.append-into.v1",
        [signature.fingerprint.0.as_slice()],
    );
    s
}
