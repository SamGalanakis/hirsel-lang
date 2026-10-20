//! Exact generic declaration lookup. The reference selects the declaration;
//! the value's location, current function and transport history do not.
use crate::*;
use hirsel_intern::work::{self, Counter};
use std::{collections::BTreeMap, sync::Arc};

/// Shared source declarations with a separate lexical signature overlay.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct GenericDeclarations {
    body: Arc<BTreeMap<SiteRef, Arc<[GenericParameter]>>>,
    permissions: Arc<BTreeMap<SiteRef, Arc<[GenericParameter]>>>,
    bound: Option<Arc<BoundDeclarations>>,
    depth: i64,
    pub lexical: Option<LexicalContext>,
}
#[derive(Debug, Eq, PartialEq)]
struct BoundDeclarations {
    position: i64,
    parameters: Arc<[GenericParameter]>,
    outer: Option<Arc<BoundDeclarations>>,
}
impl GenericDeclarations {
    pub fn signature(parameters: &[GenericParameter]) -> Self {
        work::count_by(
            Counter::CopiedGenericDeclarationRows,
            parameters.len() as u64,
        );
        Self::from_binders([(GenericBinder::Bound(0), parameters.to_vec())])
    }
    /// Construct exact source/signature binders. Empty declarations need no row.
    pub fn from_binders(
        binders: impl IntoIterator<Item = (GenericBinder, Vec<GenericParameter>)>,
    ) -> Self {
        let mut body = BTreeMap::new();
        let mut bound = BTreeMap::new();
        for (binder, parameters) in binders {
            match binder {
                GenericBinder::Body(scope) => {
                    body.insert(scope, Arc::from(parameters));
                }
                GenericBinder::Bound(depth) => {
                    bound.insert(depth, Arc::from(parameters));
                }
            }
        }
        body.retain(|_, parameters: &mut Arc<[GenericParameter]>| !parameters.is_empty());
        let mut head = None;
        for (depth, parameters) in bound.into_iter().rev() {
            let parameters: Arc<[GenericParameter]> = parameters;
            if !parameters.is_empty() {
                head = Some(Arc::new(BoundDeclarations {
                    position: -i64::from(depth),
                    parameters,
                    outer: head,
                }));
            }
        }
        Self {
            body: Arc::new(body),
            bound: head,
            ..Self::default()
        }
    }
    pub fn insert(&mut self, scope: SiteRef, parameters: Vec<GenericParameter>) {
        if self.permissions.contains_key(&scope) {
            if Arc::strong_count(&self.permissions) > 1 {
                work::count_by(
                    Counter::CopiedGenericDeclarationRows,
                    self.permissions.len() as u64,
                );
            }
            Arc::make_mut(&mut self.permissions).remove(&scope);
        }
        if parameters.is_empty() && !self.body.contains_key(&scope) {
            self.remove_lexical_parameters(scope);
            return;
        }
        if Arc::strong_count(&self.body) > 1 {
            work::count_by(
                Counter::CopiedGenericDeclarationRows,
                self.body.len() as u64,
            );
        }
        let body = Arc::make_mut(&mut self.body);
        if parameters.is_empty() {
            body.remove(&scope);
            self.remove_lexical_parameters(scope);
        } else {
            body.insert(scope, parameters.into());
        }
    }
    fn remove_lexical_parameters(&mut self, scope: SiteRef) {
        if let Some(context) = &mut self.lexical {
            if context.declarations.parameters.contains_key(&scope) {
                if Arc::strong_count(&context.declarations) > 1 {
                    work::count_by(
                        Counter::CopiedGenericDeclarationRows,
                        context.declarations.parameters.len() as u64,
                    );
                }
                Arc::make_mut(&mut context.declarations)
                    .parameters
                    .remove(&scope);
            }
        }
    }
    pub fn get<P: GenericOrdinal>(&self, reference: BinderRef<P>) -> Option<&GenericParameter> {
        self.parameters(reference.binder())?
            .get(reference.parameter().ordinal() as usize)
            .filter(|p| P::matches(p))
    }
    pub fn bounds(&self, reference: TypeParameterRef) -> &[Bound] {
        match self.get(reference) {
            Some(GenericParameter::Type { bounds, .. }) => bounds,
            _ => &[],
        }
    }
    /// Additional permissions are exact references, never a replacement generic namespace.
    pub fn with_copyable(&self, parameters: &std::collections::BTreeSet<TypeParameterRef>) -> Self {
        let mut out = self.clone();
        for p in parameters {
            if !matches!(out.get(*p), Some(GenericParameter::Type { bounds, .. }) if !bounds.contains(&Bound::Copyable))
            {
                continue;
            }
            let declaration = out.parameters(p.binder()).unwrap();
            work::count_by(
                Counter::CopiedGenericDeclarationRows,
                declaration.len() as u64,
            );
            let mut declaration = declaration.to_vec();
            if let GenericParameter::Type { bounds, .. } =
                &mut declaration[p.parameter().0 as usize]
            {
                bounds.push(Bound::Copyable);
            }
            match p.binder() {
                GenericBinder::Body(scope) => {
                    if Arc::strong_count(&out.permissions) > 1 {
                        work::count_by(
                            Counter::CopiedGenericDeclarationRows,
                            out.permissions.len() as u64,
                        );
                    }
                    Arc::make_mut(&mut out.permissions).insert(scope, declaration.into());
                }
                GenericBinder::Bound(depth) => {
                    let position = out.depth - i64::from(depth);
                    let mut prefix = vec![];
                    let mut layer = out.bound.as_ref();
                    while let Some(current) = layer {
                        if current.position == position {
                            let mut head = Some(Arc::new(BoundDeclarations {
                                position,
                                parameters: declaration.into(),
                                outer: current.outer.clone(),
                            }));
                            work::count_by(
                                Counter::CopiedGenericDeclarationRows,
                                prefix.len() as u64,
                            );
                            for current in prefix.into_iter().rev() {
                                let current: &Arc<BoundDeclarations> = current;
                                head = Some(Arc::new(BoundDeclarations {
                                    position: current.position,
                                    parameters: current.parameters.clone(),
                                    outer: head,
                                }));
                            }
                            out.bound = head;
                            break;
                        }
                        prefix.push(current);
                        layer = current.outer.as_ref();
                    }
                }
            }
        }
        out
    }
    /// Every callable shifts signature depths, including an empty binder.
    pub fn nested(&self, parameters: &[GenericParameter]) -> Self {
        let depth = self.depth + 1;
        let bound = if parameters.is_empty() {
            self.bound.clone()
        } else {
            work::count_by(
                Counter::CopiedGenericDeclarationRows,
                parameters.len() as u64,
            );
            Some(Arc::new(BoundDeclarations {
                position: depth,
                parameters: Arc::from(parameters),
                outer: self.bound.clone(),
            }))
        };
        Self {
            body: self.body.clone(),
            permissions: self.permissions.clone(),
            bound,
            depth,
            lexical: self.lexical.clone(),
        }
    }
}
pub trait GenericOrdinal: Copy {
    fn ordinal(self) -> u32;
    fn matches(parameter: &GenericParameter) -> bool;
}
impl GenericOrdinal for TypeParamId {
    fn ordinal(self) -> u32 {
        self.0
    }
    fn matches(parameter: &GenericParameter) -> bool {
        matches!(parameter, GenericParameter::Type { .. })
    }
}
impl GenericOrdinal for SizeParamId {
    fn ordinal(self) -> u32 {
        self.0
    }
    fn matches(parameter: &GenericParameter) -> bool {
        matches!(parameter, GenericParameter::Size { .. })
    }
}

/// Check every generic reference in a reachable type graph and its summaries.
/// Entering a callable always introduces a binder, including nongeneric ones.
pub fn generic_references_valid(
    store: &TypeInterner,
    root: TypeId,
    declarations: &GenericDeclarations,
) -> bool {
    let mut pending = vec![(root, declarations.clone(), false)];
    let mut active = std::collections::HashSet::new();
    let mut seen = Vec::new();
    while let Some((id, declarations, leaving)) = pending.pop() {
        if leaving {
            active.remove(&id);
            continue;
        }
        if !active.insert(id) {
            return false;
        }
        if seen.contains(&(id, declarations.clone())) {
            active.remove(&id);
            continue;
        }
        seen.push((id, declarations.clone()));
        pending.push((id, declarations.clone(), true));
        let Some(row) = store.get(id) else {
            return false;
        };
        if matches!(row.kind, TypeKind::Parameter(p) if declarations.get(p).is_none())
            || matches!(&row.drop_allocation, DropAllocation::DependsOn(ps) if ps.iter().any(|p| declarations.get(*p).is_none()))
            || matches!(&row.modes.sendability, Sendability::DependsOn(ps) if ps.iter().any(|p| declarations.get(*p).is_none()))
            || row
                .erased_captures
                .as_ref()
                .is_some_and(|c| !generic_authority_valid(&c.authority(), &declarations))
        {
            return false;
        }
        if let TypeKind::Named { arguments, .. } | TypeKind::Builtin { arguments, .. } = &row.kind {
            if arguments.iter().any(|a| matches!(a, GenericArgument::Size(Size::Parameter(p)) if declarations.get(*p).is_none())) { return false; }
        }
        let child_declarations = if let TypeKind::Function(f) = &row.kind {
            let nested = declarations.nested(&f.generics);
            if !generic_authority_valid(&f.authority, &nested)
                || matches!(&f.uses, Uses::Only(a) if !generic_authority_valid(a, &nested))
            {
                return false;
            }
            nested
        } else {
            declarations
        };
        pending.extend(
            row.kind
                .graph_edges()
                .into_iter()
                .map(|edge| (edge.ty, child_declarations.clone(), false)),
        );
    }
    true
}

pub fn generic_authority_valid(
    summary: &AuthoritySummary,
    declarations: &GenericDeclarations,
) -> bool {
    match summary {
        AuthoritySummary::Known(terms) => terms.iter().all(
            |t| !matches!(t, AuthorityTerm::TypeParameter(p) if declarations.get(*p).is_none()),
        ),
        AuthoritySummary::Unknown => true,
    }
}

/// One lexical declaration tree, shared by every body's constant-size view.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LexicalDeclarations {
    parameters: BTreeMap<SiteRef, (Vec<usize>, std::sync::Arc<[GenericParameter]>)>,
    intervals: Vec<(usize, usize)>,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LexicalContext {
    declarations: std::sync::Arc<LexicalDeclarations>,
    scope: usize,
}
impl GenericDeclarations {
    /// Build ancestry intervals once. Body scope identities may coincide only
    /// when both declarations are empty; an ambiguous generic binder is invalid.
    pub fn lexical(
        scopes: Vec<(SiteRef, Option<usize>, std::sync::Arc<[GenericParameter]>)>,
    ) -> Option<Vec<Self>> {
        let mut children = vec![vec![]; scopes.len()];
        let mut roots = vec![];
        let mut parameters: BTreeMap<SiteRef, (Vec<usize>, std::sync::Arc<[GenericParameter]>)> =
            BTreeMap::new();
        for (index, (name, parent, declaration)) in scopes.iter().enumerate() {
            if let Some((owners, previous)) = parameters.get_mut(name) {
                if (!previous.is_empty() || !declaration.is_empty())
                    && (previous != declaration
                        || parent.is_some()
                        || owners.iter().any(|owner| scopes[*owner].1.is_some()))
                {
                    return None;
                }
                owners.push(index);
            } else {
                parameters.insert(*name, (vec![index], declaration.clone()));
            }
            if let Some(parent) = parent {
                children.get_mut(*parent)?.push(index);
            } else {
                roots.push(index);
            }
        }
        let mut intervals = vec![(usize::MAX, 0); scopes.len()];
        let mut clock = 0;
        let mut pending: Vec<_> = roots.into_iter().rev().map(|i| (i, false)).collect();
        while let Some((index, exit)) = pending.pop() {
            if exit {
                intervals[index].1 = clock;
                continue;
            }
            if intervals[index].0 != usize::MAX {
                return None;
            }
            intervals[index].0 = clock;
            clock += 1;
            pending.push((index, true));
            pending.extend(children[index].iter().rev().map(|i| (*i, false)));
        }
        if clock != scopes.len() {
            return None;
        }
        parameters.retain(|_, (_, parameters)| !parameters.is_empty());
        let declarations = std::sync::Arc::new(LexicalDeclarations {
            parameters,
            intervals,
        });
        Some(
            (0..scopes.len())
                .map(|scope| Self {
                    lexical: Some(LexicalContext {
                        declarations: declarations.clone(),
                        scope,
                    }),
                    ..Self::default()
                })
                .collect(),
        )
    }
    fn parameters(&self, binder: GenericBinder) -> Option<&[GenericParameter]> {
        let name = match binder {
            GenericBinder::Bound(depth) => {
                let position = self.depth - i64::from(depth);
                let mut layer = self.bound.as_ref();
                while let Some(current) = layer {
                    if current.position == position {
                        return Some(&current.parameters);
                    }
                    if current.position < position {
                        return None;
                    }
                    layer = current.outer.as_ref();
                }
                return None;
            }
            GenericBinder::Body(name) => name,
        };
        if let Some(parameters) = self.permissions.get(&name).or_else(|| self.body.get(&name)) {
            return Some(parameters);
        }
        let context = self.lexical.as_ref()?;
        let (owners, parameters) = context.declarations.parameters.get(&name)?;
        let current = context.declarations.intervals[context.scope].0;
        owners
            .iter()
            .any(|owner| {
                let (start, end) = context.declarations.intervals[*owner];
                start <= current && current < end
            })
            .then_some(parameters)
    }
}

#[cfg(test)]
mod lexical_tests {
    use super::*;
    pub(super) fn site(owner: &str, site: NameSite) -> SiteRef {
        LexicalOwner::first(&hirsel_syntax::DefPath::core(owner)).site(site)
    }
    #[test]
    fn nested_generic_scope_views_retain_one_declaration_tree() {
        for count in [32, 128] {
            let parameter: std::sync::Arc<[GenericParameter]> = vec![GenericParameter::Type {
                name: "T".into(),
                bounds: vec![Bound::Plain].into(),
            }]
            .into();
            let mut scopes = vec![(site("f", NameSite::Item), None, parameter)];
            scopes.extend((1..=count).map(|index| {
                (
                    site(
                        "f",
                        NameSite::Expression(hirsel_syntax::ExprId(index as u32)),
                    ),
                    Some(index - 1),
                    std::sync::Arc::from([]),
                )
            }));
            let declarations = GenericDeclarations::lexical(scopes).unwrap();
            let owner = TypeParameterRef::body(site("f", NameSite::Item), TypeParamId(0));
            for view in &declarations {
                assert_eq!(view.bounds(owner), &[Bound::Plain]);
                assert!(std::sync::Arc::ptr_eq(
                    &view.lexical.as_ref().unwrap().declarations,
                    &declarations[0].lexical.as_ref().unwrap().declarations
                ));
            }
            let tree = &declarations[0].lexical.as_ref().unwrap().declarations;
            assert_eq!(tree.intervals.len(), count + 1);
            assert_eq!(
                tree.parameters
                    .values()
                    .map(|(_, parameters)| parameters.len())
                    .sum::<usize>(),
                1
            );
        }
    }
}

#[cfg(test)]
mod sharing_tests {
    use super::lexical_tests::site;
    use super::*;
    fn parameter(name: &str) -> GenericParameter {
        GenericParameter::Type {
            name: name.into(),
            bounds: vec![Bound::Plain].into(),
        }
    }
    #[test]
    fn proof_permissions_preserve_source_declarations_and_exact_binder_identity() {
        let scope = site("f", NameSite::Item);
        let other = site("g", NameSite::Item);
        let original = GenericDeclarations::from_binders([
            (GenericBinder::Body(scope), vec![parameter("Body")]),
            (GenericBinder::Body(other), vec![parameter("Other")]),
            (GenericBinder::Bound(1), vec![parameter("Outer")]),
        ]);
        let body = TypeParameterRef::body(scope, TypeParamId(0));
        let bound = TypeParameterRef::Bound {
            depth: 1,
            parameter: TypeParamId(0),
        };
        let copyable = original.with_copyable(&[body, bound].into());
        assert_eq!(original.bounds(body), &[Bound::Plain]);
        assert_eq!(original.bounds(bound), &[Bound::Plain]);
        assert_eq!(copyable.bounds(body), &[Bound::Plain, Bound::Copyable]);
        assert_eq!(copyable.bounds(bound), &[Bound::Plain, Bound::Copyable]);
        assert_eq!(
            copyable.bounds(TypeParameterRef::body(other, TypeParamId(0))),
            &[Bound::Plain]
        );
        assert!(Arc::ptr_eq(&original.body, &copyable.body));
        let nested = copyable.nested(&[]);
        assert_eq!(
            nested.bounds(TypeParameterRef::Bound {
                depth: 2,
                parameter: TypeParamId(0)
            }),
            &[Bound::Plain, Bound::Copyable]
        );
        assert_eq!(nested.bounds(body), &[Bound::Plain, Bound::Copyable]);
    }
    #[test]
    fn many_nonempty_body_binders_do_not_amplify_proof_copies() {
        let declarations = GenericDeclarations::from_binders((0..1600).map(|i| {
            (
                GenericBinder::Body(site("f", NameSite::Expression(hirsel_syntax::ExprId(i)))),
                vec![parameter("T")],
            )
        }));
        let before = work::read(Counter::CopiedGenericDeclarationRows);
        for i in 0..1600 {
            let reference = TypeParameterRef::body(
                site("f", NameSite::Expression(hirsel_syntax::ExprId(i))),
                TypeParamId(0),
            );
            let proof = declarations
                .clone()
                .nested(&[])
                .with_copyable(&[reference].into());
            assert_eq!(proof.bounds(reference), &[Bound::Plain, Bound::Copyable]);
            assert_eq!(declarations.bounds(reference), &[Bound::Plain]);
        }
        assert!(work::read(Counter::CopiedGenericDeclarationRows) - before <= 2 * 1600);
    }
    #[test]
    fn lexical_empty_overrides_are_isolated_and_duplicate_scopes_still_validate() {
        let scope = site("f", NameSite::Item);
        let child = site("f", NameSite::Expression(hirsel_syntax::ExprId(1)));
        let declared: Arc<[GenericParameter]> = vec![parameter("T")].into();
        let mut views = GenericDeclarations::lexical(vec![
            (scope, None, declared.clone()),
            (child, Some(0), Arc::from([])),
        ])
        .unwrap();
        let reference = TypeParameterRef::body(scope, TypeParamId(0));
        views[1].insert(scope, vec![]);
        assert!(views[1].get(reference).is_none());
        assert!(views[0].get(reference).is_some());
        assert!(
            GenericDeclarations::lexical(vec![
                (scope, None, declared.clone()),
                (scope, Some(0), declared),
            ])
            .is_none()
        );
        let views = GenericDeclarations::lexical(vec![
            (scope, None, Arc::from([])),
            (scope, Some(0), Arc::from([])),
        ])
        .unwrap();
        assert!(
            views[0]
                .lexical
                .as_ref()
                .unwrap()
                .declarations
                .parameters
                .is_empty()
        );
    }
}
