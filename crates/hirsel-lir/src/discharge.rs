//! PS-13 check removal plans: per source definition, the TIR sites whose
//! checks accepted evidence licenses SSA import to leave out.
//! `validate::discharge` confirms that a discharged import removed exactly
//! these.
use hirsel_syntax::DefPath;
use std::collections::{BTreeMap, BTreeSet};

/// How a licensed site imports.
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub enum Form {
    /// A TIR `Check` of `rule`: no `Inst::Check` and no failure edge.
    Check(String),
    /// A SEM-1 integer operation: a wrapping `IntArith` instead of
    /// the failing `Binary` or `Unary`.
    Integer,
    /// Signed integer negation: a wrapping subtraction from zero.
    Negation,
    /// Bounds or tag guards of this exact canonical TIR place at this site.
    Projection { place: u32, rule: String },
}

/// One licensed site, at its TIR `(body, block, statement)`: the statement is
/// the instruction index, or the block's instruction count for its
/// terminator, as `SourceAnchor`s are numbered.
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct Discharged {
    pub body: u32,
    pub block: u32,
    pub statement: u32,
    pub form: Form,
}

/// The plan of one build, keyed by the TIR definition's path. Empty means
/// every check stays.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Discharge(pub BTreeMap<DefPath, BTreeSet<Discharged>>);

impl Discharge {
    pub fn sites(&self, definition: &DefPath) -> Option<&BTreeSet<Discharged>> {
        self.0.get(definition).filter(|s| !s.is_empty())
    }
    pub fn is_empty(&self) -> bool {
        self.0.values().all(BTreeSet::is_empty)
    }
}
