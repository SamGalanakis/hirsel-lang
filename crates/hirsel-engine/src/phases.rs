//! Front phases outside the engine's dependency set: signature normalization,
//! body checking, modes and lowering. hirsel-driver implements [`Phases`] once;
//! the engine reaches those crates only through this trait object, so none of
//! their code is instantiated here.
use crate::*;
use hirsel_check_protocol::header::{Input, NormalizedDeclaration};
use hirsel_span::Span;
use hirsel_types::{NameSite, TypeId};
use std::{cell::RefCell, rc::Rc, sync::Arc};

/// H's complete definition-local result. Every TypeId belongs to checked.types.
#[derive(Clone, Debug)]
pub struct TypedBody {
    pub file: Arc<SyntaxFile>,
    pub item: ItemId,
    pub checked: CheckedBody,
    pub plans: hirsel_check_protocol::body::Plans,
    pub unit: TypeId,
    pub boolean: TypeId,
}
/// J/I's complete result, refined together and cached as one artifact.
#[derive(Clone, Debug)]
pub struct AnalyzedBody {
    pub typed: TypedBody,
    pub ownership: hirsel_types::OwnershipFacts,
    pub results: Vec<(NameSite, TypeId)>,
    pub call_allocations: Vec<(hirsel_syntax::ExprId, hirsel_types::Allocation)>,
    /// Evaluator-owned graphs stay separate until lowering checks and imports them.
    pub materialized: Vec<(hirsel_syntax::ExprId, hirsel_tir::Materialized)>,
}

/// The compiler's front phases. [`PhaseAdapters::compiler`] installs them beside
/// the engine's own lexer, parser, resolver, formatter and interpreter adapters.
pub trait Phases {
    /// Normalizes one written header and its private defining shape (LANG-6).
    fn normalize_signature(
        &self,
        input: Input<'_>,
        provider: &mut dyn SignatureProvider,
    ) -> Result<NormalizedDeclaration, QueryError>;
    /// Validates a `loop_source` association even when no loop uses its type.
    fn validate_loop_source(
        &self,
        signature: &Signature,
        span: Span,
        provider: &mut dyn SignatureProvider,
    ) -> Result<(), QueryError>;
    /// Validates a `format` association even when no f-string formats its type.
    fn validate_format(
        &self,
        signature: &Signature,
        span: Span,
        provider: &mut dyn SignatureProvider,
    ) -> Result<(), QueryError>;
    fn checker(&self) -> Rc<dyn CheckerPhase>;
    /// Modes record their diagnostics in the session's shared compiler state.
    fn modes(&self, state: Rc<RefCell<CompilerState>>) -> Rc<dyn ModesPhase>;
    fn lowering(&self) -> Rc<dyn LoweringPhase>;
}

/// A compiler without front phases: the engine's own adapters, for unit tests
/// that only parse, resolve or read compiler state.
#[cfg(test)]
pub(crate) fn engine_only_adapters() -> PhaseAdapters {
    struct Absent;
    impl Phases for Absent {
        fn normalize_signature(
            &self,
            _: Input<'_>,
            _: &mut dyn SignatureProvider,
        ) -> Result<NormalizedDeclaration, QueryError> {
            Err(QueryError::NotImplemented)
        }
        fn validate_loop_source(
            &self,
            _: &Signature,
            _: Span,
            _: &mut dyn SignatureProvider,
        ) -> Result<(), QueryError> {
            Err(QueryError::NotImplemented)
        }
        fn validate_format(
            &self,
            _: &Signature,
            _: Span,
            _: &mut dyn SignatureProvider,
        ) -> Result<(), QueryError> {
            Err(QueryError::NotImplemented)
        }
        fn checker(&self) -> Rc<dyn CheckerPhase> {
            unreachable!("engine_only_adapters installs no checker")
        }
        fn modes(&self, _: Rc<RefCell<CompilerState>>) -> Rc<dyn ModesPhase> {
            unreachable!("engine_only_adapters installs no modes")
        }
        fn lowering(&self) -> Rc<dyn LoweringPhase> {
            unreachable!("engine_only_adapters installs no lowering")
        }
    }
    let state = Rc::new(RefCell::new(CompilerState::default()));
    let phases: Rc<dyn Phases> = Rc::new(Absent);
    PhaseAdapters {
        lexer: Some(Rc::new(LexerAdapter)),
        parser: Some(Rc::new(ParserAdapter)),
        resolver: Some(Rc::new(ResolverAdapter {
            state: state.clone(),
        })),
        signature: Some(Rc::new(SignatureAdapter {
            state: state.clone(),
            phases: phases.clone(),
        })),
        compiler_state: Some(state),
        formatter: Some(Rc::new(FormatterAdapter)),
        phases: Some(phases),
        ..Default::default()
    }
}
