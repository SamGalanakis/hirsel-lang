//! One compact graph row per original base/closed-profile context. All uses
//! enter that row before demand is solved, so recursion and multiple callers
//! share one conservative requirement. No optimization clone limit applies.
//!
//! This is compiler DATA assembly. Independent current source/V observations,
//! complete profiles/target sets, physical ABI selection and C acceptance remain
//! query-owner obligations. A key or this output grants none of that authority.
use super::*;
use crate::sealed_source::demand::ExternalDemand;
use hirsel_layout::instance::InstanceKey;
use std::collections::{BTreeSet, HashMap};

const PROFILE: &str = "hirsel.closed-storage-profile.v1";
const DEMAND: &str = "hirsel.receipt-demand.v1";

/// Registry-local graph index. It is never persisted as a source identity.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct ContextId(usize);
impl ContextId {
    pub fn index(self) -> usize {
        self.0
    }
}

pub struct Context<'a> {
    /// Original lexical key, before profile/demand or optimization derivation.
    pub base: &'a InstanceKey,
    pub entry: &'a SourceCallableEntry,
    pub source: &'a SourceDemandProjection,
    pub ports: &'a BTreeMap<Port, ClosedStorageProfile>,
}

struct ContextEncoding {
    schema: u32,
    callable: ClosedStorageType,
    ports: Vec<ProfilePort>,
}
struct ProfilePort {
    kind: u32,
    ordinal: u32,
    profile: ClosedStorageProfile,
}
hirsel_codec::snapshot_struct!(ContextEncoding {
    schema,
    callable,
    ports
});
hirsel_codec::snapshot_struct!(ProfilePort {
    kind,
    ordinal,
    profile
});

impl Context<'_> {
    /// One complete source callable context, preserving which profile belongs
    /// to each original port. Source observations stay out of these bytes.
    /// Completeness of each profile is still the source query owner's proof.
    pub fn canonical_bytes(&self) -> Result<Vec<u8>, SourceError> {
        let expected = self
            .entry
            .parameters()
            .iter()
            .map(|p| Port::Parameter(p.ordinal))
            .chain([Port::Result])
            .chain(
                self.entry
                    .captures()
                    .iter()
                    .map(|c| Port::Capture(c.ordinal)),
            )
            .collect::<BTreeSet<_>>();
        if !expected.iter().eq(self.ports.keys()) {
            return Err(SourceError::Malformed(
                "receipt context needs a complete ordered port-profile map",
            ));
        }
        binding(&Input {
            entry: self.entry,
            source: self.source,
            profiles: self.ports,
            calls: &BTreeMap::new(),
            closures: &BTreeMap::new(),
        })?;
        let ports: Vec<_> = self
            .ports
            .iter()
            .map(|(port, profile)| {
                let (kind, ordinal) = match port {
                    Port::Parameter(ordinal) => (0u32, *ordinal),
                    Port::Result => (1, 0),
                    Port::Capture(ordinal) => (2, *ordinal),
                };
                ProfilePort {
                    kind,
                    ordinal,
                    profile: profile.clone(),
                }
            })
            .collect();
        Ok(hirsel_codec::to_text(&ContextEncoding {
            schema: 1,
            callable: self.entry.callable().clone(),
            ports,
        })
        .into_bytes())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Expansion {
    Targets(Vec<ContextId>),
    /// Independently complete source operation data, including known empty.
    External(ExternalDemand),
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Links {
    calls: BTreeMap<u32, Expansion>,
    closures: BTreeMap<u32, ContextId>,
}

struct Row {
    base: InstanceKey,
    context: InstanceKey,
    entry: SourceCallableEntry,
    source: SourceDemandProjection,
    ports: BTreeMap<Port, ClosedStorageProfile>,
    links: Option<Links>,
}

#[derive(Default)]
pub struct Registry {
    rows: Vec<Row>,
    by_context: HashMap<InstanceKey, ContextId>,
}

/// Logical request data. An ordinary key is usable only when its source entry
/// is independently legal; this record does not authorize an ordinary fallback.
#[derive(Debug)]
pub struct Request {
    ordinary: InstanceKey,
    context: InstanceKey,
    aware: Option<InstanceKey>,
    declaration: Declaration,
}
impl Request {
    pub fn ordinary_key(&self) -> &InstanceKey {
        &self.ordinary
    }
    pub fn context_key(&self) -> &InstanceKey {
        &self.context
    }
    pub fn aware_key(&self) -> Option<&InstanceKey> {
        self.aware.as_ref()
    }
    pub fn declaration(&self) -> &Declaration {
        &self.declaration
    }
}

impl Registry {
    pub fn len(&self) -> usize {
        self.rows.len()
    }
    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    /// Register every source context before linking targets. Repeated uses of
    /// the same complete context obtain one index, never independent clones.
    pub fn register(&mut self, input: Context<'_>) -> Result<ContextId, SourceError> {
        let reference = input.entry.reference();
        if &reference.definition != input.base.definition()
            || reference.body != input.base.body()
            || reference.instance != input.base.digest()
        {
            return Err(SourceError::Malformed(
                "receipt context lacks its original lexical base",
            ));
        }
        let bytes = input.canonical_bytes()?;
        let context = input.base.derived(PROFILE, &[&bytes]);
        if let Some(&id) = self.by_context.get(&context) {
            let old = &self.rows[id.0];
            if old.entry != *input.entry
                || old.source.reference() != input.source.reference()
                || old.source.source_identity() != input.source.source_identity()
                || old.source.implementation_revision() != input.source.implementation_revision()
                || old.source.projection() != input.source.projection()
                || old.ports != *input.ports
            {
                return Err(SourceError::Malformed(
                    "conflicting original data for receipt profile context",
                ));
            }
            return Ok(id);
        }
        let id = ContextId(self.rows.len());
        self.rows.push(Row {
            base: input.base.clone(),
            context: context.clone(),
            entry: input.entry.clone(),
            source: input.source.clone(),
            ports: input.ports.clone(),
            links: None,
        });
        self.by_context.insert(context, id);
        Ok(id)
    }

    /// Supply complete original expansions. None and known empty stay distinct.
    /// Repeated complete answers must agree; partial target sets cannot pretend
    /// to be complete or replace an earlier answer at an optimization limit.
    pub fn resolve(
        &mut self,
        id: ContextId,
        mut calls: BTreeMap<u32, Expansion>,
        closures: BTreeMap<u32, ContextId>,
    ) -> Result<(), SourceError> {
        let row = self
            .rows
            .get(id.0)
            .ok_or(SourceError::Malformed("receipt context index"))?;
        let source = row.source.projection();
        if !source.calls().keys().eq(calls.keys()) || !source.closures().keys().eq(closures.keys())
        {
            return Err(SourceError::Malformed(
                "receipt expansions differ from original source selectors",
            ));
        }
        for (&block, expansion) in &mut calls {
            if let Expansion::Targets(targets) = expansion {
                targets.sort_unstable();
                targets.dedup();
                if targets.is_empty() {
                    return Err(SourceError::Unsupported(
                        "receipt call has no original targets",
                    ));
                }
                for target in targets {
                    let target = self
                        .rows
                        .get(target.0)
                        .ok_or(SourceError::Malformed("receipt target context index"))?;
                    if let hirsel_tir::Callee::Definition { path, .. } =
                        &source.calls()[&block].callee
                        && path != &target.entry.reference().definition
                    {
                        return Err(SourceError::Malformed(
                            "receipt target differs from original definition",
                        ));
                    }
                }
            }
        }
        for (&value, target) in &closures {
            let target = self
                .rows
                .get(target.0)
                .ok_or(SourceError::Malformed("receipt closure context index"))?;
            if target.entry.reference().definition != row.entry.reference().definition
                || source.closures()[&value]
                    .iter()
                    .any(|c| c.body.0 != target.entry.reference().body)
            {
                return Err(SourceError::Malformed(
                    "receipt closure differs from original producer",
                ));
            }
        }
        let links = Links { calls, closures };
        let row = &mut self.rows[id.0];
        if row.links.as_ref().is_some_and(|old| old != &links) {
            return Err(SourceError::Malformed(
                "conflicting complete receipt target expansions",
            ));
        }
        row.links = Some(links);
        Ok(())
    }

    /// Solve the quotient graph once. Semantic contracts and their logical
    /// addresses exclude body/V revisions; declarations retain observations
    /// separately for the independent action and acceptance boundaries.
    pub fn solve(&self) -> Result<Vec<Request>, SourceError> {
        let mut calls = Vec::new();
        let mut closures = Vec::new();
        for row in &self.rows {
            let links = row.links.as_ref().ok_or(SourceError::Unsupported(
                "receipt context lacks complete original expansion",
            ))?;
            calls.push(
                links
                    .calls
                    .iter()
                    .map(|(&block, expansion)| {
                        (
                            block,
                            match expansion {
                                Expansion::Targets(targets) => {
                                    CallDemand::Bodies(targets.iter().map(|id| id.0).collect())
                                }
                                Expansion::External(data) => CallDemand::External(data.clone()),
                            },
                        )
                    })
                    .collect(),
            );
            closures.push(
                links
                    .closures
                    .iter()
                    .map(|(&value, id)| (value, id.0))
                    .collect(),
            );
        }
        let inputs = self
            .rows
            .iter()
            .enumerate()
            .map(|(i, row)| Input {
                entry: &row.entry,
                source: &row.source,
                profiles: &row.ports,
                calls: &calls[i],
                closures: &closures[i],
            })
            .collect::<Vec<_>>();
        let declarations = derive(&inputs)?;
        self.rows
            .iter()
            .zip(declarations)
            .map(|(row, declaration)| {
                let aware = declaration
                    .contract()
                    .map(|contract| -> Result<InstanceKey, SourceError> {
                        Ok(row.context.derived(
                            DEMAND,
                            &[&contract.canonical_bytes().map_err(SourceError::Malformed)?],
                        ))
                    })
                    .transpose()?;
                Ok(Request {
                    ordinary: row.base.clone(),
                    context: row.context.clone(),
                    aware,
                    declaration,
                })
            })
            .collect()
    }
}
