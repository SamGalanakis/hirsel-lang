//! A transient bytes.slice result used only by synchronous bytes.append can
//! borrow the stable String owner's existing loan instead of acquiring a credit.
use super::*;
use hirsel_types::{Primitive, TypeKind};

#[derive(Clone, Copy)]
pub(super) struct Slice {
    pub source: tir::PlaceId,
    pub loan: tir::LoanId,
}

#[derive(Default)]
pub(super) struct Slices {
    pub sources: HashMap<tir::BlockId, Slice>,
    pub locals: Vec<bool>,
}

pub(super) fn discover(
    body: &tir::Body,
    types: &TypeInterner,
    aliases: &[Option<tir::PlaceId>],
) -> Slices {
    use tir::InstructionKind as K;
    let n = body.locals.len();
    let root = |p: tir::PlaceId| body.places[p.0 as usize].root.0 as usize;
    let whole = |p: tir::PlaceId| body.places[p.0 as usize].projection.is_empty();
    let bytes = |l: usize| {
        matches!(
            types.get(body.locals[l].ty).map(|t| &t.kind),
            Some(TypeKind::Primitive(Primitive::Bytes))
        )
    };
    let mut parent: Vec<usize> = (0..n).collect();
    fn find(parent: &mut [usize], i: usize) -> usize {
        if parent[i] != i {
            parent[i] = find(parent, parent[i]);
        }
        parent[i]
    }
    for i in body.blocks.iter().flat_map(|b| &b.instructions) {
        if let K::Assign { destination, value } = i.kind
            && let tir::Rvalue::Use(op) = &body.rvalues[value.0 as usize]
            && op.usage == UseKind::Take
            && whole(destination)
            && whole(op.place)
            && bytes(root(destination))
            && bytes(root(op.place))
        {
            let a = find(&mut parent, root(destination));
            let b = find(&mut parent, root(op.place));
            parent[a] = b;
        }
    }
    for l in 0..n {
        find(&mut parent, l);
    }
    let mut invalid = vec![false; n];
    let mut producers: Vec<Vec<(tir::BlockId, tir::PlaceId)>> = vec![Vec::new(); n];
    let mut consumed: Vec<Vec<tir::LoanId>> = vec![Vec::new(); n];
    let mut defined = vec![0; n];
    for (b, block) in body.blocks.iter().enumerate() {
        for i in &block.instructions {
            match &i.kind {
                K::Assign { destination, value } => {
                    let d = root(*destination);
                    defined[d] += 1;
                    if !(matches!(&body.rvalues[value.0 as usize], tir::Rvalue::Use(op)
                        if op.usage == UseKind::Take && whole(*destination) && whole(op.place)
                            && parent[d] == parent[root(op.place)]))
                    {
                        invalid[parent[d]] = true;
                        for op in rvalue_operands(&body.rvalues[value.0 as usize]) {
                            invalid[parent[root(op.place)]] = true;
                        }
                    }
                }
                K::BeginLoan { place, mutable, .. } => {
                    if *mutable || !whole(*place) {
                        invalid[parent[root(*place)]] = true;
                    }
                }
                K::Replace {
                    destination,
                    value,
                    displaced,
                    ..
                } => {
                    for p in [*destination, value.place, *displaced] {
                        invalid[parent[root(p)]] = true;
                    }
                }
                K::Snapshot { destination, value } => {
                    for p in [*destination, value.place] {
                        invalid[parent[root(p)]] = true;
                    }
                }
                K::Check { condition, .. } => invalid[parent[root(condition.place)]] = true,
                _ => {}
            }
        }
        match &block.terminator {
            tir::Terminator::Call {
                callee,
                arguments,
                returns,
                ..
            } => {
                if let Some(ret) = returns {
                    let d = root(ret.destination);
                    defined[d] += 1;
                    if matches!(callee, tir::Callee::Definition { path, .. } if path.is_core("bytes.slice"))
                        && whole(ret.destination)
                        && let Some(source) = arguments.iter().find(|a| a.parameter.0 == 0)
                        && source.value.usage == UseKind::Borrow
                    {
                        producers[parent[d]].push((tir::BlockId(b as u32), source.value.place));
                    } else {
                        invalid[parent[d]] = true;
                    }
                }
                let append = matches!(callee, tir::Callee::Definition { path, .. } if path.is_core("bytes.append"))
                    && arguments.len() == 2
                    && arguments
                        .iter()
                        .any(|a| a.parameter.0 == 0 && a.value.usage == UseKind::Mut);
                for a in arguments {
                    let group = parent[root(a.value.place)];
                    if append
                        && a.parameter.0 == 1
                        && a.value.usage == UseKind::Borrow
                        && whole(a.value.place)
                    {
                        if let Some(loan) = body.places[a.value.place.0 as usize].loan {
                            consumed[group].push(loan);
                        } else {
                            invalid[group] = true;
                        }
                    } else {
                        invalid[group] = true;
                    }
                }
                if let tir::Callee::Indirect { value, .. } = callee {
                    invalid[parent[root(value.place)]] = true;
                }
            }
            tir::Terminator::Return { value, .. }
            | tir::Terminator::Branch {
                condition: value, ..
            }
            | tir::Terminator::Switch { value, .. } => invalid[parent[root(value.place)]] = true,
            _ => {}
        }
    }
    for table in &body.matches {
        invalid[parent[root(table.scrutinee)]] = true;
        for pattern in &table.patterns {
            invalid[parent[root(pattern.place)]] = true;
        }
    }
    for l in 0..n {
        if !bytes(l) || body.locals[l].storage != tir::Storage::Temporary || defined[l] != 1 {
            invalid[parent[l]] = true;
        }
    }
    let mut out = Slices {
        locals: vec![false; n],
        ..Default::default()
    };
    for group in 0..n {
        let &[(block, mut source)] = producers[group].as_slice() else {
            continue;
        };
        let &[loan] = consumed[group].as_slice() else {
            continue;
        };
        if invalid[group] {
            continue;
        }
        // byte_views already proved the as_bytes owner's stability and nesting.
        // Follow its aliases to the held text's loan. A plain Bytes producer or
        // mutable source retains the original owning call.
        let data_group = parent[root(source)];
        let mut held = None;
        let mut seen = BTreeSet::new();
        while let Some(base) = aliases[root(source)] {
            if !seen.insert(root(source)) {
                held = None;
                break;
            }
            source = base;
            held = Some(base);
        }
        let Some(held) = held else { continue };
        let held_place = &body.places[held.0 as usize];
        if !held_place.projection.is_empty()
            || !matches!(
                types.get(held_place.ty).map(|t| &t.kind),
                Some(TypeKind::Primitive(Primitive::String))
            )
        {
            continue;
        }
        // Resolve the stable owner through the original as_bytes operand.
        // Its call-only loan can already have ended: the slice instead uses
        // the consumer's loan, begun at slice creation and ended after append.
        let source = body.blocks.iter().find_map(|b| {
            let tir::Terminator::Call {
                callee: tir::Callee::Definition { path, .. },
                arguments,
                returns: Some(ret),
                ..
            } = &b.terminator
            else {
                return None;
            };
            if !path.is_core("string.as_bytes") || parent[root(ret.destination)] != data_group {
                return None;
            }
            let input = arguments.iter().find(|a| a.parameter.0 == 0)?;
            (input.value.usage == UseKind::Borrow
                && body.places[input.value.place.0 as usize].loan.is_some())
            .then_some(input.value.place)
        });
        let Some(source) = source else { continue };
        out.sources.insert(block, Slice { source, loan });
        for l in 0..n {
            if parent[l] == group {
                out.locals[l] = true;
            }
        }
    }
    out
}

impl Importer<'_> {
    pub(super) fn slice_view(
        &mut self,
        arguments: &[tir::CallArgument],
        ret: tir::CallReturn,
        slice: Slice,
    ) -> R<()> {
        let source = self.place(slice.source)?;
        let address = self.address(&source)?;
        let loan = self.loan(slice.loan)?;
        let active = self.state.loans[loan.index()];
        self.push(Inst::BeginLoan {
            loan,
            address,
            mutable: false,
            if_inactive: active == Tri::Maybe,
        });
        self.state.loans[loan.index()] = Tri::Yes;
        // TIR begins this loan at append's entry. Until then, an early
        // slice failure must also end it before releasing the source owner.
        self.failure.before.insert(0, Action::EndLoan { loan });
        let value = self.operand_at(UseKind::Borrow, &source, address, Some(slice.loan))?;
        let argument = |n| {
            arguments
                .iter()
                .find(|a| a.parameter.0 == n)
                .map(|a| &a.value)
        };
        let start =
            self.operand(argument(1).ok_or_else(|| PassError::Malformed("slice start".into()))?)?;
        let end =
            self.operand(argument(2).ok_or_else(|| PassError::Malformed("slice end".into()))?)?;
        let Holding::Borrowed(loan) = self.out.values[value.index()].holding else {
            return malformed("a slice view without a held source loan");
        };
        let layout = self.place_layout(ret.destination)?;
        let ty = self.intern(layout);
        let result = self.value(ty, Holding::Borrowed(loan));
        let failure = self.fail(Some("SEM-3".into()))?;
        self.push(Inst::SliceView {
            result,
            value,
            start,
            end,
            failure,
        });
        let l = self.body.places[ret.destination.0 as usize].root.0 as usize;
        self.views[l] = Some(result);
        self.state.views[l] = Tri::Yes;
        self.failure.before.remove(0);
        let edge = self.enter(ret.next);
        self.terminate(Terminator::Jump(edge));
        Ok(())
    }
}
