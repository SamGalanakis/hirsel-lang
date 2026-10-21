//! Each executed body, lowered once per run into dispatch form: one flat op array
//! with block offsets, whole-local operands resolved to slots, scalar constants
//! decoded against their destination type, scope ends expanded to the slots they
//! clear, and cleanup lists of whole-local drops expanded to their slots. The
//! scheduler steps the top frame through these ops without consulting the TIR.
//! An op the dispatch form does not specialize stays `General` and executes from
//! the TIR exactly as before, so every resolved op keeps the general path's
//! semantics, steps, sites and trace.
use crate::Value;
use hirsel_span::Span;
use hirsel_syntax::{BinaryOp, UnaryOp};
use hirsel_tir::*;
use hirsel_types::{Multiplicity, TypeInterner, UseKind};

/// A whole local read as an operand.
#[derive(Clone, Copy)]
pub(crate) struct Local {
    pub slot: u32,
    pub usage: UseKind,
    /// The local's type is used at most once, so a copy of it is invalid TIR.
    pub once: bool,
}

pub(crate) enum CleanupStep {
    Drop { slot: u32, if_live: bool },
    EndLoan,
}

pub(crate) enum OpKind {
    /// Loans, origin ends, field bindings and origin bindings of whole locals:
    /// one step and no effect.
    Nop,
    Live(u32),
    Dead {
        slot: u32,
        if_live: bool,
    },
    /// The slots of every local whose scope is the ended scope or inside it.
    EndScope(Box<[u32]>),
    Constant {
        slot: u32,
        value: Value,
    },
    Use {
        slot: u32,
        value: Local,
    },
    Unary {
        op: UnaryOp,
        slot: u32,
        value: Local,
    },
    Binary {
        op: BinaryOp,
        slot: u32,
        left: Local,
        right: Local,
    },
    Replace {
        slot: u32,
        value: Local,
        displaced: u32,
        if_live: bool,
    },
    Check(Local),
    /// A cleanup list of whole-local drops and loan ends, in list order.
    Cleanup {
        id: CleanupId,
        steps: Box<[CleanupStep]>,
    },
    Goto(BlockId),
    Branch {
        condition: Local,
        yes: BlockId,
        no: BlockId,
    },
    /// Executed from the TIR instruction or terminator.
    General,
}

pub(crate) struct Op {
    pub kind: OpKind,
    pub site: Span,
    /// A direct call's site.
    pub call: Option<std::rc::Rc<CallSite>>,
}

/// A direct call's written callee and, when its generic arguments close without
/// the calling frame, the target it resolves to for the run.
pub(crate) struct CallSite {
    pub callee: hirsel_syntax::DefPath,
    /// No generic argument is a size parameter of the calling frame.
    pub closed: bool,
    pub target: std::cell::OnceCell<std::rc::Rc<Target>>,
}

/// A direct callee as a call resolves it.
pub(crate) struct Target {
    /// The callee after CT-5 instance selection.
    pub path: hirsel_syntax::DefPath,
    /// Its closed generic arguments.
    pub actuals: hirsel_types::Actuals,
    /// The self-tail site the call restarts, when it calls its own definition.
    pub tail: Option<TailSite>,
    pub forget: bool,
    /// A core@1 path, which the evaluator's arena and freeze rules may name.
    pub core: bool,
    /// The TIR entry, once an operation layer has declined the call.
    pub entry: std::cell::OnceCell<Entry>,
}

pub(crate) struct Entry {
    pub definition: DefinitionRef,
    pub body: BodyId,
    pub sizes: crate::value::SizeBindings,
    pub type_arguments: crate::type_arguments::Bindings,
    pub code: u32,
}

pub(crate) struct Code {
    /// The op index of each block's first instruction; its terminator follows
    /// its instructions.
    starts: Box<[u32]>,
    ops: Box<[Op]>,
}

impl Code {
    pub fn op(&self, block: BlockId, instruction: usize) -> &Op {
        &self.ops[self.starts[block.0 as usize] as usize + instruction]
    }

    pub fn new(body: &Body, types: &TypeInterner) -> Self {
        let lower = Lowering { body, types };
        let mut starts = Vec::with_capacity(body.blocks.len());
        let mut ops = Vec::new();
        for block in &body.blocks {
            starts.push(ops.len() as u32);
            for instruction in &block.instructions {
                ops.push(Op {
                    kind: lower
                        .instruction(&instruction.kind)
                        .unwrap_or(OpKind::General),
                    site: instruction.span,
                    call: None,
                });
            }
            ops.push(Op {
                kind: lower
                    .terminator(&block.terminator)
                    .unwrap_or(OpKind::General),
                site: block.span,
                call: match &block.terminator {
                    Terminator::Call {
                        callee: Callee::Definition { path, arguments },
                        ..
                    } => Some(std::rc::Rc::new(CallSite {
                        callee: path.clone(),
                        closed: !arguments.iter().any(|argument| {
                            matches!(
                                argument,
                                hirsel_types::GenericArgument::Size(hirsel_types::Size::Parameter(
                                    _
                                ))
                            )
                        }),
                        target: Default::default(),
                    })),
                    _ => None,
                },
            });
        }
        Self {
            starts: starts.into(),
            ops: ops.into(),
        }
    }
}

struct Lowering<'a> {
    body: &'a Body,
    types: &'a TypeInterner,
}

impl Lowering<'_> {
    /// The slot of a place that names a whole local.
    fn slot(&self, place: PlaceId) -> Option<u32> {
        let place = self.body.places.get(place.0 as usize)?;
        (place.projection.is_empty() && (place.root.0 as usize) < self.body.locals.len())
            .then_some(place.root.0)
    }

    fn local(&self, operand: &Operand) -> Option<Local> {
        let slot = self.slot(operand.place)?;
        let ty = self.body.places[operand.place.0 as usize].ty;
        Some(Local {
            slot,
            usage: operand.usage,
            once: self.types.get(ty)?.modes.multiplicity == Multiplicity::Once,
        })
    }

    fn instruction(&self, kind: &InstructionKind) -> Option<OpKind> {
        Some(match *kind {
            InstructionKind::BeginLoan { .. }
            | InstructionKind::EndLoan { .. }
            | InstructionKind::BindFields { .. }
            | InstructionKind::EndOrigin(_) => OpKind::Nop,
            // A whole local always resolves, so its binding records no failure site.
            InstructionKind::BindOrigin { source, place, .. } => {
                self.slot(source)?;
                self.slot(place)?;
                OpKind::Nop
            }
            InstructionKind::StorageLive(local) => {
                ((local.0 as usize) < self.body.locals.len()).then_some(())?;
                OpKind::Live(local.0)
            }
            InstructionKind::StorageDead { place, if_live } => OpKind::Dead {
                slot: self.slot(place)?,
                if_live,
            },
            InstructionKind::EndScope(scope) => OpKind::EndScope(self.scope_slots(scope)?),
            InstructionKind::Assign { destination, value } => {
                let slot = self.slot(destination)?;
                match self.body.rvalues.get(value.0 as usize)? {
                    Rvalue::Constant(constant) => {
                        let ty = self.body.places[destination.0 as usize].ty;
                        let value =
                            crate::scalar::constant(constant, &self.types.get(ty)?.kind).ok()?;
                        OpKind::Constant { slot, value }
                    }
                    Rvalue::Use(operand) => OpKind::Use {
                        slot,
                        value: self.local(operand)?,
                    },
                    Rvalue::Unary { op, value } if *op != UnaryOp::Unique => OpKind::Unary {
                        op: *op,
                        slot,
                        value: self.local(value)?,
                    },
                    Rvalue::Binary { op, left, right } => OpKind::Binary {
                        op: *op,
                        slot,
                        left: self.local(left)?,
                        right: self.local(right)?,
                    },
                    _ => return None,
                }
            }
            InstructionKind::Replace {
                destination,
                ref value,
                displaced,
                if_live,
            } => OpKind::Replace {
                slot: self.slot(destination)?,
                value: self.local(value)?,
                displaced: self.slot(displaced)?,
                if_live,
            },
            InstructionKind::Check { ref condition, .. } => OpKind::Check(self.local(condition)?),
            InstructionKind::RunCleanup(id) => {
                let mut steps = vec![];
                for action in self.body.cleanup_actions(id) {
                    steps.push(match *action {
                        CleanupAction::Drop { place, if_live } => CleanupStep::Drop {
                            slot: self.slot(place)?,
                            if_live,
                        },
                        CleanupAction::EndLoan { .. } => CleanupStep::EndLoan,
                        _ => return None,
                    });
                }
                OpKind::Cleanup {
                    id,
                    steps: steps.into(),
                }
            }
            _ => return None,
        })
    }

    fn terminator(&self, terminator: &Terminator) -> Option<OpKind> {
        Some(match *terminator {
            Terminator::Goto(block) => OpKind::Goto(self.block(block)?),
            Terminator::Branch {
                ref condition,
                yes,
                no,
            } => OpKind::Branch {
                condition: self.local(condition)?,
                yes: self.block(yes)?,
                no: self.block(no)?,
            },
            _ => return None,
        })
    }

    fn block(&self, block: BlockId) -> Option<BlockId> {
        ((block.0 as usize) < self.body.blocks.len()).then_some(block)
    }

    /// EndScope clears each local whose scope chain reaches `scope`, in local order.
    fn scope_slots(&self, scope: ScopeId) -> Option<Box<[u32]>> {
        let mut slots = vec![];
        for (index, local) in self.body.locals.iter().enumerate() {
            let mut current = Some(local.scope);
            while let Some(id) = current {
                if id == scope {
                    slots.push(index as u32);
                    break;
                }
                current = self.body.scopes.get(id.0 as usize)?.parent;
            }
        }
        Some(slots.into())
    }
}

/// The dispatch forms of one run's bodies. A run's definitions are immutable, so a
/// body is lowered at most once per run, at the first step of its first frame.
#[derive(Default)]
pub(crate) struct Codes {
    /// Each definition's bodies, by body ordinal.
    ids: hirsel_syntax::DefMap<Vec<Option<u32>>>,
    codes: Vec<Code>,
}

impl Codes {
    pub fn id(&mut self, types: &TypeInterner, definition: &Definition, body: BodyId) -> u32 {
        let index = body.0 as usize;
        if let Some(&Some(id)) = self
            .ids
            .get(&definition.path)
            .and_then(|ids| ids.get(index))
        {
            return id;
        }
        let id = self.codes.len() as u32;
        self.codes.push(Code::new(&definition.bodies[index], types));
        let ids = self.ids.entry(definition.path).or_default();
        if ids.len() <= index {
            ids.resize(definition.bodies.len().max(index + 1), None);
        }
        ids[index] = Some(id);
        id
    }
    pub fn get(&self, id: u32) -> &Code {
        &self.codes[id as usize]
    }
}
