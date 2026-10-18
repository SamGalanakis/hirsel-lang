//! The executor's semantic observation, following semantic observation schema
//! v1 (`hirsel_interp::observation`): the returned value structurally, output
//! bytes and the ordered failure chain. It never contains allocation IDs,
//! counts, reuse events or addresses. The executor cannot depend on the
//! interpreter crate, so mapping this onto `Observable` (nominal identities,
//! line/column sites) belongs to the driver's differential gate.
use hirsel_lir::layout_types::ScalarKind;
use hirsel_syntax::DefPath;

pub const OBSERVATION_SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Observed {
    Unit,
    Bool(bool),
    Integer {
        scalar: ScalarKind,
        bits: u128,
    },
    F32(u32),
    F64(u64),
    /// Inline fields in declaration order; None for an uninitialized field.
    Aggregate(Vec<Option<Observed>>),
    Variant {
        tag: u32,
        fields: Vec<Option<Observed>>,
    },
    /// The payload of a block reference.
    Block(Box<Observed>),
    Handle(u64),
    /// Function values need behavior probes; they never compare equal.
    NonComparableFunction,
    /// A representation this executor does not inspect structurally.
    Opaque,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ObservedFailure {
    pub rule: String,
    pub definition: DefPath,
    /// Byte range of the failing site within its definition's source.
    pub site: (u32, u32),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Observation {
    pub schema_version: u32,
    pub returned: Option<Observed>,
    pub output: Vec<u8>,
    /// Primary failure first, then cleanup failures in order.
    pub failures: Vec<ObservedFailure>,
    /// The run is Blocked (CONC-8): it neither returned nor failed.
    pub blocked: bool,
}

impl Observation {
    /// Equal observations, refusing function-valued returns as in schema v1.
    pub fn equivalent(&self, other: &Self) -> bool {
        fn comparable(o: &Observed) -> bool {
            match o {
                Observed::NonComparableFunction => false,
                Observed::Aggregate(fields) | Observed::Variant { fields, .. } => {
                    fields.iter().flatten().all(comparable)
                }
                Observed::Block(inner) => comparable(inner),
                _ => true,
            }
        }
        self.returned.as_ref().is_none_or(comparable)
            && other.returned.as_ref().is_none_or(comparable)
            && self == other
    }
}
