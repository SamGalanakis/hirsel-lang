//! Strict local interner backing, using the Arc/Vec layouts already visited
//! by generics.rs and hashbrown capacity bounds. Shared payload occurrences
//! are charged separately, without a temporary identity set or lazy row reads.
use super::*;
use std::alloc::Layout;

#[derive(Default)]
struct Bytes(u64);
impl Bytes {
    fn add(&mut self, n: usize) {
        self.0 = self.0.saturating_add(n as u64);
    }
    fn arc<T: ?Sized>(&mut self, value: &T) {
        self.add(
            Layout::new::<[usize; 2]>()
                .extend(Layout::for_value(value))
                .expect("existing Arc allocation")
                .0
                .pad_to_align()
                .size(),
        );
    }
    fn shared<T>(&mut self, values: &SharedSlice<T>, mut visit: impl FnMut(&mut Self, &T)) {
        if values.storage_identity() != 0 {
            self.arc(values.as_slice());
            for value in values {
                visit(self, value);
            }
        }
    }
    fn flat<T>(&mut self, values: &SharedSlice<T>) {
        self.shared(values, |_, _| ());
    }
    fn fields<T>(&mut self, mut path: &SharedPath<T>) {
        while path.node_identity().is_some() {
            self.add(path.node_allocation_bytes());
            path = path.prefix().expect("nonempty path has prefix");
        }
    }
    fn path(&mut self, value: &DefPath) {
        let hirsel_syntax::ItemPath {
            package,
            modules,
            name,
        } = &value.0;
        self.add(package.name.capacity());
        self.add(package.version.capacity());
        self.add(name.capacity());
        self.add(modules.capacity().saturating_mul(size_of::<String>()));
        for module in modules {
            self.add(module.capacity());
        }
    }
    fn root(&mut self, value: &OriginRoot) {
        match value {
            OriginRoot::Definition(path) => self.path(path),
            OriginRoot::ArgumentBrand(_)
            | OriginRoot::Expression(..)
            | OriginRoot::Parameter(_)
            | OriginRoot::Local(..)
            | OriginRoot::Sibling(_) => (),
        }
    }
    fn origin(&mut self, value: &Origin) {
        self.root(&value.root);
        self.fields(&value.fields);
    }
    fn authority(&mut self, value: &AuthoritySummary) {
        if let AuthoritySummary::Known(terms) = value {
            self.shared(terms, |w, term| match term {
                AuthorityTerm::Capability(path) => w.path(path),
                AuthorityTerm::Parameter(_) | AuthorityTerm::TypeParameter(_) => (),
            });
        }
    }
    fn generic(&mut self, value: &GenericParameter) {
        match value {
            GenericParameter::Type { name, bounds } => {
                self.add(name.capacity());
                self.flat(bounds);
            }
            GenericParameter::Size { name } => self.add(name.capacity()),
        }
    }
    fn function(&mut self, value: &FunctionType) {
        let FunctionType {
            state_promises,
            generics,
            parameters,
            result: _,
            error: _,
            allocation: _,
            authority,
            uses,
            contracts,
            incomplete: _,
            foreign,
        } = value;
        self.shared(state_promises, |w, value| {
            w.fields(&value.selector.fields);
            w.flat(&value.unretained);
        });
        self.shared(generics, Self::generic);
        self.shared(parameters, |w, value| {
            for name in [&value.name, &value.label].into_iter().flatten() {
                w.add(name.capacity());
            }
        });
        self.authority(authority);
        if let Uses::Only(value) = uses {
            self.authority(value);
        }
        self.flat(contracts);
        if let Some(value) = foreign {
            self.add(value.abi.capacity());
            self.add(value.aliasing_contract.capacity());
        }
    }
    fn row(&mut self, value: &Type) {
        let Type {
            sendability_requirement: _,
            allocation_target: _,
            drop_allocation,
            kind,
            modes,
            erased_captures,
            brand,
        } = value;
        let Modes {
            relation,
            uniqueness: _,
            multiplicity: _,
            sendability,
        } = modes;
        // Relation is a nonempty SharedSlice when it has backing. Empty
        // SharedSlices own no allocation (slice.rs From implementations).
        if !relation.reaches().is_empty() {
            self.arc(relation.reaches());
            for reach in relation.reaches() {
                if let Reach::Place(origin) = reach {
                    self.origin(origin);
                }
            }
        }
        if let Sendability::DependsOn(values) = sendability {
            self.flat(values);
        }
        if let DropAllocation::DependsOn(values) = drop_allocation {
            self.flat(values);
        }
        self.shared(&brand.alternatives, Self::origin);
        if let Some(value) = erased_captures {
            match value {
                ErasedCaptures::Known(authority) => self.authority(authority),
                ErasedCaptures::FromArguments(values) => {
                    self.shared(values, |w, value| w.fields(&value.fields))
                }
            }
        }
        match kind {
            TypeKind::Tuple(values) => self.flat(values),
            TypeKind::Record(values) => self.flat(values),
            TypeKind::Variant(values) => self.shared(values, |w, value| w.flat(&value.fields)),
            TypeKind::Function(value) => self.function(value),
            TypeKind::Named { arguments, .. } | TypeKind::Builtin { arguments, .. } => {
                self.flat(arguments)
            }
            TypeKind::Primitive(_)
            | TypeKind::Parameter(_)
            | TypeKind::Box(_)
            | TypeKind::Error => (),
        }
    }
    fn entry(&mut self, entry: &Entry) {
        self.row(&entry.row);
        if let Some(Some(key)) = entry.actual_key.get() {
            self.arc(&**key);
        }
        if let Some(Err(error)) = entry.valid.get() {
            match error {
                GraphError::UnboundOrigin(root) => self.root(root),
                GraphError::UnboundGeneric(_)
                | GraphError::UnresolvedPlacement(_)
                | GraphError::UnboundParameter(_)
                | GraphError::MissingType(_)
                | GraphError::StructuralCycle(_)
                | GraphError::RecoveryType(_)
                | GraphError::ArgumentKind(_)
                | GraphError::MissingArgument(_)
                | GraphError::MissingAuthority(_) => (),
            }
        }
    }
    fn index(&mut self, index: &HashMap<u64, Ids, IdHasher>) {
        if index.capacity() != 0 {
            let layout = Layout::new::<(u64, Ids)>();
            self.add(
                index
                    .capacity()
                    .saturating_mul(2)
                    .saturating_mul(layout.size().saturating_add(1))
                    .saturating_add(16)
                    .saturating_add(layout.align()),
            );
        }
        for ids in index.values() {
            if let Ids::Many(ids) = ids {
                self.add(ids.capacity().saturating_mul(size_of::<TypeId>()));
            }
        }
    }
    fn positions(&mut self, index: &HashMap<TypeId, u32, IdHasher>) {
        if index.capacity() != 0 {
            let layout = Layout::new::<(TypeId, u32)>();
            self.add(
                index
                    .capacity()
                    .saturating_mul(2)
                    .saturating_mul(layout.size().saturating_add(1))
                    .saturating_add(16)
                    .saturating_add(layout.align()),
            );
        }
    }
}
impl TypeProjection {
    /// Upper bound for the projected context, before allocating copied rows.
    /// Shared row payloads are charged per occurrence, as for a live context.
    pub fn retained_bytes(&self) -> u64 {
        let mut bytes = Bytes::default();
        bytes.arc(&*self.source);
        bytes.add(self.ids.len().saturating_mul(size_of::<(TypeId, Entry)>()));
        bytes.positions(&self.positions);
        for shard in &self.shards {
            bytes.index(shard);
        }
        for id in &self.ids {
            bytes.row(self.source.get_if_loaded(*id).expect("projected local row"));
        }
        bytes.0
    }
}
impl TypeInterner {
    /// Strict backing of this local interner, including its Arc allocation,
    /// reserved append-table slots, normal-form caches and row payloads.
    /// Shared image/base backing is a separate original owner. This observes
    /// current allocations, never loads base rows or predicts future growth.
    pub fn retained_local_bytes(&self) -> u64 {
        let mut bytes = Bytes::default();
        bytes.arc(self);
        bytes.add(self.entries.allocated_bytes());
        bytes.add(self.base_normals.allocated_bytes());
        bytes.add(size_of_val(&*self.frozen.rows));
        bytes.positions(&self.frozen.positions);
        for (_, entry) in &self.frozen.rows {
            bytes.entry(entry);
        }
        for shard in &self.shards {
            bytes.index(&shard.read().unwrap_or_else(PoisonError::into_inner));
        }
        for index in 0..self.assigned.load(Ordering::Acquire) {
            if let Some(entry) = self.entries.get(index) {
                bytes.entry(entry);
            }
        }
        bytes.0
    }
}
