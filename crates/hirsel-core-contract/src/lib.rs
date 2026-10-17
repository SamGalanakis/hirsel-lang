//! Operation manifest rows shared by every engine.
//!
//! Data only. The driver registry produces operation identities; engines supply
//! dispositions; nothing here infers a schema from an implementation.
pub mod snapshot;
use hirsel_syntax::{ContentHash, DefPath, PassingMode};

/// Declared parameter convention (OWN-1/2/3), hashable for manifest and ABI keys.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Ord, PartialOrd)]
pub enum Passing {
    Borrow,
    Take,
    Mut,
}

impl From<PassingMode> for Passing {
    fn from(mode: PassingMode) -> Self {
        match mode {
            PassingMode::Borrow => Self::Borrow,
            PassingMode::Take => Self::Take,
            PassingMode::Mut => Self::Mut,
        }
    }
}
use std::collections::{BTreeMap, BTreeSet};

/// Canonical registry identity of one operation: its path and declaration digest.
#[derive(Clone, Debug, Eq, PartialEq, Hash, Ord, PartialOrd)]
pub struct OperationId {
    pub definition: DefPath,
    pub declaration_digest: ContentHash,
}

/// The engines that execute an operation. Each has its own disposition.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Ord, PartialOrd)]
pub enum Engine {
    /// The reference interpreter over TIR.
    Reference,
    /// Compiled native code (LLVM emission plus `hirsel-rt`).
    Native,
    /// The independent checked post-RC heap executor.
    HeapCheck,
}

impl Engine {
    pub const ALL: [Engine; 3] = [Engine::Reference, Engine::Native, Engine::HeapCheck];
}

/// How one engine implements an operation (declaration-only, TIR/Hirsel
/// body, instruction, runtime primitive, or explicitly unavailable).
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub enum Disposition {
    /// A type or constant declaration with no executable entry.
    DeclarationOnly,
    /// An ordinary TIR/Hirsel body.
    Body,
    /// An operation lowered to LIR instructions, named by its stable spelling:
    /// the scalar intrinsics, and the sequence reads and constructions the
    /// native library lowers inline. No call to it remains at Ready.
    Instruction(String),
    /// A `hirsel_rt_v2` runtime primitive or typed leaf kernel, named by its C symbol.
    RuntimePrimitive(String),
    /// An explicit engine outcome, never a silent fallback to another engine.
    Unavailable(Unavailable),
}

/// Why an operation is unavailable on an engine.
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub enum Unavailable {
    /// The skeleton default: this row is not classified yet.
    Unclassified,
    /// An M2 feature (tasks, channels, Shared, arenas, ...).
    M2(M2Feature),
    /// Waiting on an open spec issue.
    OpenSpec(u32),
    /// Classified, but this engine does not implement it yet.
    NotImplemented,
}

/// M2-only operation families that stay explicit unavailable rows in M1b.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Ord, PartialOrd)]
pub enum M2Feature {
    Tasks,
    Channels,
    Mutexes,
    Atomics,
    Shared,
    Arenas,
    Regions,
    Slots,
}

/// Destruction effect of a parameter or result, mirroring `DropAllocation`.
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub enum CleanupEffect {
    NoAlloc,
    MayAllocate,
    /// Depends on the drop effect of these generic parameter ordinals.
    DependsOn(Vec<u32>),
}

/// Exact call ownership of one parameter, by declaration ordinal.
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct ParameterOwnership {
    pub ordinal: u32,
    pub passing: Passing,
    pub cleanup: CleanupEffect,
}

/// Exact call ownership: Take/Mut/Borrow per parameter and result cleanup.
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct CallOwnership {
    pub parameters: Vec<ParameterOwnership>,
    pub result_cleanup: CleanupEffect,
}

/// Independent semantic effect dimensions; `true` includes unknown.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub struct SemanticEffects {
    pub reads: bool,
    pub writes: bool,
    pub owns: bool,
    pub may_fail: bool,
    pub allocates: bool,
    pub observable: bool,
}

impl SemanticEffects {
    /// Every dimension unknown: the only default a row may start from.
    pub const CONSERVATIVE: Self = Self {
        reads: true,
        writes: true,
        owns: true,
        may_fail: true,
        allocates: true,
        observable: true,
    };
}

/// Identity of one independent test vector set for an operation.
#[derive(Clone, Debug, Eq, PartialEq, Hash, Ord, PartialOrd)]
pub struct TestVectorId(pub String);

/// One disposition per engine; each admits `Unavailable`.
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct EngineDispositions {
    pub reference: Disposition,
    pub native: Disposition,
    pub heap_check: Disposition,
}

impl EngineDispositions {
    pub fn unclassified() -> Self {
        let none = Disposition::Unavailable(Unavailable::Unclassified);
        Self {
            reference: none.clone(),
            native: none.clone(),
            heap_check: none,
        }
    }
    pub fn get(&self, engine: Engine) -> &Disposition {
        match engine {
            Engine::Reference => &self.reference,
            Engine::Native => &self.native,
            Engine::HeapCheck => &self.heap_check,
        }
    }
}

/// The semantic rule a runtime export asks its caller to record on status 1.
/// Numeric runtime codes are assigned for each program by the driver.
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct RuleCode(pub String);

/// Rules the runtime records itself, inside an export whose row names no rule, at fixed
/// codes: `RUNTIME_RULE_BASE + i` is `RUNTIME_RULES[i]`. Per-program codes stay below it.
pub const RUNTIME_RULE_BASE: u32 = 1 << 31;
pub const RUNTIME_RULES: [&str; 2] = ["CAP-2", "LANG-9"];

/// The fixed runtime code of `rule`, if the runtime may record it.
pub fn runtime_rule_code(rule: &str) -> Option<u32> {
    RUNTIME_RULES
        .iter()
        .position(|r| *r == rule)
        .map(|i| RUNTIME_RULE_BASE + i as u32)
}

/// One manifest row per registry operation.
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct OperationRow {
    pub id: OperationId,
    /// None until the row is classified: unavailable, never guessed.
    pub ownership: Option<CallOwnership>,
    pub effects: SemanticEffects,
    pub dispositions: EngineDispositions,
    pub vectors: Vec<TestVectorId>,
    /// Native deterministic failure recorded by the caller at the call anchor.
    /// None means the export succeeds or propagates a failure already recorded.
    pub failure: Option<RuleCode>,
}

impl OperationRow {
    /// The conservative unclassified row for one identity.
    pub fn unclassified(id: OperationId) -> Self {
        Self {
            id,
            ownership: None,
            effects: SemanticEffects::CONSERVATIVE,
            dispositions: EngineDispositions::unclassified(),
            vectors: Vec::new(),
            failure: None,
        }
    }
}

/// A manifest whose row set is fixed at construction from registry identities.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Manifest {
    rows: BTreeMap<OperationId, OperationRow>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ManifestError {
    /// `replace` named an identity that is not a row; rows are never added.
    UnknownOperation(OperationId),
}

/// Row identities that differ from an inventory, in both directions.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CoverageMismatch {
    pub missing: BTreeSet<OperationId>,
    pub extra: BTreeSet<OperationId>,
}

impl Manifest {
    /// Every identity gets the conservative unclassified row.
    pub fn skeleton(ids: impl IntoIterator<Item = OperationId>) -> Self {
        Self {
            rows: ids
                .into_iter()
                .map(|id| (id.clone(), OperationRow::unclassified(id)))
                .collect(),
        }
    }
    /// Classify an existing row. A new identity is an error, never an insertion.
    pub fn replace(&mut self, row: OperationRow) -> Result<(), ManifestError> {
        match self.rows.get_mut(&row.id) {
            Some(slot) => {
                *slot = row;
                Ok(())
            }
            None => Err(ManifestError::UnknownOperation(row.id)),
        }
    }
    pub fn row(&self, id: &OperationId) -> Option<&OperationRow> {
        self.rows.get(id)
    }
    /// Rows in canonical identity order.
    pub fn rows(&self) -> impl ExactSizeIterator<Item = &OperationRow> {
        self.rows.values()
    }
    pub fn len(&self) -> usize {
        self.rows.len()
    }
    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }
    /// Set equality of row identities against an inventory.
    pub fn check_coverage(
        &self,
        inventory: impl IntoIterator<Item = OperationId>,
    ) -> Result<(), CoverageMismatch> {
        let inventory: BTreeSet<_> = inventory.into_iter().collect();
        let rows: BTreeSet<_> = self.rows.keys().cloned().collect();
        let mismatch = CoverageMismatch {
            missing: inventory.difference(&rows).cloned().collect(),
            extra: rows.difference(&inventory).cloned().collect(),
        };
        if mismatch.missing.is_empty() && mismatch.extra.is_empty() {
            Ok(())
        } else {
            Err(mismatch)
        }
    }
    /// Identities that still lack a disposition on `engine`.
    pub fn unclassified(&self, engine: Engine) -> impl Iterator<Item = &OperationId> {
        self.rows.values().filter_map(move |row| {
            (row.dispositions.get(engine) == &Disposition::Unavailable(Unavailable::Unclassified))
                .then_some(&row.id)
        })
    }
}
