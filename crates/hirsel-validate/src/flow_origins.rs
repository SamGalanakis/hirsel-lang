//! Binding incarnations share the ownership flow fixed point. These facts are
//! derived from semantic introductions and storage transfers, never claimed brands.
use super::indexed::Indexed;
use super::origin_dependencies::DependencyRows;
use super::shared::Shared;
use super::*;
use hirsel_types::rows::Rows;
use hirsel_types::{Origin, OriginRoot};

/// A finite per-value summary of hidden captures, separate from explicit type
/// origins. Paths are relative to the value; indices conservatively share a cell.
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub(super) enum EnvironmentField {
    Field(u32),
    Variant(u32, u32),
    Index,
}
impl EnvironmentField {
    pub fn path<'a>(projections: impl IntoIterator<Item = &'a Projection>) -> Vec<Self> {
        projections
            .into_iter()
            .map(|p| match p {
                Projection::Field(f) => Self::Field(*f),
                Projection::VariantField { variant, field } => Self::Variant(*variant, *field),
                Projection::Index(_) => Self::Index,
            })
            .collect()
    }
}
#[derive(Clone, Default, Eq, PartialEq)]
struct CaptureDependencies {
    sharing: BTreeSet<u32>,
    retained: BTreeSet<Origin>,
    expired: BTreeSet<Origin>,
    crossing: BTreeSet<(u32, Vec<hirsel_types::OwnedTypePart>)>,
}
impl CaptureDependencies {
    fn join(&mut self, other: &Self) {
        self.sharing.extend(&other.sharing);
        self.retained.extend(other.retained.iter().cloned());
        self.expired.extend(other.expired.iter().cloned());
        self.crossing.extend(other.crossing.iter().cloned());
    }
}
#[derive(Clone, Default, Eq, PartialEq)]
pub(super) struct Environment(BTreeMap<Vec<EnvironmentField>, CaptureDependencies>);
impl Environment {
    pub fn closure(retained: BTreeSet<Origin>, expired: BTreeSet<Origin>) -> Self {
        if retained.is_empty() && expired.is_empty() {
            Self::default()
        } else {
            Self(BTreeMap::from([(
                vec![],
                CaptureDependencies {
                    sharing: BTreeSet::new(),
                    retained,
                    expired,
                    crossing: BTreeSet::new(),
                },
            )]))
        }
    }
    pub fn join(&mut self, other: &Self) {
        for (path, dependencies) in &other.0 {
            self.0.entry(path.clone()).or_default().join(dependencies);
        }
    }
    pub fn require_crossing(&mut self, place: PlaceId, path: Vec<hirsel_types::OwnedTypePart>) {
        self.0
            .entry(vec![])
            .or_default()
            .crossing
            .insert((place.0, path));
    }
    pub fn share_capture(&mut self, place: PlaceId) {
        self.0.entry(vec![]).or_default().sharing.insert(place.0);
    }
    pub fn sharing(&self) -> BTreeSet<u32> {
        self.0
            .values()
            .flat_map(|d| d.sharing.iter().copied())
            .collect()
    }
    pub fn remap_sharing(&mut self, mut map: impl FnMut(u32) -> Option<u32>) {
        for dependencies in self.0.values_mut() {
            dependencies.sharing = dependencies
                .sharing
                .iter()
                .filter_map(|p| map(*p))
                .collect();
        }
    }
    pub fn crossing(&self) -> BTreeSet<(u32, Vec<hirsel_types::OwnedTypePart>)> {
        self.0
            .values()
            .flat_map(|dependencies| dependencies.crossing.iter().cloned())
            .collect()
    }
    pub fn remap_crossing(
        &mut self,
        mut map: impl FnMut(
            u32,
            &[hirsel_types::OwnedTypePart],
        ) -> Option<(u32, Vec<hirsel_types::OwnedTypePart>)>,
    ) {
        for dependencies in self.0.values_mut() {
            dependencies.crossing = dependencies
                .crossing
                .iter()
                .filter_map(|(place, path)| map(*place, path))
                .collect();
        }
    }
    pub fn retained(&self) -> BTreeSet<Origin> {
        self.0
            .values()
            .flat_map(|d| d.retained.iter().cloned())
            .collect()
    }
    pub fn expired(&self) -> BTreeSet<Origin> {
        self.0
            .values()
            .flat_map(|d| d.expired.iter().cloned())
            .collect()
    }
    pub fn expire(&mut self, owner: &Origin) {
        for dependencies in self.0.values_mut() {
            if dependencies.retained.iter().any(|o| overlaps(owner, o)) {
                dependencies.expired.insert(owner.clone());
            }
        }
    }
    pub fn project(&self, prefix: &[EnvironmentField]) -> Self {
        let mut projected = Self::default();
        for (path, dependencies) in &self.0 {
            if path.starts_with(prefix) {
                projected
                    .0
                    .entry(path[prefix.len()..].to_vec())
                    .or_default()
                    .join(dependencies);
            } else if prefix.starts_with(path) {
                // A summarized deeper subtree still carries all its captures.
                projected.0.entry(vec![]).or_default().join(dependencies);
            }
        }
        projected
    }
    pub fn insert(
        &mut self,
        prefix: &[EnvironmentField],
        value: &Self,
        paths: &BTreeSet<Vec<EnvironmentField>>,
    ) {
        for (path, dependencies) in &value.0 {
            let mut target = prefix.to_vec();
            target.extend(path.iter().cloned());
            // Only statically distinguished paths become cells. A deeper
            // recursive subtree joins its nearest represented prefix, retaining
            // every dependency and expiration in a finite per-value lattice.
            while !paths.contains(&target) {
                target.pop();
            }
            self.0.entry(target).or_default().join(dependencies);
        }
    }
    pub fn replace(
        &mut self,
        prefix: &[EnvironmentField],
        value: &Self,
        paths: &BTreeSet<Vec<EnvironmentField>>,
    ) {
        // An Index summarizes every element, so overwriting one element cannot
        // discharge captures retained by the others.
        if !prefix.contains(&EnvironmentField::Index) {
            self.0.retain(|path, _| !path.starts_with(prefix));
        }
        self.insert(prefix, value, paths);
    }
}

/// A row's origin set. Snapshots and joins share it until a write.
pub(super) type Origins = Shared<BTreeSet<Origin>>;
/// A row's per-field-path identities. Snapshots and joins share it until a write.
pub(super) type Parts = Shared<BTreeMap<Vec<hirsel_types::FieldId>, BTreeSet<Origin>>>;

/// The union of `incoming` into `row`, reusing `incoming` itself when it
/// already holds every element of `row`.
pub(super) fn union_origins(row: &mut Origins, incoming: &Origins) {
    if row.shares(incoming) || incoming.is_subset(row) {
        return;
    }
    if row.is_subset(incoming) {
        *row = incoming.clone();
    } else {
        row.extend(incoming.iter().cloned());
    }
}

#[derive(Clone, Default, Eq, PartialEq)]
pub(super) struct OriginState {
    pub identities: Rows<Origins>,
    pub parts: Rows<Parts>,
    pub owners: Rows<Origins>,
    pub ended: Indexed<OriginRoot, bool>,
    pub dead: Indexed<OriginRoot, BTreeSet<hirsel_types::SharedPath<hirsel_types::FieldId>>>,
    pub bindings: Indexed<OriginRoot, BTreeSet<Origin>>,
    /// Current canonical storage and required access-loan ordinal for each name.
    pub locations: Indexed<OriginRoot, BTreeSet<(usize, Option<u32>)>>,
    pub retained: DependencyRows<BTreeSet<Origin>>,
    pub environments: DependencyRows<Environment>,
    pub expired: Rows<BTreeSet<Origin>>,
    pub proofs: Rows<BTreeMap<OriginRoot, BTreeSet<Origin>>>,
}
impl OriginState {
    pub fn new(places: usize) -> Self {
        Self {
            identities: Rows::new(Origins::default(), places)
                .with_copy_counter(Counter::ValidateCopiedRows),
            parts: Rows::new(Parts::default(), places)
                .with_copy_counter(Counter::ValidateCopiedRows),
            owners: Rows::new(Origins::default(), places)
                .with_copy_counter(Counter::ValidateCopiedRows),
            ended: Indexed::default(),
            dead: Indexed::default(),
            retained: DependencyRows::new(places),
            environments: DependencyRows::new(places),
            expired: Rows::new(BTreeSet::new(), places)
                .with_copy_counter(Counter::ValidateCopiedRows),
            proofs: Rows::new(BTreeMap::new(), places)
                .with_copy_counter(Counter::ValidateCopiedRows),
            bindings: BTreeMap::new().into(),
            locations: BTreeMap::new().into(),
        }
    }
    /// Join `other`, which is `base` changed in place, into a state that
    /// already joined `base` (see `State::join_since`). A part row falls back
    /// to its place's identities, so it is revisited when either changed.
    pub fn join_since(&mut self, base: &Self, other: &Self) {
        let rows: BTreeSet<_> = base
            .parts
            .differences(&other.parts)
            .into_iter()
            .chain(base.identities.differences(&other.identities))
            .collect();
        work::count_by(Counter::ValidateJoinRows, rows.len() as u64);
        for i in rows {
            let (mine, incoming) = (&self.parts[i], &other.parts[i]);
            let paths: BTreeSet<_> = mine.keys().chain(incoming.keys()).cloned().collect();
            let mut joined = BTreeMap::new();
            for path in paths {
                let fallback = |identities: &BTreeSet<Origin>| {
                    identities
                        .iter()
                        .map(|o| {
                            let mut o = o.clone();
                            o.fields.extend(&path);
                            o
                        })
                        .collect::<BTreeSet<_>>()
                };
                let mut identities = mine
                    .get(&path)
                    .cloned()
                    .unwrap_or_else(|| fallback(&self.identities[i]));
                identities.extend(
                    incoming
                        .get(&path)
                        .cloned()
                        .unwrap_or_else(|| fallback(&other.identities[i])),
                );
                joined.insert(path, identities);
            }
            // Equal rows keep their current or incoming allocation.
            if joined == **incoming {
                let incoming = incoming.clone();
                self.parts.set(i, incoming);
            } else if joined != **mine {
                self.parts.set(i, joined.into());
            }
        }
        self.ended
            .union_since(&base.ended, &other.ended, |a, b| *a |= b);
        self.dead
            .union_since(&base.dead, &other.dead, |a, b| a.extend(b.iter().cloned()));
        self.owners
            .union_since(&base.owners, &other.owners, union_origins);
        self.proofs
            .union_since(&base.proofs, &other.proofs, |a, b| {
                for (name, values) in b {
                    a.entry(name.clone())
                        .or_default()
                        .extend(values.iter().cloned());
                }
            });
        self.identities
            .union_since(&base.identities, &other.identities, union_origins);
        self.environments
            .union_since(&base.environments, &other.environments, |a, b| a.join(b));
        self.retained
            .union_since(&base.retained, &other.retained, |a, b| {
                a.extend(b.iter().cloned())
            });
        self.expired
            .union_since(&base.expired, &other.expired, |a, b| {
                a.extend(b.iter().cloned())
            });
        self.locations
            .union_since(&base.locations, &other.locations, |a, b| a.extend(b));
        self.bindings
            .union_since(&base.bindings, &other.bindings, |a, b| {
                a.extend(b.iter().cloned())
            });
    }

    /// Clearing a value must not materialize empty snapshot rows or clone a
    /// shared collection only to discard it. Presence inside parts/proofs is
    /// still significant; only the whole value is being overwritten here.
    pub fn clear_value(&mut self, index: usize) {
        if !self.identities[index].is_empty() {
            self.identities.set(index, Origins::default());
        }
        if !self.parts[index].is_empty() {
            self.parts.set(index, Parts::default());
        }
        if !self.owners[index].is_empty() {
            self.owners.set(index, Origins::default());
        }
        if !self.proofs[index].is_empty() {
            self.proofs.set(index, BTreeMap::new());
        }
        self.retained.set(index, BTreeSet::new());
        self.environments.set(index, Environment::default());
        if !self.expired[index].is_empty() {
            self.expired.set(index, BTreeSet::new());
        }
    }

    pub fn expire(&mut self, owner: &Origin) {
        self.dead
            .entry(owner.root.clone())
            .or_insert_with(BTreeSet::new)
            .insert(owner.fields.clone());
        for index in self.environments.holders(&owner.root) {
            work::count(Counter::ValidateOriginDependencyRows);
            self.environments
                .update(index, |environment| environment.expire(owner));
        }
        for index in self.retained.holders(&owner.root) {
            work::count(Counter::ValidateOriginDependencyRows);
            if self.retained[index]
                .iter()
                .any(|value| overlaps(owner, value))
            {
                self.expired[index].insert(owner.clone());
            }
        }
    }
    pub fn introduce(&mut self, owner: &Origin) {
        if owner.fields.is_empty() {
            self.dead.remove(&owner.root);
            return;
        }
        let Some(paths) = self.dead.get(&owner.root) else {
            return;
        };
        // Lexicographic field paths keep every descendant in one contiguous range.
        let replaced: Vec<_> = paths
            .range(owner.fields.clone()..)
            .take_while(|path| {
                work::count(Counter::ValidateOriginDependencyRows);
                path.starts_with(&owner.fields)
            })
            .cloned()
            .collect();
        if replaced.len() == paths.len() {
            self.dead.remove(&owner.root);
        } else if !replaced.is_empty() {
            let paths = self
                .dead
                .entry(owner.root.clone())
                .or_insert_with(BTreeSet::new);
            for path in replaced {
                paths.remove(&path);
            }
        }
    }
    pub fn expired_by(&self, retained: &BTreeSet<Origin>) -> BTreeSet<Origin> {
        let mut result = BTreeSet::new();
        for value in retained {
            if let Some(paths) = self.dead.get(&value.root) {
                for path in paths {
                    work::count(Counter::ValidateOriginDependencyRows);
                    if path.starts_with(&value.fields) || value.fields.starts_with(path) {
                        result.insert(Origin {
                            root: value.root.clone(),
                            fields: path.clone(),
                        });
                    }
                }
            }
        }
        result
    }
}

pub(super) fn contains(owner: &Origin, value: &Origin) -> bool {
    owner.root == value.root && value.fields.starts_with(&owner.fields)
}

/// Mutating a path also ends brands on its enclosing owner; siblings survive.
pub(super) fn overlaps(owner: &Origin, value: &Origin) -> bool {
    contains(owner, value) || contains(value, owner)
}

pub(super) trait OriginProof {
    fn get(&self, root: &OriginRoot) -> Option<&BTreeSet<Origin>>;
}
impl OriginProof for BTreeMap<OriginRoot, BTreeSet<Origin>> {
    fn get(&self, root: &OriginRoot) -> Option<&BTreeSet<Origin>> {
        self.get(root)
    }
}
impl OriginProof for Indexed<OriginRoot, BTreeSet<Origin>> {
    fn get(&self, root: &OriginRoot) -> Option<&BTreeSet<Origin>> {
        self.get(root)
    }
}

/// Origins are brands and arena places (`arena`); a relation place that names
/// a lending parameter is a lifetime, which `lifetimes` judges.
pub(super) fn retained(
    store: &TypeInterner,
    types: &super::origin_types::Types,
    ty: TypeId,
    proof: &impl OriginProof,
    arena: &dyn Fn(&Origin) -> bool,
) -> BTreeSet<Origin> {
    let mut retained = BTreeSet::new();
    let _ = types.visit(store, ty, arena, |origin, depth| {
        if matches!(origin.root, OriginRoot::Parameter(hirsel_types::ParameterRef::Bound { depth: d, .. }) | OriginRoot::ArgumentBrand(hirsel_types::ParameterRef::Bound { depth: d, .. }) if d < depth)
        {
            return Ok(());
        }
        retained.insert(origin.clone());
        if let Some(identities) = proof.get(&origin.root) {
            for identity in identities {
                let mut identity = identity.clone();
                identity.fields.extend(&origin.fields);
                retained.insert(identity);
            }
        }
        Ok(())
    });
    retained
}

/// Canonicalize only origins supported by this value's retained proof. Bound
/// references of nested callables remain declarations, never argument names.
pub(super) fn canonical(
    store: &TypeInterner,
    ty: TypeId,
    proof: &impl OriginProof,
    arena: &dyn Fn(&Origin) -> bool,
) -> Result<TypeId, hirsel_types::GraphError> {
    let name = |origin: &Origin, depth: u32| {
        if matches!(origin.root, OriginRoot::Definition(_))
            || matches!(origin.root, OriginRoot::Parameter(hirsel_types::ParameterRef::Bound { depth: d, .. }) | OriginRoot::ArgumentBrand(hirsel_types::ParameterRef::Bound { depth: d, .. }) if d < depth)
        {
            return Ok(origin.clone());
        }
        let Some(identities) = proof.get(&origin.root) else {
            return Err(hirsel_types::GraphError::UnboundOrigin(origin.root.clone()));
        };
        if identities.len() != 1 {
            return Err(hirsel_types::GraphError::UnboundOrigin(origin.root.clone()));
        }
        let mut identity = identities.first().unwrap().clone();
        identity.fields.extend(&origin.fields);
        Ok(identity)
    };
    let ty = hirsel_types::map_brand_origins(store, ty, name)?;
    // Origins are brands and arena places; any other relation place bounds a
    // lifetime (S2) and is not a name this proof binds.
    hirsel_types::map_relation_places(store, ty, |origin, depth| {
        if arena(origin) {
            name(origin, depth)
        } else {
            Ok(origin.clone())
        }
    })
}

/// Immutable view of the primary flow facts at call entry. Ended names are
/// excluded at lookup, exactly as in the former filtered materialization.
#[derive(Clone)]
pub(super) struct LiveNames {
    bindings: Indexed<OriginRoot, BTreeSet<Origin>>,
    ended: Indexed<OriginRoot, bool>,
}
impl LiveNames {
    pub fn new(state: &OriginState) -> Self {
        work::count(Counter::ValidateCallProofRows);
        Self {
            bindings: state.bindings.clone(),
            ended: state.ended.clone(),
        }
    }
}
impl OriginProof for LiveNames {
    fn get(&self, root: &OriginRoot) -> Option<&BTreeSet<Origin>> {
        work::count(Counter::ValidateCallProofRows);
        if self.ended.contains_key(root) {
            None
        } else {
            self.bindings.get(root)
        }
    }
}

pub(super) struct CallOriginProof {
    pub weakened: BTreeSet<u32>,
    pub current_unique: BTreeSet<u32>,
    pub current_sendable: BTreeSet<u32>,
    /// Independently owned deep-Global content behind Global receiving storage.
    pub acquired_global: BTreeSet<u32>,
    pub arguments: BTreeMap<hirsel_types::ParameterRef, hirsel_types::OriginArgument>,
    pub fields:
        BTreeMap<(hirsel_types::ParameterRef, Vec<hirsel_types::FieldId>), BTreeSet<Origin>>,
    pub names: LiveNames,
    pub proofs: BTreeMap<u32, BTreeMap<OriginRoot, BTreeSet<Origin>>>,
}

impl CallOriginProof {
    /// Flow proved this operand's current content Sendable. A mut callable or
    /// iterator view keeps its storage's fixed contract unless that contract
    /// is itself a sendable requirement.
    pub(super) fn currently_sendable(&self, value: &Operand, row: &hirsel_types::Type) -> bool {
        self.current_sendable.contains(&value.place.0)
            && (value.usage != UseKind::Mut
                || !matches!(
                    row.kind,
                    TypeKind::Function(_)
                        | TypeKind::Builtin {
                            kind: hirsel_types::BuiltinType::Iter,
                            ..
                        }
                )
                || row.sendability_requirement == hirsel_types::SendabilityRequirement::Sendable)
    }
}

/// Field selectors are resolved from the same independently derived storage
/// partition as the whole operand, before removing the callee's binder.
pub(super) fn sources(
    store: &TypeInterner,
    root: TypeId,
    origins: &CallOriginProof,
    actuals: &BTreeMap<hirsel_types::ParameterRef, TypeId>,
) -> Result<TypeId, hirsel_types::GraphError> {
    let root = hirsel_types::map_brand_origins(store, root, |origin, depth| {
        let OriginRoot::Parameter(hirsel_types::ParameterRef::Bound {
            depth: d,
            parameter,
        }) = origin.root
        else {
            return Ok(origin.clone());
        };
        if d < depth || origin.fields.is_empty() {
            return Ok(origin.clone());
        }
        let key = (
            hirsel_types::ParameterRef::Bound {
                depth: d - depth,
                parameter,
            },
            origin.fields.iter().copied().collect(),
        );
        let Some(identities) = origins.fields.get(&key) else {
            return Ok(origin.clone());
        };
        if identities.len() != 1 {
            return Err(hirsel_types::GraphError::UnboundOrigin(origin.root.clone()));
        }
        Ok(identities.first().unwrap().clone())
    })?;
    hirsel_types::instantiate_parameter_sources(store, root, &origins.arguments, actuals)
}

pub(super) fn remember(
    store: &TypeInterner,
    types: &super::origin_types::Types,
    ty: TypeId,
    names: &impl OriginProof,
    arena: &dyn Fn(&Origin) -> bool,
) -> BTreeMap<OriginRoot, BTreeSet<Origin>> {
    let mut proof = BTreeMap::new();
    let _ = types.visit(store, ty, arena, |origin, depth| {
        if !matches!(origin.root, OriginRoot::Parameter(hirsel_types::ParameterRef::Bound { depth: d, .. }) | OriginRoot::ArgumentBrand(hirsel_types::ParameterRef::Bound { depth: d, .. }) if d < depth)
        {
            if let Some(identities) = names.get(&origin.root) {
                proof.insert(origin.root.clone(), identities.clone());
            }
        }
        Ok(())
    });
    proof
}

/// DFS feedback edges break every directed cycle, including irreducible CFGs.
/// Reducible feedback expires introductions in its natural loop, stopping at
/// the dominating header. Irreducible components retain the conservative SCC
/// region. A whole
/// introduction subsumes all fields; a field introduction preserves disjoint
/// siblings. No block-number ordering stands in for CFG reachability.
pub(super) fn loop_expirations(body: &Body) -> BTreeMap<(u32, u32), BTreeSet<Origin>> {
    let graph: Vec<_> = body
        .blocks
        .iter()
        .map(|b| body.successors(&b.terminator))
        .collect();
    let mut reverse = vec![vec![]; graph.len()];
    for (a, targets) in graph.iter().enumerate() {
        for b in targets {
            reverse[b.0 as usize].push(BlockId(a as u32));
        }
    }
    let dense: Vec<Vec<usize>> = graph
        .iter()
        .map(|targets| targets.iter().map(|b| b.0 as usize).collect())
        .collect();
    let dominators = hirsel_graph::Dominators::new(&dense, body.entry.0 as usize);
    let mut irreducible = vec![false; graph.len()];
    for component in hirsel_graph::components(&dense) {
        // A dominating header must precede every member in reverse postorder.
        let header = *component
            .iter()
            .min_by_key(|&&b| dominators.order(b).unwrap_or(usize::MAX))
            .unwrap();
        if component.iter().any(|&b| !dominators.dominates(header, b)) {
            for b in component {
                irreducible[b] = true;
            }
        }
    }
    let mut color = vec![0u8; graph.len()];
    let mut stack = vec![(body.entry, 0usize)];
    color[body.entry.0 as usize] = 1;
    let mut feedback = vec![];
    while let Some((node, next)) = stack.last_mut() {
        if *next == graph[node.0 as usize].len() {
            color[node.0 as usize] = 2;
            stack.pop();
            continue;
        }
        let target = graph[node.0 as usize][*next];
        *next += 1;
        match color[target.0 as usize] {
            0 => {
                color[target.0 as usize] = 1;
                stack.push((target, 0));
            }
            1 => feedback.push((*node, target)),
            _ => (),
        }
    }
    fn reachable(start: BlockId, graph: &[Vec<BlockId>]) -> BTreeSet<u32> {
        let mut seen = BTreeSet::new();
        let mut pending = vec![start];
        while let Some(node) = pending.pop() {
            if seen.insert(node.0) {
                pending.extend(&graph[node.0 as usize]);
            }
        }
        seen
    }
    let mut result = BTreeMap::new();
    for (source, target) in feedback {
        let region: BTreeSet<u32> = if !irreducible[source.0 as usize]
            && dominators.dominates(target.0 as usize, source.0 as usize)
        {
            // LLVM GenericLoopInfoImpl.h's backward natural-loop traversal:
            // the header is already visited, so do not cross its predecessors.
            let mut region = BTreeSet::from([target.0]);
            let mut pending = vec![source];
            while let Some(node) = pending.pop() {
                if region.insert(node.0) {
                    pending.extend(
                        reverse[node.0 as usize]
                            .iter()
                            .copied()
                            .filter(|p| dominators.dominates(target.0 as usize, p.0 as usize)),
                    );
                }
            }
            region
        } else {
            let forward = reachable(target, &graph);
            let backward = reachable(source, &reverse);
            forward.intersection(&backward).copied().collect()
        };
        // A name introduced in the loop is a new incarnation each iteration
        // (PROV-1), so the backedge ends it. A name introduced before the loop
        // (a parameter, or a binding bound outside it), and each of its
        // fields, continues across it: a fresh rebinding inside ends the
        // incarnation it replaces where it happens, so the next iteration and
        // the exit see the current one (#78).
        let continuing: BTreeSet<_> = (0..body.blocks.len() as u32)
            .filter(|block| !region.contains(block))
            .flat_map(|block| fresh_bindings(body, block))
            .collect();
        let roots = region
            .iter()
            .flat_map(|block| fresh_bindings(body, *block))
            .filter(|origin| {
                !matches!(origin.root, OriginRoot::Parameter(_))
                    && !continuing.iter().any(|name| contains(name, origin))
            })
            .collect();
        result.insert((source.0, target.0), roots);
    }
    result
}
/// The names a block introduces fresh.
fn fresh_bindings(body: &Body, block: u32) -> impl Iterator<Item = Origin> + '_ {
    body.blocks[block as usize]
        .instructions
        .iter()
        .flat_map(|instruction| match &instruction.kind {
            InstructionKind::BindOrigin {
                place,
                kind: OriginBindingKind::Fresh,
                ..
            } => std::slice::from_ref(place),
            InstructionKind::BindFields { fields, .. } => fields.as_slice(),
            _ => &[],
        })
        .filter_map(|place| body.places[place.0 as usize].binding.clone())
}

/// A closure entry receives the identities retained by its creation operands.
/// Re-entering a callable does not execute a fresh outer binding introduction.
#[derive(Clone, Default, Eq, PartialEq)]
pub(super) struct CapturedOrigin {
    pub nested_shared: bool,
    pub crossing: super::flow::crossing::Parts,
    pub contract_readonly: bool,
    pub tags: super::flow::tags::Parts,
    pub environment: Environment,
    pub identities: BTreeSet<Origin>,
    pub parts: BTreeMap<Vec<hirsel_types::FieldId>, BTreeSet<Origin>>,
    pub proof: BTreeMap<OriginRoot, BTreeSet<Origin>>,
    pub expired: BTreeSet<Origin>,
    pub owners: BTreeSet<Origin>,
}
pub(super) fn join_captures(target: &mut Vec<CapturedOrigin>, incoming: &[CapturedOrigin]) -> bool {
    let old = target.clone();
    for (a, b) in target.iter_mut().zip(incoming) {
        a.crossing.join(&b.crossing);
        a.nested_shared |= b.nested_shared;
        a.contract_readonly |= b.contract_readonly;
        a.environment.join(&b.environment);
        super::flow::tags::join_parts(&mut a.tags, &b.tags);
        let paths: BTreeSet<_> = a.parts.keys().chain(b.parts.keys()).cloned().collect();
        for path in paths {
            let project = |identities: &BTreeSet<Origin>| {
                identities
                    .iter()
                    .map(|o| {
                        let mut o = o.clone();
                        o.fields.extend(&path);
                        o
                    })
                    .collect::<BTreeSet<_>>()
            };
            let mut identities = a
                .parts
                .get(&path)
                .cloned()
                .unwrap_or_else(|| project(&a.identities));
            identities.extend(
                b.parts
                    .get(&path)
                    .cloned()
                    .unwrap_or_else(|| project(&b.identities)),
            );
            a.parts.insert(path, identities);
        }
        a.identities.extend(b.identities.iter().cloned());
        a.expired.extend(b.expired.iter().cloned());
        a.owners.extend(b.owners.iter().cloned());
        for (name, values) in &b.proof {
            a.proof
                .entry(name.clone())
                .or_default()
                .extend(values.iter().cloned());
        }
    }
    *target != old
}

#[cfg(test)]
mod environment_tests {
    use super::*;
    use crate::support::lexical;
    #[test]
    fn clearing_value_facts_keeps_name_history_and_past_snapshots() {
        let origin = Origin {
            root: OriginRoot::Local(lexical(), hirsel_types::LocalId(1)),
            fields: vec![].into(),
        };
        let mut state = OriginState::new(64);
        let identities = BTreeSet::from([origin.clone()]);
        state.identities[7] = identities.clone().into();
        // Even an explicit empty part/proof entry differs from an absent one.
        state.parts[7] = BTreeMap::from([(vec![hirsel_types::FieldId(0)], BTreeSet::new())]).into();
        state.proofs[7] = BTreeMap::from([(origin.root.clone(), BTreeSet::new())]);
        state.owners[7] = identities.clone().into();
        state.retained.set(7, identities.clone());
        state
            .environments
            .set(7, Environment::closure(identities.clone(), BTreeSet::new()));
        state.bindings.insert(origin.root.clone(), identities);
        state
            .locations
            .insert(origin.root.clone(), BTreeSet::from([(7, None)]));
        state.expire(&origin);
        let before = state.clone();
        state.clear_value(7);
        assert!(state.identities[7].is_empty());
        assert!(state.parts[7].is_empty());
        assert!(state.owners[7].is_empty());
        assert!(state.proofs[7].is_empty());
        assert!(state.retained[7].is_empty());
        assert!(state.environments[7] == Environment::default());
        assert!(state.expired[7].is_empty());
        assert!(state.bindings == before.bindings);
        assert!(state.locations == before.locations);
        assert!(state.dead == before.dead);
        assert!(!before.identities[7].is_empty());
        assert!(!before.parts[7].is_empty());
        assert!(!before.proofs[7].is_empty());
        assert!(!before.expired[7].is_empty());
        assert_eq!(before.retained.holders(&origin.root), vec![7]);
        assert!(state.retained.holders(&origin.root).is_empty());
        assert!(state.environments.holders(&origin.root).is_empty());
        let before = work::read(Counter::ValidateCopiedRows);
        let mut empty = state.clone();
        empty.clear_value(7);
        empty.clear_value(31);
        assert_eq!(work::read(Counter::ValidateCopiedRows), before);
    }

    #[test]
    fn introducing_a_prefix_visits_only_its_ended_descendants() {
        let origin = |fields: &[u32]| Origin {
            root: OriginRoot::Local(lexical(), hirsel_types::LocalId(0)),
            fields: fields.iter().copied().map(hirsel_types::FieldId).collect(),
        };
        let mut state = OriginState::new(0);
        for sibling in 0..2048 {
            state.expire(&origin(&[sibling, 0]));
        }
        state.expire(&origin(&[1024, 1]));
        let before = state.clone();
        let visits = work::read(Counter::ValidateOriginDependencyRows);
        state.introduce(&origin(&[1024]));
        assert!(work::read(Counter::ValidateOriginDependencyRows) - visits <= 3);
        let paths = state.dead.get(&origin(&[]).root).unwrap();
        assert_eq!(paths.len(), 2047);
        assert!(paths.contains(&origin(&[1023, 0]).fields));
        assert!(paths.contains(&origin(&[1025, 0]).fields));
        assert!(!paths.contains(&origin(&[1024, 0]).fields));
        assert!(!paths.contains(&origin(&[1024, 1]).fields));
        assert_eq!(before.dead.get(&origin(&[]).root).unwrap().len(), 2049);
        let visits = work::read(Counter::ValidateOriginDependencyRows);
        state.introduce(&origin(&[]));
        assert_eq!(work::read(Counter::ValidateOriginDependencyRows), visits);
        assert!(state.dead.get(&origin(&[]).root).is_none());
        assert_eq!(before.dead.get(&origin(&[]).root).unwrap().len(), 2049);
    }

    #[test]
    fn expiry_preserves_siblings_snapshots_and_existing_value_expiration() {
        let origin = |root, fields: Vec<u32>| Origin {
            root: OriginRoot::Local(lexical(), hirsel_types::LocalId(root)),
            fields: fields
                .into_iter()
                .map(hirsel_types::FieldId)
                .collect::<Vec<_>>()
                .into(),
        };
        let whole = origin(1, vec![]);
        let left = origin(1, vec![0]);
        let right = origin(1, vec![1]);
        let unrelated = origin(2, vec![]);
        let mut state = OriginState::new(4);
        for (i, value) in [
            whole.clone(),
            left.clone(),
            right.clone(),
            unrelated.clone(),
        ]
        .into_iter()
        .enumerate()
        {
            state.retained.set(i, BTreeSet::from([value.clone()]));
            state.environments.set(
                i,
                Environment::closure(BTreeSet::from([value]), BTreeSet::new()),
            );
        }
        let initial = state.clone();
        state.expire(&left);
        for i in 0..4 {
            let expected = if i < 2 {
                BTreeSet::from([left.clone()])
            } else {
                BTreeSet::new()
            };
            assert_eq!(state.expired[i], expected);
            assert_eq!(state.environments[i].expired(), expected);
            assert!(initial.expired[i].is_empty());
        }
        let ended = state.clone();
        state.introduce(&right);
        assert_eq!(
            state.expired_by(&BTreeSet::from([whole.clone()])),
            BTreeSet::from([left.clone()])
        );
        state.introduce(&whole);
        assert!(
            state
                .expired_by(&BTreeSet::from([whole.clone()]))
                .is_empty()
        );
        assert_eq!(state.expired[0], BTreeSet::from([left.clone()]));
        let base = state.clone();
        state.join_since(&base, &ended);
        assert_eq!(
            state.expired_by(&BTreeSet::from([whole])),
            BTreeSet::from([left])
        );
        state.retained.set(0, BTreeSet::from([unrelated.clone()]));
        state.environments.set(0, Environment::default());
        state.expire(&right);
        assert!(state.environments[0].expired().is_empty());
        assert!(
            state.expired[0]
                .iter()
                .all(|old| old.fields != right.fields)
        );
        assert!(state.expired_by(&BTreeSet::from([unrelated])).is_empty());
    }
    #[test]
    fn recursive_storage_summaries_stabilize_without_losing_expiration() {
        let owner = Origin {
            root: OriginRoot::Local(lexical(), hirsel_types::LocalId(1)),
            fields: vec![].into(),
        };
        let paths = BTreeSet::from([
            vec![],
            vec![EnvironmentField::Field(0)],
            vec![EnvironmentField::Field(0), EnvironmentField::Field(0)],
        ]);
        let mut value = Environment::closure(BTreeSet::from([owner.clone()]), BTreeSet::new());
        for _ in 0..8 {
            let mut wrapped = Environment::default();
            wrapped.insert(&[EnvironmentField::Field(0)], &value, &paths);
            value = wrapped;
        }
        let mut wrapped = Environment::default();
        wrapped.insert(&[EnvironmentField::Field(0)], &value, &paths);
        assert!(wrapped == value);
        value.expire(&owner);
        for _ in 0..8 {
            value = value.project(&[EnvironmentField::Field(0)]);
        }
        assert_eq!(value.expired(), BTreeSet::from([owner]));
    }
    #[test]
    fn replacing_one_sequence_element_keeps_other_elements_dependencies() {
        let owner = Origin {
            root: OriginRoot::Local(lexical(), hirsel_types::LocalId(1)),
            fields: vec![].into(),
        };
        let capture = Environment::closure(BTreeSet::from([owner.clone()]), BTreeSet::new());
        let paths = BTreeSet::from([vec![], vec![EnvironmentField::Index]]);
        let mut sequence = Environment::default();
        sequence.insert(&[EnvironmentField::Index], &capture, &paths);
        sequence.replace(&[EnvironmentField::Index], &Environment::default(), &paths);
        sequence.expire(&owner);
        assert_eq!(
            sequence.project(&[EnvironmentField::Index]).expired(),
            BTreeSet::from([owner])
        );
    }
}

#[cfg(test)]
mod loop_region_tests {
    use super::*;
    use crate::support::{Fixture, block, inst, lexical};

    // Independent graph oracle: one distinct fresh owner at every node. This
    // tests expiration membership without using a producer's loan summaries.
    fn expirations(graph: &[&[u32]]) -> BTreeMap<(u32, u32), BTreeSet<u32>> {
        let mut fixture = Fixture::unit();
        let body = fixture.body();
        let base = body.places[0].clone();
        body.places = (0..graph.len())
            .map(|node| Place {
                binding: Some(Origin {
                    root: OriginRoot::Local(lexical(), hirsel_types::LocalId(node as u32)),
                    fields: vec![].into(),
                }),
                ..base.clone()
            })
            .collect();
        body.blocks = graph
            .iter()
            .enumerate()
            .map(|(node, edges)| {
                let term = match *edges {
                    [] => Terminator::Unreachable,
                    [target] => Terminator::Goto(BlockId(*target)),
                    [yes, no] => Terminator::Branch {
                        condition: Operand {
                            place: PlaceId(0),
                            usage: UseKind::Copy,
                        },
                        yes: BlockId(*yes),
                        no: BlockId(*no),
                    },
                    _ => panic!("fixture supports at most two successors"),
                };
                block(
                    vec![inst(
                        InstructionKind::BindOrigin {
                            source: PlaceId(node as u32),
                            place: PlaceId(node as u32),
                            kind: OriginBindingKind::Fresh,
                        },
                        0,
                    )],
                    term,
                )
            })
            .collect();
        loop_expirations(body)
            .into_iter()
            .map(|(edge, origins)| {
                (
                    edge,
                    origins
                        .into_iter()
                        .map(|origin| match origin.root {
                            OriginRoot::Local(_, local) => local.0,
                            _ => unreachable!(),
                        })
                        .collect(),
                )
            })
            .collect()
    }

    #[test]
    fn nested_natural_loop_expires_inner_owners_but_keeps_preheader_owners() {
        let regions = expirations(&[&[1], &[2, 6], &[3], &[4, 5], &[3], &[1], &[]]);
        assert_eq!(regions[&(4, 3)], BTreeSet::from([3, 4]));
        // The enclosing iteration still ends both its own and inner owners.
        assert_eq!(regions[&(5, 1)], BTreeSet::from([1, 2, 3, 4, 5]));
    }

    #[test]
    fn continue_outer_expires_owners_and_break_outer_does_not_add_feedback() {
        // Node 4 continues directly to outer header 1; node 5 either breaks
        // to exit 7 or takes the ordinary enclosing feedback through node 6.
        let regions = expirations(&[&[1], &[2, 7], &[3], &[4, 5], &[1], &[7, 6], &[1], &[]]);
        assert_eq!(regions[&(4, 1)], BTreeSet::from([1, 2, 3, 4]));
        assert_eq!(regions[&(6, 1)], BTreeSet::from([1, 2, 3, 5, 6]));
        assert!(!regions.contains_key(&(5, 7)));
    }

    #[test]
    fn irreducible_component_keeps_scc_expiration_even_for_its_natural_subloop() {
        // The SCC {1,2,3,4} has two entries, 1 and 2. Its internal self-loop
        // at 3 has a dominating header, but must retain the SCC fallback.
        let regions = expirations(&[&[1, 2], &[3], &[3], &[3, 4], &[1, 2]]);
        for region in regions.values() {
            assert_eq!(*region, BTreeSet::from([1, 2, 3, 4]));
        }
        assert!(regions.contains_key(&(3, 3)));
    }
}
