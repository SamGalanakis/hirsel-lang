//! Borrowed source decisions, indexed once and shared by all builders of a definition.
//! Matching rows retain producer order. Indexing does not validate unused rows;
//! each consumer keeps its original first/unique-row and error-site policy.
use super::*;
use hirsel_syntax::PatternId;
use hirsel_types::*;
use std::collections::BTreeMap;
pub(super) struct Table<'a, K, V> {
    rows: BTreeMap<K, Vec<&'a V>>,
}
impl<'a, K: Ord, V> Table<'a, K, V> {
    pub fn new(values: &'a [V], key: impl Fn(&V) -> K) -> Self {
        let mut rows: BTreeMap<K, Vec<&'a V>> = BTreeMap::new();
        for value in values {
            work::count(Counter::LoweringDecisionRows);
            rows.entry(key(value)).or_default().push(value);
        }
        Self { rows }
    }
    pub fn get(&self, key: &K) -> &[&'a V] {
        work::count(Counter::LoweringDecisionRows);
        self.rows.get(key).map_or(&[], Vec::as_slice)
    }
}
pub(super) struct Decisions<'a> {
    pub cloned_scopes: std::cell::RefCell<BTreeMap<NameSite, NameSite>>,
    pub resource_patterns: Table<'a, PatternId, (PatternId, Vec<TypeId>)>,
    pub literals:
        Table<'a, hirsel_syntax::PatternId, (hirsel_syntax::PatternId, hirsel_syntax::Literal)>,
    pub entries: Table<'a, NameSite, (NameSite, ExprId, hirsel_diag::RuleId)>,
    pub contracts: Table<'a, NameSite, hirsel_check_protocol::body::ContractPlan>,
    pub facts: Table<'a, ExprId, hirsel_check_protocol::body::FactPlan>,
    pub iterations: Table<'a, StmtId, (StmtId, RangePlan)>,
    pub protocols: Table<'a, StmtId, (StmtId, hirsel_check_protocol::body::ForExecution)>,
    pub cursors: Table<'a, StmtId, hirsel_types::CursorPlan>,

    pub repeats: Table<'a, ExprId, hirsel_syntax::RepeatLiteral>,
    pub expressions: Table<'a, ExprId, Elaboration>,
    pub patterns: Table<'a, PatternId, PatternDecision>,
    pub places: Table<'a, ExprId, SourcePlace>,
    pub bindings: Table<'a, NameSite, BindingDecision>,
    pub owners: Table<'a, NameSite, BindingDecision>,
    pub names: Table<'a, ExprId, (ExprId, ResolvedName)>,
    pub contextual: Table<'a, NameSite, (NameSite, ContextualName)>,
    pub closures: Table<'a, ExprId, ClosureInfo>,
    pub calls: Table<'a, ExprId, CheckedCall>,
    pub instantiations: Table<'a, ExprId, Instantiation>,
    pub tails: Table<'a, ExprId, CheckedTail>,
    pub results: Table<'a, NameSite, (NameSite, TypeId)>,
    pub constants: Table<'a, ExprId, (ExprId, hirsel_tir::Constant)>,
    pub materialized: Table<'a, ExprId, (ExprId, super::Materialized)>,
    pub shapes: Table<'a, TypeExprId, (TypeExprId, TypeId)>,
    pub propagation: Table<'a, ExprId, (ExprId, super::TryPlan)>,
    pub physical_projections: Table<'a, ExprId, (ExprId, TypeId)>,
    pub unique: Table<'a, ExprId, UniqueDecision>,
    pub replacements: Table<'a, ExprId, (ExprId, TypeId, Option<bool>)>,
    pub captures: Table<'a, (ExprId, u32), CaptureMode>,
    pub arguments: Table<'a, (ExprId, ExprId), (ExprId, ExprId, UseKind)>,
    pub guards: Table<'a, PatternId, GuardedPatternUse>,
    pub fields: Table<'a, NameSite, (NameSite, NameSite, Origin, Vec<(FieldId, TypeId)>)>,
    pub joins: Table<'a, NameSite, (NameSite, NameSite, Origin)>,
}
impl<'a> Decisions<'a> {
    pub fn new(input: &Input<'a>) -> Self {
        let checked = input.checked;
        Self {
            cloned_scopes: Default::default(),
            repeats: Table::new(&input.tree.repeats, |row| row.sequence),
            resource_patterns: Table::new(&input.plans.resource_patterns, |row| row.0),
            literals: Table::new(&input.plans.pattern_literals, |row| row.0),
            entries: Table::new(&input.plans.entry_checks, |row| row.0),
            contracts: Table::new(&input.plans.contracts, |row| row.owner),
            facts: Table::new(&input.plans.facts, |row| row.expression),
            iterations: Table::new(&input.plans.iterations, |row| row.0),
            protocols: Table::new(&input.plans.protocols, |row| row.0),
            cursors: Table::new(&input.plans.cursors, |row| row.statement),
            expressions: Table::new(&checked.expressions, |row| row.expression),
            patterns: Table::new(&checked.patterns, |row| row.pattern),
            places: Table::new(&checked.places, |row| row.expression),
            bindings: Table::new(&checked.bindings, |row| row.site),
            owners: Table::new(&checked.bindings, |row| row.owner),
            names: Table::new(&checked.resolution.expressions, |row| row.0),
            contextual: Table::new(&checked.resolution.contextual, |row| row.0),
            closures: Table::new(&checked.closures, |row| row.expression),
            calls: Table::new(&checked.calls, |row| row.expression),
            instantiations: Table::new(&checked.instantiations, |row| row.expression),
            tails: Table::new(&checked.tails, |row| row.call),
            results: Table::new(input.result_types, |row| row.0),
            constants: Table::new(&input.plans.constants, |row| row.0),
            materialized: Table::new(&input.plans.materialized, |row| row.0),
            shapes: Table::new(&input.plans.shapes, |row| row.0),
            propagation: Table::new(&input.plans.propagation, |row| row.0),
            physical_projections: Table::new(&input.ownership.physical_projections, |row| row.0),
            unique: Table::new(&input.ownership.unique, |row| row.expression),
            replacements: Table::new(&input.ownership.replacements, |row| row.0),
            captures: Table::new(&input.ownership.captures, |row| {
                (row.expression, row.ordinal)
            }),
            arguments: Table::new(&input.ownership.call_arguments, |row| (row.0, row.1)),
            guards: Table::new(&input.ownership.guarded_patterns, |row| row.pattern),
            fields: Table::new(&input.ownership.identity_fields, |row| row.0),
            joins: Table::new(&input.ownership.identity_joins, |row| row.0),
        }
    }
    pub fn formal_result(&self, owner: &NameSite, types: &TypeStore) -> super::Result<TypeId> {
        let mut rows = self.results.get(owner).iter().copied();
        let Some((_, result)) = rows.next() else {
            return Err(LowerError {
                site: Span::default(),
                message: "missing J formal frame result".into(),
            });
        };
        if rows.next().is_some() || types.get(*result).is_none() {
            return Err(LowerError {
                site: Span::default(),
                message: "invalid J formal frame result".into(),
            });
        }
        Ok(*result)
    }
}
