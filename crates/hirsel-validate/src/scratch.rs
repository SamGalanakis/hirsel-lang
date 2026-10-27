//! Per-definition proof caches. Temporary rows are interned into the session
//! interner; none is published back into immutable IR.
use hirsel_types::TypeId;
use std::cell::RefCell;

/// Like V's property proofs, these answers depend only on immutable rows,
/// signature views and this body's generic declarations. Live evidence is
/// applied by `transferred_type` before selecting a transformation.
#[derive(Clone, Copy, Eq, Hash, PartialEq)]
pub(super) enum Transformation {
    Copy(TypeId),
    Unique,
    Weaken,
}
pub(super) type Transformations = crate::hashed::HashMap<(TypeId, Transformation), TypeId>;

pub(super) type Acquisitions =
    std::collections::BTreeMap<(u32, Option<u32>), Vec<(hirsel_tir::PlaceId, &'static str)>>;

pub(super) struct Scratch {
    /// Symbolic construction receipts whose storage-row proof needs live flow.
    pub(super) closure_receipts:
        RefCell<std::collections::BTreeSet<(u32, Option<u32>, Option<u32>)>>,
    pub(super) origin_types: Vec<super::origin_types::Types>,
    pub(super) frame_authorities: Vec<std::cell::OnceCell<Option<hirsel_types::AuthoritySummary>>>,
    definition_inputs: std::cell::OnceCell<DefinitionInputs>,
    pub(super) fixed_constructions:
        Vec<RefCell<crate::hashed::HashMap<hirsel_types::TypeId, bool>>>,
    pub(super) escaping_roots: Vec<std::cell::OnceCell<std::collections::BTreeSet<u32>>>,
    pub(super) content_inputs: Vec<std::cell::OnceCell<super::flow::lifetimes::ContentInputs>>,
    /// Per body: operands typing received only as deep Global content, by
    /// block, instruction (none for the terminator) and place. Flow owes each
    /// the content proof (`Checker::content_proof`).
    pub(super) acquisitions: Vec<RefCell<Acquisitions>>,
    pub(super) parameter_authorities: Vec<
        std::cell::OnceCell<
            Option<
                std::collections::BTreeMap<
                    hirsel_types::ParameterRef,
                    hirsel_types::AuthoritySummary,
                >,
            >,
        >,
    >,
    pub(super) variant_inputs: Vec<std::cell::OnceCell<super::variant_brands::Inputs>>,
    /// Successful immutable row transformations, with a separate environment per body.
    pub(super) transformations: Vec<RefCell<Transformations>>,
    /// Per body: its structural property proofs (`proofs`).
    pub(super) proofs: Vec<RefCell<super::proofs::Proofs>>,
    /// Canonical Plain rows (`Checker::canonical_plain`), for every body.
    pub(super) canonical:
        RefCell<crate::hashed::HashMap<hirsel_types::TypeId, hirsel_types::TypeId>>,
    /// Per body: the type relations `Checker::related` proved.
    pub(super) relations: Vec<RefCell<super::typing::Relations>>,
    /// Per body: the layouts `Checker::layout` found.
    pub(super) layouts:
        Vec<RefCell<crate::hashed::HashMap<hirsel_types::TypeId, hirsel_types::TypeKind>>>,
    /// Per body, per place: every proper projected prefix is declared unique,
    /// so an owned call receipt's root evidence reaches the place (MODE-2).
    pub(super) receipt_paths: Vec<std::cell::OnceCell<Vec<bool>>>,
}
impl Scratch {
    pub(super) fn new(bodies: usize) -> Self {
        Self {
            closure_receipts: RefCell::default(),
            origin_types: (0..bodies)
                .map(|_| super::origin_types::Types::default())
                .collect(),
            frame_authorities: (0..bodies).map(|_| std::cell::OnceCell::new()).collect(),
            definition_inputs: std::cell::OnceCell::new(),
            fixed_constructions: (0..bodies)
                .map(|_| RefCell::new(crate::hashed::HashMap::default()))
                .collect(),
            escaping_roots: (0..bodies).map(|_| std::cell::OnceCell::new()).collect(),
            content_inputs: (0..bodies).map(|_| std::cell::OnceCell::new()).collect(),
            acquisitions: (0..bodies).map(|_| RefCell::default()).collect(),
            parameter_authorities: (0..bodies).map(|_| std::cell::OnceCell::new()).collect(),
            variant_inputs: (0..bodies).map(|_| std::cell::OnceCell::new()).collect(),
            transformations: (0..bodies).map(|_| RefCell::default()).collect(),
            proofs: (0..bodies).map(|_| RefCell::default()).collect(),
            layouts: (0..bodies).map(|_| RefCell::default()).collect(),
            relations: (0..bodies).map(|_| RefCell::default()).collect(),
            receipt_paths: (0..bodies).map(|_| std::cell::OnceCell::new()).collect(),
            canonical: RefCell::default(),
        }
    }
    pub(super) fn definition_inputs(
        &self,
        t: &hirsel_types::TypeInterner,
        definition: &hirsel_tir::Definition,
    ) -> &DefinitionInputs {
        self.definition_inputs
            .get_or_init(|| DefinitionInputs::new(t, definition))
    }
}

/// Immutable executable inputs, shared by every frame proof in this definition.
#[derive(Default)]
pub(super) struct Producer {
    pub(super) first_function: Option<hirsel_types::TypeId>,
    pub(super) last_header: Option<hirsel_types::TypeId>,
    pub(super) lends_captures: bool,
    pub(super) captures: Vec<(hirsel_tir::BodyId, hirsel_tir::RvalueId)>,
}
pub(super) struct DefinitionInputs {
    pub(super) producers: Vec<Producer>,
    pub(super) environment_paths:
        std::rc::Rc<std::collections::BTreeSet<Vec<super::flow_origins::EnvironmentField>>>,
    pub(super) origin_selectors: std::collections::BTreeSet<Vec<hirsel_types::FieldId>>,
}
impl DefinitionInputs {
    fn new(t: &hirsel_types::TypeInterner, definition: &hirsel_tir::Definition) -> Self {
        use super::flow_origins::EnvironmentField;
        use crate::{Constructor, InstructionKind, Rvalue};
        use hirsel_intern::work::{self, Counter};
        let mut producers: Vec<_> = (0..definition.bodies.len())
            .map(|_| Producer::default())
            .collect();
        let mut environment_paths = std::collections::BTreeSet::from([vec![]]);
        for (index, body) in definition.bodies.iter().enumerate() {
            for instruction in body.blocks.iter().flat_map(|block| &block.instructions) {
                work::count(Counter::ValidateDefinitionInputRows);
                if let InstructionKind::Assign { destination, value } = instruction.kind {
                    if let Rvalue::Closure { body: nested, .. } = body.rvalues[value.0 as usize] {
                        let producer = &mut producers[nested.0 as usize];
                        let ty = body.places[destination.0 as usize].ty;
                        producer.lends_captures |= t.get(ty).is_some_and(|row| {
                            row.modes.multiplicity != hirsel_types::Multiplicity::Once
                        });
                        if producer.first_function.is_none()
                            && t.get(ty).is_some_and(|row| {
                                matches!(row.kind, hirsel_types::TypeKind::Function(_))
                            })
                        {
                            producer.first_function = Some(ty);
                        }
                        producer.last_header = Some(ty);
                        producer
                            .captures
                            .push((hirsel_tir::BodyId(index as u32), value));
                    }
                }
            }
            for place in &body.places {
                work::count(Counter::ValidateDefinitionInputRows);
                let path = EnvironmentField::path(&place.projection);
                for start in 0..path.len() {
                    for end in start + 1..=path.len() {
                        environment_paths.insert(path[start..end].to_vec());
                    }
                }
            }
            for value in &body.rvalues {
                work::count(Counter::ValidateDefinitionInputRows);
                if let Rvalue::Construct {
                    constructor,
                    fields,
                    ..
                } = value
                {
                    for field in fields {
                        let key = match constructor {
                            Constructor::Variant { variant, .. } => {
                                EnvironmentField::Variant(*variant, field.field)
                            }
                            Constructor::Sequence | Constructor::Map | Constructor::Set => {
                                EnvironmentField::Index
                            }
                            _ => EnvironmentField::Field(field.field),
                        };
                        environment_paths.insert(vec![key]);
                    }
                }
            }
        }
        let mut origin_selectors = std::collections::BTreeSet::new();
        // A copied projection receives identities relative to that value.
        // For a brand on x.a.other, copying x.a must retain the `other`
        // selector as well as the enclosing `a.other` path. Direct callee
        // headers need not occur in the definition's emitted type closure.
        let mut select = |origin: &hirsel_types::Origin| {
            let fields: Vec<_> = origin.fields.iter().copied().collect();
            for start in 0..fields.len() {
                origin_selectors.insert(fields[start..].to_vec());
            }
        };
        for row in definition.types.iter().map(|&id| t.row(id)) {
            work::count(Counter::ValidateDefinitionInputRows);
            for origin in &row.brand.alternatives {
                select(origin);
            }
            for reach in row.modes.relation.reaches() {
                if let hirsel_types::Reach::Place(origin) = reach {
                    select(origin);
                }
            }
        }
        Self {
            producers,
            environment_paths: std::rc::Rc::new(environment_paths),
            origin_selectors,
        }
    }
}
