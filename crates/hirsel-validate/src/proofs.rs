//! V's own memo of structural property proofs, per definition and body.
//!
//! A proof is a pure function of the session's rows, the body's generic
//! declarations and the signature views V reads: its expansion budget is the
//! proof's own (`NominalExpansions` is local to each call), and the views are
//! fixed for the whole validation. So a body asks each (type, query) once.
//! The memo is V's: it shares nothing with inference or with any other
//! definition's validation, and a hit re-reads nothing because `Reader`
//! already recorded every view the first proof read (its reads are a set).
//! An error is kept and returned again, so each caller reports it at its own
//! site through `proof_failure`, as it would have without the memo.
//!
//! Borrowed from rustc's cached type-property queries (`is_copy_raw`, keyed
//! by the type and its typing environment: rustc_middle/src/queries.rs:1683,
//! asked through rustc_middle/src/ty/util.rs:193); differs because the key is
//! a session `TypeId` plus a body, whose generic declarations are fixed for
//! the validation, and the cache dies with it.
use super::*;
use hirsel_types::{GenericDeclarations, Sendability, StructuralProperty};

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(super) enum Proof {
    /// `structural_property` with Plain, Copyable or Sendable.
    Plain,
    Copyable,
    Sendable,
    Flat,
    Detaches,
    Equality(hirsel_types::Bound),
    Sendability,
    Content,
    AfterSharing,
}

pub(super) type Proofs = HashMap<(TypeId, Proof), Result<Sendability, QueryError>>;

fn verdict(proven: bool) -> Sendability {
    if proven {
        Sendability::Sendable
    } else {
        Sendability::NotSendable
    }
}

impl Checker<'_, '_> {
    /// The answer to `query` about `root`, as `hirsel_types` proves it; a
    /// boolean property answers Sendable for true. Memoized when the proof
    /// reads this body's own declarations in the session's store.
    pub(super) fn proof(
        &self,
        store: &TypeInterner,
        root: TypeId,
        query: Proof,
        generics: &GenericDeclarations,
    ) -> Result<Sendability, QueryError> {
        if !std::ptr::eq(store, self.t) || !std::ptr::eq(generics, self.generic_declarations()) {
            return self.prove(store, root, query, generics);
        }
        let proofs = &self.scratch.proofs[self.site.body.0 as usize];
        if let Some(answer) = proofs.borrow().get(&(root, query)) {
            return answer.clone();
        }
        let answer = self.prove(store, root, query, generics);
        proofs.borrow_mut().insert((root, query), answer.clone());
        answer
    }
    /// `ty`'s canonical Plain row, or `ty` itself when its graph has none.
    /// It reads no generic declaration, so one memo serves the definition.
    pub(super) fn canonical_plain(&self, store: &TypeInterner, ty: TypeId) -> TypeId {
        let compute = || {
            hirsel_types::canonical_plain_type(store, ty, &mut |d| {
                Ok(self.signatures.get(d).cloned().map(std::sync::Arc::new))
            })
            .unwrap_or(ty)
        };
        if !std::ptr::eq(store, self.t) {
            return compute();
        }
        if let Some(&canonical) = self.scratch.canonical.borrow().get(&ty) {
            return canonical;
        }
        let canonical = compute();
        self.scratch.canonical.borrow_mut().insert(ty, canonical);
        canonical
    }
    /// `proof` for a boolean property.
    pub(super) fn holds(
        &self,
        store: &TypeInterner,
        root: TypeId,
        query: Proof,
        generics: &GenericDeclarations,
    ) -> Result<bool, QueryError> {
        self.proof(store, root, query, generics)
            .map(|answer| answer == Sendability::Sendable)
    }
    /// `proof` for a boolean property, false (and reported) on a tooling limit.
    pub(super) fn proven(
        &self,
        store: &TypeInterner,
        root: TypeId,
        query: Proof,
        generics: &GenericDeclarations,
    ) -> bool {
        self.holds(store, root, query, generics)
            .unwrap_or_else(|error| self.proof_failure(error, false))
    }
    fn prove(
        &self,
        store: &TypeInterner,
        root: TypeId,
        query: Proof,
        generics: &GenericDeclarations,
    ) -> Result<Sendability, QueryError> {
        let mut lookup = |d: DefId| {
            Ok::<_, QueryError>(self.signatures.get(d).cloned().map(std::sync::Arc::new))
        };
        match query {
            Proof::Plain | Proof::Copyable | Proof::Sendable => {
                let property = match query {
                    Proof::Plain => StructuralProperty::Plain,
                    Proof::Copyable => StructuralProperty::Copyable,
                    _ => StructuralProperty::Sendable,
                };
                hirsel_types::structural_property(store, root, property, generics, &mut lookup)
                    .map(verdict)
            }
            Proof::Flat => {
                hirsel_types::structural_flat(store, root, generics, &mut lookup).map(verdict)
            }
            Proof::Detaches => {
                hirsel_types::copy_detaches(store, root, generics, &mut lookup).map(verdict)
            }
            Proof::Equality(bound) => {
                hirsel_types::structural_equality(store, root, bound, generics, &mut lookup)
                    .map(verdict)
            }
            Proof::Sendability => {
                hirsel_types::structural_sendability(store, root, generics, &mut lookup)
            }
            Proof::Content => hirsel_types::content_sendability(store, root, generics, &mut lookup),
            Proof::AfterSharing => {
                hirsel_types::sendability_after_sharing(store, root, generics, &mut lookup)
            }
        }
    }
}
