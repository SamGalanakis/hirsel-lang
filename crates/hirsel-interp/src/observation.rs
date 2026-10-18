//! The semantic comparison boundary shared by reference and native engines.
use crate::{Execution, Failure, Outcome, TraceEvent, Value};
use hirsel_span::{SourceMap, Span};
use hirsel_syntax::DefPath;
use hirsel_tir::{BodyId, CapabilityEvent, CleanupId};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Anchor {
    pub definition: DefPath,
    pub body: BodyId,
    pub cleanup: Option<(CleanupId, u32)>,
    pub site: Span,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Effect {
    HandleDropped { kind: String, id: u64 },
    ResourceCreated { definition: DefPath },
    Finalizer { definition: DefPath },
    Forgotten { definition: DefPath },
    CapabilityConsumed(CapabilityEvent),
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OrderedEffect {
    pub anchor: Anchor,
    pub effect: Effect,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FailureRecord {
    pub failure: Failure,
    pub anchor: Anchor,
    pub cause: Option<usize>,
    pub spawn_path: Vec<u32>,
}
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct SourceSite {
    pub definition: DefPath,
    pub line: u32,
    pub column: u32,
}
/// Role and selected-source coordinates identify semantic events independently of
/// body, block and cleanup arenas. Repeated occurrences retain vector order.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SemanticRole {
    ResourceConstruction,
    FinalizerEntry,
    Forget,
    HandleDrop,
    CapabilityEffect,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SemanticAnchor {
    pub source: SourceSite,
    pub role: SemanticRole,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ObservedEffect {
    pub anchor: SemanticAnchor,
    pub effect: Effect,
}
impl Anchor {
    pub fn source_site(&self, sources: &SourceMap) -> Option<SourceSite> {
        let loc = sources.location(self.site)?;
        Some(SourceSite {
            definition: self.definition.clone(),
            line: loc.line,
            column: loc.column,
        })
    }
}
impl OrderedEffect {
    pub fn semantic_anchor(&self, sources: &SourceMap) -> Option<SemanticAnchor> {
        let role = match &self.effect {
            Effect::ResourceCreated { .. } => SemanticRole::ResourceConstruction,
            Effect::Finalizer { .. } => SemanticRole::FinalizerEntry,
            Effect::Forgotten { .. } => SemanticRole::Forget,
            Effect::HandleDropped { .. } => SemanticRole::HandleDrop,
            Effect::CapabilityConsumed(_) => SemanticRole::CapabilityEffect,
        };
        Some(SemanticAnchor {
            source: self.anchor.source_site(sources)?,
            role,
        })
    }
}
/// A language failure node; indices refer to the ordered `failures` arena.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ObservedFailure {
    pub kind: String,
    pub rule: hirsel_diag::RuleId,
    pub stable_site: SourceSite,
    pub spawn_path: Vec<u32>,
    pub cause_indices: Vec<usize>,
}
/// Semantic value tags retain scalar widths, nominal identities and sequence kinds.
/// Callable values require behavior probes and cannot compare equal structurally.
#[derive(Clone, Debug, PartialEq)]
pub struct ObservedValue {
    /// Root is node zero. Fields index this owned arena, never interpreter bodies.
    pub nodes: Vec<ObservedNode>,
}
#[derive(Clone, Debug, PartialEq)]
pub enum ObservedNode {
    Unit,
    Bool(bool),
    Integer(crate::Integer),
    F32(u32),
    F64(u64),
    Char(char),
    String(String),
    Bytes(Vec<u8>),
    Handle {
        kind: String,
        logical_id: u64,
    },
    Aggregate {
        kind: ObservedAggregate,
        fields: Vec<Option<usize>>,
    },
    FunctionReference {
        definition: DefPath,
        arguments: Vec<Option<Vec<u8>>>,
    },
    NonComparableFunction,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ObservedAggregate {
    Map,
    Set,
    Tuple,
    Box,
    Record(DefPath),
    Distinct(DefPath),
    Resource(DefPath),
    ResourceVariant(DefPath, u32),
    Variant(u32),
    Sequence(hirsel_types::BuiltinType),
}
impl ObservedValue {
    pub fn comparable(&self) -> bool {
        !self
            .nodes
            .iter()
            .any(|n| matches!(n, ObservedNode::NonComparableFunction) || matches!(n, ObservedNode::FunctionReference { arguments, .. } if arguments.iter().any(Option::is_none)))
    }
    pub fn from_value(value: &Value) -> Self {
        use ObservedAggregate as A;
        use ObservedNode as O;
        let mut nodes = vec![];
        let mut pending = std::collections::VecDeque::from([value]);
        while let Some(value) = pending.pop_front() {
            let (kind, fields) = match value {
                Value::Unit => {
                    nodes.push(O::Unit);
                    continue;
                }
                Value::Bool(v) => {
                    nodes.push(O::Bool(*v));
                    continue;
                }
                Value::Integer(v) => {
                    nodes.push(O::Integer(*v));
                    continue;
                }
                Value::Char(v) => {
                    nodes.push(O::Char(*v));
                    continue;
                }
                Value::F32(v) => {
                    nodes.push(O::F32(if f32::from_bits(*v).is_nan() {
                        0x7fc0_0000
                    } else {
                        *v
                    }));
                    continue;
                }
                Value::F64(v) => {
                    nodes.push(O::F64(if f64::from_bits(*v).is_nan() {
                        0x7ff8_0000_0000_0000
                    } else {
                        *v
                    }));
                    continue;
                }
                Value::String(v) => {
                    nodes.push(O::String(v.clone()));
                    continue;
                }
                Value::Bytes(v) => {
                    nodes.push(O::Bytes(v.clone()));
                    continue;
                }
                Value::Handle { kind, id } => {
                    nodes.push(O::Handle {
                        kind: kind.clone(),
                        logical_id: *id,
                    });
                    continue;
                }
                Value::Shared(root) => {
                    nodes.push(O::Handle {
                        kind: "Shared".into(),
                        logical_id: root.id(),
                    });
                    continue;
                }
                Value::FunctionReference(reference) => {
                    nodes.push(O::FunctionReference {
                        definition: reference.definition.clone(),
                        arguments: reference
                            .arguments
                            .iter()
                            .map(|argument| {
                                hirsel_types::encoding::canonical_argument_encoding(
                                    &reference.types,
                                    argument,
                                )
                                .ok()
                            })
                            .collect(),
                    });
                    continue;
                }
                Value::Closure { .. } => {
                    nodes.push(O::NonComparableFunction);
                    continue;
                }
                Value::Map { entries: f, .. } => (A::Map, f),
                Value::Set { entries: f, .. } => (A::Set, f),
                Value::Tuple(f) => (A::Tuple, f),
                Value::Box(f) => (A::Box, f),
                Value::Record { definition, fields } => (A::Record(definition.clone()), fields),
                Value::Distinct { definition, fields } => (A::Distinct(definition.clone()), fields),
                Value::Resource {
                    definition, fields, ..
                } => (A::Resource(definition.clone()), fields),
                Value::ResourceVariant {
                    definition,
                    variant,
                    fields,
                    ..
                } => (A::ResourceVariant(definition.clone(), *variant), fields),
                Value::Variant { variant, fields } => (A::Variant(*variant), fields),
                Value::Sequence { kind, elements, .. } => (A::Sequence(*kind), elements),
            };
            let fields = fields
                .iter()
                .map(|f| {
                    f.as_ref().map(|v| {
                        let index = nodes.len() + pending.len() + 1;
                        pending.push_back(v);
                        index
                    })
                })
                .collect();
            nodes.push(O::Aggregate { kind, fields });
        }
        Self { nodes }
    }
}
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct ObservedWait {
    pub spawn_path: Vec<u32>,
    pub site: SourceSite,
    pub kind: String,
    pub targets: Vec<u64>,
}
#[derive(Clone, Debug, PartialEq)]
pub struct Observable {
    pub blocked: Option<Vec<ObservedWait>>,
    pub schema_version: u32,
    /// Canonical declared result graph, populated by the driver (no arena IDs).
    pub returned_type: Option<Vec<u8>>,
    pub returned: Option<ObservedValue>,
    pub exit_code: Option<i32>,
    pub output: Vec<u8>,
    pub effects: Vec<ObservedEffect>,
    pub failures: Vec<ObservedFailure>,
    pub final_state: Option<crate::library::simulation::ObservedSimulation>,
}
impl Observable {
    /// Function-valued observations cannot silently establish engine parity.
    pub fn equivalent(&self, other: &Self) -> bool {
        self.returned.as_ref().is_none_or(ObservedValue::comparable)
            && other
                .returned
                .as_ref()
                .is_none_or(ObservedValue::comparable)
            && self == other
    }
}
impl Execution {
    /// Tooling limits are not language observations; no comparison is produced.
    pub fn observable(&self, sources: &SourceMap) -> Option<Observable> {
        if matches!(
            self.outcome,
            Outcome::ToolingLimit(_) | Outcome::UndefinedBehavior(_)
        ) {
            return None;
        }
        let site = |a: &Anchor| a.source_site(sources);
        let failures = self
            .failures
            .iter()
            .map(|f| {
                if f.cause.is_some_and(|i| i >= self.failures.len()) {
                    return None;
                }
                Some(ObservedFailure {
                    kind: f.failure.kind.clone(),
                    rule: f.failure.rule.clone(),
                    stable_site: site(&f.anchor)?,
                    spawn_path: f.spawn_path.clone(),
                    cause_indices: f.cause.into_iter().collect(),
                })
            })
            .collect::<Option<Vec<_>>>()?;
        let effects = self
            .trace
            .iter()
            .filter_map(|event| match event {
                TraceEvent::Effect(e) => Some(e),
                _ => None,
            })
            .map(|e| {
                Some(ObservedEffect {
                    anchor: e.semantic_anchor(sources)?,
                    effect: e.effect.clone(),
                })
            })
            .collect::<Option<Vec<_>>>()?;
        let (returned, exit_code) = match &self.outcome {
            Outcome::Returned { value, exit_code } => {
                (Some(ObservedValue::from_value(value)), Some(*exit_code))
            }
            _ => (None, None),
        };
        let blocked = if let Outcome::Blocked(waits) = &self.outcome {
            let mut waits = waits
                .iter()
                .map(|wait| {
                    Some(ObservedWait {
                        spawn_path: wait.spawn_path.clone(),
                        site: wait.anchor.source_site(sources)?,
                        kind: wait.kind.clone(),
                        targets: wait.targets.clone(),
                    })
                })
                .collect::<Option<Vec<_>>>()?;
            waits.sort();
            Some(waits)
        } else {
            None
        };
        Some(Observable {
            blocked,
            schema_version: 1,
            returned_type: None,
            returned,
            exit_code,
            output: self.stdout.clone(),
            effects,
            failures,
            final_state: None,
        })
    }
}
