//! Obligation identity, computed from TIR alone.
use crate::wire::Cx;
use hirsel_codec::{DecodeError, Reader, Snapshot, Writer, snapshot_enum, snapshot_struct};
use hirsel_diag::RuleId;
use hirsel_span::Span;
use hirsel_syntax::{BinaryOp, ContentHash, ContractKind, DefPath, UnaryOp};
use hirsel_tir::{
    BlockId, Body, BodyId, Callee, Definition, Instruction, InstructionKind, LocalId, Operand,
    PlaceId, Projection, Rvalue, Terminator,
};
use hirsel_types::{Contract, Primitive, Signature, SignatureKind, TypeKind};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fmt;

/// PS-2. Closed per format version; later clause forms are new kinds.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub enum ObligationKind {
    Check,
    FailureEdge,
    RequiresAtCall,
    Ensures,
    Requires,
}
impl ObligationKind {
    pub const ALL: [Self; 5] = [
        Self::Check,
        Self::FailureEdge,
        Self::RequiresAtCall,
        Self::Ensures,
        Self::Requires,
    ];
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Check => "check",
            Self::FailureEdge => "failure-edge",
            Self::RequiresAtCall => "requires-at-call",
            Self::Ensures => "ensures",
            Self::Requires => "requires",
        }
    }
}
impl Snapshot<Cx> for ObligationKind {
    fn write(&self, w: &mut Writer<Cx>) {
        w.text(self.as_str());
    }
    fn read(r: &mut Reader<'_, Cx>) -> Result<Self, DecodeError> {
        let text = r.text()?;
        Self::ALL
            .into_iter()
            .find(|kind| kind.as_str() == text)
            .ok_or_else(|| r.error(format!("unknown obligation kind {text:?}")))
    }
}

/// PS-1. Positional, never containing arena indices, spans or hashes.
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct ObligationId {
    pub definition: DefPath,
    pub rule: RuleId,
    pub kind: ObligationKind,
    pub ordinal: u32,
}
snapshot_struct!(ObligationId {
    definition,
    rule,
    kind,
    ordinal
} in Cx);
impl fmt::Display for ObligationId {
    /// `pkg@ver/mod/name#RULE/kind/ordinal`
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}#{}/{}/{}",
            self.definition,
            self.rule,
            self.kind.as_str(),
            self.ordinal
        )
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SitePosition {
    Instruction(u32),
    Terminator,
}
snapshot_enum!(SitePosition { Instruction(i), Terminator } in Cx);

/// One TIR site of an obligation; `span` is the instruction's, or the block's
/// for a terminator.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Site {
    pub body: BodyId,
    pub block: BlockId,
    pub position: SitePosition,
    pub span: Span,
}
snapshot_struct!(Site {
    body,
    block,
    position,
    span
} in Cx);

/// Per-kind detail: the called definition (None for an indirect call) and the
/// callee clause, or the definition's own clause with its code hash.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Detail {
    Site,
    Call {
        callee: Option<DefPath>,
        clause: u32,
    },
    Clause {
        clause: u32,
        code_hash: ContentHash,
    },
}
snapshot_enum!(Detail {
    Site,
    Call { callee, clause },
    Clause { clause, code_hash }
} in Cx);

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Obligation {
    pub id: ObligationId,
    /// PS-6.
    pub checksum: ContentHash,
    pub sites: Vec<Site>,
    pub detail: Detail,
}
snapshot_struct!(Obligation {
    id,
    checksum,
    sites,
    detail
} in Cx);

/// Obligations without checksums, and the definitions called by name.
pub(crate) struct Table {
    pub obligations: Vec<(ObligationId, Vec<Site>, Detail)>,
    pub callees: BTreeSet<DefPath>,
}

fn function(signature: Option<&Signature>) -> Option<&hirsel_types::FunctionType> {
    match &signature?.kind {
        SignatureKind::Function(function) => Some(function),
        _ => None,
    }
}

fn clauses(contracts: &[Contract], kind: ContractKind) -> Vec<&Contract> {
    let mut clauses: Vec<_> = contracts.iter().filter(|c| c.kind == kind).collect();
    clauses.sort_by_key(|c| c.ordinal);
    clauses
}

/// Assigns ordinals per (rule, kind) in PS-3 order.
struct Builder<'d> {
    definition: &'d DefPath,
    counters: BTreeMap<(RuleId, ObligationKind), u32>,
    obligations: Vec<(ObligationId, Vec<Site>, Detail)>,
    /// MEM-13/14 obligations are per projected place (body, root and
    /// projection; loans and bindings give one access several PlaceIds):
    /// every instruction or terminator naming it is a site of its one
    /// obligation.
    places: HashMap<(usize, LocalId, Vec<Projection>, RuleId), usize>,
}
impl Builder<'_> {
    fn push(&mut self, rule: RuleId, kind: ObligationKind, site: Site, detail: Detail) -> usize {
        let counter = self.counters.entry((rule.clone(), kind)).or_default();
        let id = ObligationId {
            definition: self.definition.clone(),
            rule,
            kind,
            ordinal: *counter,
        };
        *counter += 1;
        self.obligations.push((id, vec![site], detail));
        self.obligations.len() - 1
    }
    /// MEM-14 for a place with an Index projection, MEM-13 for one with a
    /// VariantField projection.
    fn place(&mut self, body: &Body, b: usize, place: PlaceId, site: &Site) {
        let Some(row) = body.places.get(place.0 as usize) else {
            return;
        };
        let projection = row.projection.to_vec();
        let index = projection.iter().any(|p| matches!(p, Projection::Index(_)));
        let variant = projection
            .iter()
            .any(|p| matches!(p, Projection::VariantField { .. }));
        for (code, present) in [
            (const { RuleId::of("MEM-14") }, index),
            (const { RuleId::of("MEM-13") }, variant),
        ] {
            if !present {
                continue;
            }
            let key = (b, row.root, projection.clone(), code.clone());
            match self.places.get(&key) {
                Some(index) => self.obligations[*index].1.push(site.clone()),
                None => {
                    let index = self.push(
                        code,
                        ObligationKind::FailureEdge,
                        site.clone(),
                        Detail::Site,
                    );
                    self.places.insert(key, index);
                }
            }
        }
    }
}

/// PS-2/PS-3 over one definition. `signature` returns a referenced
/// definition's public signature.
pub(crate) fn table<'a>(
    definition: &Definition,
    signature: impl Fn(&DefPath) -> Option<&'a Signature>,
) -> Table {
    let fact = const { RuleId::of("FACT-1") };
    let owner = function(signature(&definition.path));
    let owner_contracts: &[Contract] = owner.map_or(&[], |f| &f.contracts);
    let contract_checks = contract_checks(definition, owner_contracts);
    let mut builder = Builder {
        definition: &definition.path,
        counters: BTreeMap::new(),
        obligations: vec![],
        places: HashMap::new(),
    };
    let mut clause_sites: BTreeMap<u32, Vec<Site>> = BTreeMap::new();
    let mut callees = BTreeSet::new();
    for (b, body) in definition.bodies.iter().enumerate() {
        for (k, block) in body.blocks.iter().enumerate() {
            for (i, instruction) in block.instructions.iter().enumerate() {
                let site = Site {
                    body: BodyId(b as u32),
                    block: BlockId(k as u32),
                    position: SitePosition::Instruction(i as u32),
                    span: instruction.span,
                };
                match &instruction.kind {
                    InstructionKind::Check { rule, .. } => match contract_checks.get(&(b, k, i)) {
                        Some(clause) => clause_sites.entry(*clause).or_default().push(site.clone()),
                        None => {
                            builder.push(
                                rule.clone(),
                                ObligationKind::Check,
                                site.clone(),
                                Detail::Site,
                            );
                        }
                    },
                    InstructionKind::Assign { value, .. }
                        if body
                            .rvalues
                            .get(value.0 as usize)
                            .is_some_and(|rvalue| checked_arithmetic(definition, body, rvalue)) =>
                    {
                        builder.push(
                            const { RuleId::of("SEM-1") },
                            ObligationKind::FailureEdge,
                            site.clone(),
                            Detail::Site,
                        );
                    }
                    _ => {}
                }
                for place in instruction_places(body, instruction) {
                    builder.place(body, b, place, &site);
                }
            }
            let site = Site {
                body: BodyId(b as u32),
                block: BlockId(k as u32),
                position: SitePosition::Terminator,
                span: block.span,
            };
            match &block.terminator {
                Terminator::Fail { rule, .. } => {
                    builder.push(
                        rule.clone(),
                        ObligationKind::FailureEdge,
                        site.clone(),
                        Detail::Site,
                    );
                }
                Terminator::Call { callee, .. } => {
                    let (path, contracts): (Option<&DefPath>, &[Contract]) = match callee {
                        Callee::Definition { path, .. } => {
                            callees.insert(path.clone());
                            (
                                Some(path),
                                function(signature(path)).map_or(&[], |f| &f.contracts),
                            )
                        }
                        Callee::Indirect { value, .. } => {
                            let ty = body.places.get(value.place.0 as usize).map(|p| p.ty);
                            match ty
                                .and_then(|ty| definition.types.get(ty))
                                .map(|row| &row.kind)
                            {
                                Some(TypeKind::Function(f)) => (None, &f.contracts),
                                _ => (None, &[]),
                            }
                        }
                    };
                    for clause in clauses(contracts, ContractKind::Requires) {
                        builder.push(
                            fact.clone(),
                            ObligationKind::RequiresAtCall,
                            site.clone(),
                            Detail::Call {
                                callee: path.cloned(),
                                clause: clause.ordinal,
                            },
                        );
                    }
                }
                _ => {}
            }
            for place in terminator_places(&block.terminator) {
                builder.place(body, b, place, &site);
            }
        }
    }
    let clause_obligations = |kind: ContractKind, obligation: ObligationKind| {
        clauses(owner_contracts, kind)
            .into_iter()
            .enumerate()
            .map(|(ordinal, clause)| {
                (
                    ObligationId {
                        definition: definition.path.clone(),
                        rule: fact.clone(),
                        kind: obligation,
                        ordinal: ordinal as u32,
                    },
                    clause_sites
                        .get(&clause.ordinal)
                        .cloned()
                        .unwrap_or_default(),
                    Detail::Clause {
                        clause: clause.ordinal,
                        code_hash: clause.code_hash,
                    },
                )
            })
            .collect::<Vec<_>>()
    };
    let mut obligations = clause_obligations(ContractKind::Requires, ObligationKind::Requires);
    obligations.extend(builder.obligations);
    obligations.extend(clause_obligations(
        ContractKind::Ensures,
        ObligationKind::Ensures,
    ));
    Table {
        obligations,
        callees,
    }
}

/// PS-2 contract sites: a FACT-1 Check in the entry body whose condition is the
/// result of an indirect call of a closure over a body in
/// `Definition.contracts`, mapped to that clause's ordinal when the owner
/// signature declares it.
fn contract_checks(
    definition: &Definition,
    contracts: &[Contract],
) -> HashMap<(usize, usize, usize), u32> {
    let mut checks = HashMap::new();
    let entry = definition.entry.0 as usize;
    let Some(body) = definition.bodies.get(entry) else {
        return checks;
    };
    let root = |place: PlaceId| body.places.get(place.0 as usize).map(|p| p.root);
    let clause_of_body: HashMap<BodyId, u32> = definition
        .contracts
        .iter()
        .filter(|(ordinal, _)| contracts.iter().any(|c| c.ordinal == *ordinal))
        .map(|(ordinal, body)| (*body, *ordinal))
        .collect();
    let mut closures: HashMap<LocalId, BodyId> = HashMap::new();
    let mut results: HashMap<LocalId, BodyId> = HashMap::new();
    for block in &body.blocks {
        for instruction in &block.instructions {
            if let InstructionKind::Assign { destination, value } = &instruction.kind
                && let Some(Rvalue::Closure { body, .. }) = body.rvalues.get(value.0 as usize)
                && let Some(root) = root(*destination)
            {
                closures.insert(root, *body);
            }
        }
    }
    for block in &body.blocks {
        if let Terminator::Call {
            callee: Callee::Indirect { value, .. },
            returns: Some(returns),
            ..
        } = &block.terminator
            && let Some(closure) = root(value.place).and_then(|r| closures.get(&r))
            && let Some(result) = root(returns.destination)
        {
            results.insert(result, *closure);
        }
    }
    let fact = const { RuleId::of("FACT-1") };
    for (k, block) in body.blocks.iter().enumerate() {
        for (i, instruction) in block.instructions.iter().enumerate() {
            if let InstructionKind::Check { condition, rule } = &instruction.kind
                && *rule == fact
                && let Some(closure) = root(condition.place).and_then(|r| results.get(&r))
                && let Some(clause) = clause_of_body.get(closure)
            {
                checks.insert((entry, k, i), *clause);
            }
        }
    }
    checks
}

fn integer(definition: &Definition, body: &Body, operand: &Operand) -> bool {
    let Some(place) = body.places.get(operand.place.0 as usize) else {
        return false;
    };
    matches!(
        definition.types.get(place.ty).map(|row| &row.kind),
        Some(TypeKind::Primitive(
            Primitive::U8
                | Primitive::U16
                | Primitive::U32
                | Primitive::U64
                | Primitive::U128
                | Primitive::I8
                | Primitive::I16
                | Primitive::I32
                | Primitive::I64
                | Primitive::I128
        ))
    )
}

/// SEM-1 checked integer operations; comparisons, bitwise operations and
/// floats cannot fail.
fn checked_arithmetic(definition: &Definition, body: &Body, rvalue: &Rvalue) -> bool {
    match rvalue {
        Rvalue::Binary { op, left, .. } => {
            matches!(
                op,
                BinaryOp::Add
                    | BinaryOp::Sub
                    | BinaryOp::Mul
                    | BinaryOp::Div
                    | BinaryOp::Rem
                    | BinaryOp::Shl
                    | BinaryOp::Shr
            ) && integer(definition, body, left)
        }
        Rvalue::Unary {
            op: UnaryOp::Neg,
            value,
        } => integer(definition, body, value),
        _ => false,
    }
}

/// Places an instruction names, in field order.
fn instruction_places(body: &Body, instruction: &Instruction) -> Vec<PlaceId> {
    match &instruction.kind {
        InstructionKind::Defuse { place }
        | InstructionKind::EndOrigin(place)
        | InstructionKind::StorageDead { place, .. }
        | InstructionKind::BeginLoan { place, .. }
        | InstructionKind::Drop { place, .. } => vec![*place],
        InstructionKind::BindOrigin { source, place, .. } => vec![*source, *place],
        InstructionKind::BindFields { source, fields } => std::iter::once(*source)
            .chain(fields.iter().copied())
            .collect(),
        InstructionKind::Assign { destination, value } => {
            let mut places = vec![*destination];
            if let Some(rvalue) = body.rvalues.get(value.0 as usize) {
                places.extend(rvalue_places(rvalue));
            }
            places
        }
        InstructionKind::Replace {
            destination,
            value,
            displaced,
            ..
        } => vec![*destination, value.place, *displaced],
        InstructionKind::Check { condition, .. } => vec![condition.place],
        InstructionKind::Snapshot { destination, value } => vec![*destination, value.place],
        InstructionKind::StorageLive(_)
        | InstructionKind::EndScope(_)
        | InstructionKind::EndLoan { .. }
        | InstructionKind::RunCleanup(_) => vec![],
    }
}

fn rvalue_places(rvalue: &Rvalue) -> Vec<PlaceId> {
    match rvalue {
        Rvalue::Use(value) | Rvalue::Unary { value, .. } => vec![value.place],
        Rvalue::Binary { left, right, .. } => vec![left.place, right.place],
        Rvalue::Construct { fields, .. } => fields.iter().map(|f| f.value.place).collect(),
        Rvalue::Closure { captures, .. } => captures.iter().map(|c| c.place).collect(),
        Rvalue::Constant(_)
        | Rvalue::ReadConstant { .. }
        | Rvalue::FunctionReference { .. }
        | Rvalue::Size(_)
        | Rvalue::Shape(_) => vec![],
    }
}

fn terminator_places(terminator: &Terminator) -> Vec<PlaceId> {
    match terminator {
        Terminator::Branch { condition, .. } => vec![condition.place],
        Terminator::Switch { value, .. } | Terminator::Return { value, .. } => vec![value.place],
        Terminator::Call {
            callee, arguments, ..
        } => {
            let mut places = match callee {
                Callee::Indirect { value, .. } => vec![value.place],
                Callee::Definition { .. } => vec![],
            };
            places.extend(arguments.iter().map(|a| a.value.place));
            places
        }
        Terminator::Goto(_)
        | Terminator::Match { .. }
        | Terminator::Fail { .. }
        | Terminator::Unreachable => vec![],
    }
}
