//! Loan sets (§04, Places and loans). A loan is a kind on a place, and each
//! carries the site that issued it, which diagnostics name as its cause.
use super::*;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub(super) enum Kind {
    /// The value is the place's own storage, read in place: an alias.
    Identity,
    /// The value lives in, or depends on content living in, the place.
    Bound,
    /// A view of the place, held while the value is live.
    Read,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub(super) enum On {
    /// A binding of this body, by its row.
    Binding(usize),
    /// A lexical scope (`LifetimeScopes`): the frame, a block, a loop
    /// iteration or the temporaries of one statement (MEM-10).
    Scope(usize),
    /// The value's own environment `env`, when it is a callable.
    Environment,
    /// A scoped placement (an arena or region) that a join lost (PROV-4).
    Placement,
}

/// Where a loan was issued.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub(super) enum Site {
    Expression(ExprId),
    Binding(usize),
    /// The frame of this body.
    Frame,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct Loan {
    pub kind: Kind,
    pub on: On,
    /// The record field path within a binding the loan is on; empty for the
    /// whole binding and for every other place (#77).
    pub within: Vec<u32>,
    pub site: Site,
}

/// At most one loan per kind and place, sorted by both; joining two loans on
/// the same place keeps the earliest issue site and the longest field path
/// both are within.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(super) struct LoanSet(Vec<Loan>);

impl LoanSet {
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
    pub fn iter(&self) -> impl Iterator<Item = &Loan> {
        self.0.iter()
    }
    pub fn insert(&mut self, kind: Kind, on: On, site: Site) {
        self.insert_within(kind, on, &[], site);
    }
    pub fn insert_within(&mut self, kind: Kind, on: On, within: &[u32], site: Site) {
        match self.0.binary_search_by(|l| (l.kind, l.on).cmp(&(kind, on))) {
            Ok(i) => {
                let loan = &mut self.0[i];
                loan.site = loan.site.min(site);
                let common = loan
                    .within
                    .iter()
                    .zip(within)
                    .take_while(|(a, b)| a == b)
                    .count();
                loan.within.truncate(common);
            }
            Err(i) => self.0.insert(
                i,
                Loan {
                    kind,
                    on,
                    within: within.to_vec(),
                    site,
                },
            ),
        }
    }
    /// The loan of `kind` on `on`, issued at `site`, or none.
    pub fn set(&mut self, kind: Kind, on: On, site: Option<Site>) {
        self.0.retain(|l| (l.kind, l.on) != (kind, on));
        if let Some(site) = site {
            self.insert(kind, on, site);
        }
    }
    pub fn join(&mut self, other: &Self) {
        for loan in &other.0 {
            self.insert_within(loan.kind, loan.on, &loan.within, loan.site);
        }
    }
    pub fn get(&self, kind: Kind, on: On) -> Option<Site> {
        self.0
            .binary_search_by(|l| (l.kind, l.on).cmp(&(kind, on)))
            .ok()
            .map(|i| self.0[i].site)
    }
    pub fn contains(&self, kind: Kind, on: On) -> bool {
        self.get(kind, on).is_some()
    }
    pub fn retain(&mut self, keep: impl FnMut(&Loan) -> bool) {
        self.0.retain(keep);
    }
    /// The bindings this set holds `kind` loans on, in row order.
    pub fn bindings(&self, kind: Kind) -> impl Iterator<Item = usize> + '_ {
        self.0.iter().filter_map(move |l| match l.on {
            On::Binding(binding) if l.kind == kind => Some(binding),
            _ => None,
        })
    }
    /// The scopes this set holds loans on: bounds first, then views.
    pub fn scopes(&self) -> impl Iterator<Item = (Kind, usize, Site)> + '_ {
        self.0.iter().filter_map(|l| match l.on {
            On::Scope(scope) => Some((l.kind, scope, l.site)),
            _ => None,
        })
    }
}
