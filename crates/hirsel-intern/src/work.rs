//! Deterministic per-thread work counters shared by every phase. Variants name
//! the measured unit; increments stay at the call site they describe. Reads are
//! cumulative for the current thread, so scaling bounds stay machine-independent.
use std::cell::Cell;

/// One measured kind of work, named for its owner and unit.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Counter {
    /// Rows a session type interner adds: row content no earlier row had.
    InternedTypeRows,
    /// Declaration rows copied into shared generic declaration snapshots.
    CopiedGenericDeclarationRows,
    /// Rows normalized once for their semantic, mode-erased or shape normal
    /// form, or given a nominal-expansion height.
    TypeNormalRows,
    /// Private ownership-state rows copied by V's shared snapshots.
    ValidateCopiedRows,
    /// V's scope-index construction, search, selected-local and ordering work.
    ValidateScopeLocalRows,
    /// V's pattern-coverage work units (spec #33).
    ValidateMatchWork,
    /// V's binding-origin candidates examined for call proof inputs.
    ValidateCallProofRows,
    /// V's retained-value and ended-origin candidates inspected by expiry.
    ValidateOriginDependencyRows,
    /// V's definition rows inspected for executable and projection inputs.
    ValidateDefinitionInputRows,
    /// V's candidate place rows inspected by ownership accesses.
    ValidatePlaceRows,
    /// V's snapshot rows prepared for assignment failure unions.
    ValidateFailureStateRows,
    /// V's backward result-permission block transfers.
    ValidateResultDemandSteps,
    /// V's forward ownership block transfers.
    ValidateFlowSteps,
    /// V's candidate roots for temporary ordering and cleanup.
    ValidateCleanupRootRows,
    /// V's cleanup action replays.
    ValidateCleanupReplaySteps,
    /// V's lexical-declaration instruction inspections.
    ValidateDeclarationRows,
    /// V's signature input inspections.
    ValidateSignatureRows,
    /// Binding rows copied by shared mode-flow snapshots.
    CopiedSlotRows,
    /// Ownership source-index construction and lookup candidates.
    OwnershipSourceRows,
    /// Ownership forward fixed-point node transfers, including diagnostic replay.
    OwnershipFlowSteps,
    /// Ownership value joins, including state and operand joins.
    OwnershipValueJoins,
    /// I's backward temporary-demand node transfers.
    OwnershipLivenessSteps,
    /// I's binding rows inspected for ownership frame inputs.
    OwnershipInitializedRows,
    /// J's source-row and closure-table lookups.
    AuthoritySourceRows,
    /// J's binding holders inspected by identity-lifetime queries.
    AuthorityHolderRows,
    /// J's immutable-input and invalidated-brand candidates.
    AuthorityInvalidationRows,
    /// K's checked-decision rows indexed or looked up.
    LoweringDecisionRows,
    /// K's live-root rows inspected while compiling cleanup chains.
    LoweringCleanupRootRows,
    /// K's live-root and loan-source rows for statement/scope retirement.
    LoweringRetirementRows,
    /// K's scoped storage binding lookup rows.
    LoweringBindingRows,
    /// H's source-name, scope and elaboration lookups.
    BodySourceRows,
    /// H's source-place publication lookups.
    BodySourcePlaceRows,
    /// H's pattern matrices examined by usefulness and exhaustiveness.
    BodyUsefulnessMatrices,
    /// Allocation/representation audit source-index candidates.
    AuditSourceRows,
    /// Source declaration inspections while composing module inputs.
    ModuleDeclarationRows,
    /// Row transfers while solving and replaying cleanup version inputs.
    RcCleanupVersionRows,
    /// Value nodes the reference interpreter copies beneath a copied value, and
    /// the elements of a shared block its first write detaches.
    InterpCopiedValues,
    /// Ordered collection entries indexed, compared or appended by the interpreter.
    InterpKeyedRows,
    /// Cached dependency-node inspections.
    CacheValidationRows,
}

// CacheValidationRows must stay the last variant; the array is sized by it.
const VARIANTS: usize = Counter::CacheValidationRows as usize + 1;

thread_local! {
    static COUNTERS: [Cell<u64>; VARIANTS] = const { [const { Cell::new(0) }; VARIANTS] };
}

/// Add one unit of `counter` work on this thread.
#[inline]
pub fn count(counter: Counter) {
    count_by(counter, 1);
}

/// Add `rows` units of `counter` work on this thread.
pub fn count_by(counter: Counter, rows: u64) {
    COUNTERS.with(|counters| {
        let cell = &counters[counter as usize];
        cell.set(cell.get() + rows);
    });
}

/// Cumulative `counter` units on this thread.
pub fn read(counter: Counter) -> u64 {
    COUNTERS.with(|counters| counters[counter as usize].get())
}

/// Every counter's value on one thread, carried to another by `jobs::map`.
pub struct Tally([u64; VARIANTS]);

impl Tally {
    /// This thread's cumulative counters.
    pub fn read() -> Tally {
        Tally(COUNTERS.with(|counters| counters.each_ref().map(Cell::get)))
    }

    /// Add these counters to this thread's.
    pub fn add(&self) {
        COUNTERS.with(|counters| {
            for (cell, rows) in counters.iter().zip(self.0) {
                cell.set(cell.get() + rows);
            }
        });
    }
}
