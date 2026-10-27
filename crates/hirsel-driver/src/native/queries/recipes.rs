//! Source-instance recipes contain typed actuals and lexical owner keys, never
//! cached Values or source bodies. Decode is reserved for an instance miss.
use hirsel_engine::{NativeRecordSource, NativeRecords};
use hirsel_layout::{instance::InstanceKey, layout::LayoutBuilder};
use hirsel_rc::instance::{
    Actuals, DiscoveryContext, InstanceId, InstanceRole, Instances, Program, discover_instance,
};
use hirsel_syntax::DefPath;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, HashMap, HashSet};

/// Bodyless original instance inputs used to re-plan adapter declarations on
/// both misses and hits. Key validation binds every actual and lexical binder;
/// the current GEN-5 catalogue, not native discovery output, selects references.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) struct InstancePlan {
    actuals: Actuals,
}
impl InstancePlan {
    pub fn capture(instances: &Instances, id: InstanceId) -> Result<Self, String> {
        Ok(Self {
            actuals: NativeKeyTypes::new(&instances.types)
                .actuals(&instances.actuals[id.index()])?,
        })
    }
    pub fn register(
        &self,
        program: &Program,
        context: &DiscoveryContext,
        key: &InstanceKey,
        library: bool,
        layouts: &mut LayoutBuilder<'_>,
        instances: &mut Instances,
    ) -> Result<(), String> {
        let (convention, environment) = key
            .identity_shape()
            .map_err(|e| format!("native original key shape: {e:?}"))?;
        let selected = hirsel_rc::instance::key_of(
            &program.types,
            key.definition(),
            hirsel_tir::BodyId(key.body()),
            &self.actuals,
            convention.as_ref(),
            environment,
        )
        .map_err(|e| format!("native original plan key: {e:?}"))?;
        if &selected != key {
            return Err("native original plan actuals differ from selected key".into());
        }
        let requests = if library {
            if let Some(primitive) = hirsel_lir::pretransfer::primitive(key.definition()) {
                let request = hirsel_rc::library::keyed::request(
                    program,
                    &primitive,
                    self.actuals.arguments.clone(),
                )
                .map_err(|e| format!("native adapter declaration: {e:?}"))?
                .ok_or("missing native adapter declaration")?;
                if request
                    .key(&program.types, layouts)
                    .map_err(|e| format!("native adapter declaration key: {e:?}"))?
                    != *key
                {
                    return Err("native adapter declaration differs from original plan".into());
                }
                vec![request]
            } else {
                Vec::new()
            }
        } else {
            hirsel_rc::library::keyed::source_requests(
                program,
                context,
                key.definition(),
                &self.actuals,
            )
            .map_err(|e| format!("native source adapter requirements: {e:?}"))?
        };
        for adapter in requests {
            let primitive = hirsel_rc::library::keyed::primitive_request(
                program,
                &adapter.path,
                adapter.actuals.arguments.clone(),
            )
            .map_err(|e| format!("native primitive declaration: {e:?}"))?
            .ok_or("missing native primitive declaration")?;
            for request in [adapter, primitive] {
                hirsel_rc::library::instantiate(layouts, instances, request)
                    .map_err(|e| format!("native original adapter plan: {e:?}"))?;
            }
        }
        Ok(())
    }
}

/// Native actuals share an instance across relation, brand, uniqueness and
/// sendability. Persist the same representative at every depth so independent
/// discoveries of that key register the same recipe and original plan.
struct NativeKeyTypes<'a> {
    types: &'a hirsel_types::TypeInterner,
    done: HashMap<hirsel_types::TypeId, hirsel_types::TypeId>,
}
impl<'a> NativeKeyTypes<'a> {
    fn new(types: &'a hirsel_types::TypeInterner) -> Self {
        Self {
            types,
            done: HashMap::new(),
        }
    }
    fn actuals(&mut self, actuals: &Actuals) -> Result<Actuals, String> {
        Ok(Actuals {
            binder: actuals.binder,
            arguments: self.arguments(&actuals.arguments)?,
            outer: actuals
                .outer
                .iter()
                .map(|(site, arguments)| Ok((*site, self.arguments(arguments)?)))
                .collect::<Result<_, String>>()?,
        })
    }
    fn arguments(
        &mut self,
        arguments: &[hirsel_types::GenericArgument],
    ) -> Result<Vec<hirsel_types::GenericArgument>, String> {
        arguments
            .iter()
            .map(|argument| match argument {
                hirsel_types::GenericArgument::Type(ty) => {
                    Ok(hirsel_types::GenericArgument::Type(self.ty(*ty)?))
                }
                other => Ok(other.clone()),
            })
            .collect()
    }
    fn ty(&mut self, root: hirsel_types::TypeId) -> Result<hirsel_types::TypeId, String> {
        let mut entered = HashSet::new();
        let mut pending = vec![(root, false)];
        while let Some((id, exit)) = pending.pop() {
            if self.done.contains_key(&id) {
                continue;
            }
            let row = self
                .types
                .get(id)
                .ok_or("native recipe actual is not a type row")?;
            if !exit {
                if !entered.insert(id) {
                    return Err("native recipe actual is a cyclic type graph".into());
                }
                pending.push((id, true));
                pending.extend(row.kind.graph_edges().into_iter().map(|e| (e.ty, false)));
                continue;
            }
            let mut row = row.clone();
            row.modes.relation = hirsel_types::Relation::global();
            row.modes.uniqueness = hirsel_types::Uniqueness::Aliased;
            row.modes.sendability = hirsel_types::Sendability::NotSendable;
            row.sendability_requirement = hirsel_types::SendabilityRequirement::None;
            row.brand = hirsel_types::Brand::default();
            row.kind = row.kind.map_edges(|child| self.done[&child]);
            let canonical = self.types.intern(row);
            self.done.insert(id, canonical);
        }
        Ok(self.done[&root])
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Step {
    body: u32,
    encoding: Vec<u8>,
}
impl Step {
    fn of(key: &InstanceKey) -> Self {
        Self {
            body: key.body(),
            encoding: key.encoding().to_vec(),
        }
    }
    fn key(&self, definition: &DefPath) -> Result<InstanceKey, String> {
        InstanceKey::from_encoding(definition, self.body, self.encoding.clone())
            .map_err(|e| format!("source recipe key: {e:?}"))
    }
}

/// Ordinary discovery metadata for a source function/finalizer or a lexical
/// closure. Parent metadata can be reconstructed on a closure miss without
/// constructing or decoding any ancestor Value. Library requests are separate.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SourceRecipe {
    schema: u32,
    definition: DefPath,
    actuals: Actuals,
    finalizer: bool,
    steps: Vec<Step>,
}
impl SourceRecipe {
    pub fn capture(instances: &Instances, selected: InstanceId) -> Result<Self, String> {
        let definition = instances.view(selected).key.definition().clone();
        let mut steps = Vec::new();
        let mut visited = BTreeSet::new();
        let mut current = selected;
        let finalizer = loop {
            if !visited.insert(current) || instances.view(current).key.definition() != &definition {
                return Err("cyclic or mixed source recipe ancestry".into());
            }
            steps.push(Step::of(instances.lexical_key(current)));
            match instances.roles[current.index()] {
                InstanceRole::Function => break false,
                InstanceRole::Finalizer => break true,
                InstanceRole::Closure { parent } => current = parent,
                InstanceRole::Library { .. } => {
                    return Err("library request is not a source recipe".into());
                }
            }
        };
        steps.reverse();
        Ok(Self {
            schema: 1,
            definition,
            actuals: NativeKeyTypes::new(&instances.types)
                .actuals(&instances.actuals[current.index()])?,
            finalizer,
            steps,
        })
    }
    pub fn key(&self) -> Result<InstanceKey, String> {
        self.steps
            .last()
            .ok_or("empty source recipe")?
            .key(&self.definition)
    }
    pub fn encode(
        &self,
        records: &NativeRecords,
        engine: &impl NativeRecordSource,
    ) -> Result<Vec<u8>, String> {
        records
            .encode(engine, self)
            .ok_or_else(|| "source recipe encoding failed".into())
    }
    pub fn decode(
        records: &NativeRecords,
        engine: &impl NativeRecordSource,
        bytes: &[u8],
    ) -> Result<Self, String> {
        let recipe: Self = records
            .decode(engine, bytes)
            .ok_or("source recipe decoding failed")?;
        if recipe.schema != 1
            || recipe.steps.is_empty()
            || recipe.steps.len() > hirsel_rc::instance::MAX_INSTANCES
        {
            return Err("source recipe schema or ancestry limit differs".into());
        }
        let mut seen = BTreeSet::new();
        for step in &recipe.steps {
            step.key(&recipe.definition)?;
            if !seen.insert(&step.encoding) {
                return Err("repeated source recipe ancestor".into());
            }
        }
        Ok(recipe)
    }

    /// The caller first obtains the actual current V-owned original Definition
    /// and captures any independent source inventory before native discovery.
    /// Ordinary recipe bytes confer no source or native validation authority.
    pub fn restore(
        &self,
        program: &Program,
        context: &DiscoveryContext,
        original: hirsel_tir::DefinitionRef,
        layouts: &mut LayoutBuilder<'_>,
    ) -> Result<(Instances, InstanceId), String> {
        self.restore_selected(program, context, original, None, layouts)
    }

    /// Restore the lexical recipe, then bind the current independently selected
    /// request. Full profiles stay with the instance until its import completes.
    pub fn restore_selected(
        &self,
        program: &Program,
        context: &DiscoveryContext,
        original: hirsel_tir::DefinitionRef,
        selected: Option<&hirsel_rc::sealed_source::contract::registry::Request>,
        layouts: &mut LayoutBuilder<'_>,
    ) -> Result<(Instances, InstanceId), String> {
        let source = context
            .sources
            .get(&self.definition)
            .ok_or("missing source recipe declaration")?;
        let current = hirsel_rc::instance::SourceDeclaration::of(&original)
            .map_err(|e| format!("source recipe original contracts: {e:?}"))?;
        if source.entry != current.entry
            || source.entry_parameters != current.entry_parameters
            || source.declaration != current.declaration
        {
            return Err("source recipe contracts differ from current original source".into());
        }
        let root = self
            .steps
            .first()
            .ok_or("empty source recipe")?
            .key(&self.definition)?;
        let mut instances = Instances::new(program.types.clone(), program.memo.clone());
        let (mut current, _) = if self.finalizer {
            instances.register_finalizer(program, source, self.actuals.clone(), layouts)
        } else {
            instances.register_function(
                program,
                source.declaration.clone(),
                source.entry,
                self.actuals.clone(),
                layouts,
            )
        }
        .map_err(|e| format!("source recipe registration: {e:?}"))?;
        if instances.view(current).key != root {
            return Err("source recipe actuals differ from its root key".into());
        }
        let mut expanded = BTreeSet::new();
        for (position, step) in self.steps.iter().enumerate() {
            let expected = step.key(&self.definition)?;
            if instances.view(current).key != expected {
                return Err("source recipe lexical instance differs".into());
            }
            // Reconstruct only original lexical discovery metadata. No import,
            // Value materialization or native pass is performed for ancestors.
            if expanded.insert(current) {
                (instances, _) = discover_instance(
                    program,
                    context,
                    layouts,
                    instances,
                    current,
                    original.clone(),
                )
                .map_err(|e| format!("source recipe discovery: {e:?}"))?;
            }
            if let Some(next) = self.steps.get(position + 1) {
                let key = next.key(&self.definition)?;
                // As in instance::expand, a generic nested function may be
                // instantiated by a coercion wrapper rather than its lexical
                // parent. Follow only already discovered closures of this
                // original source; this opens no other Definition or Value.
                while instances.get(&key).is_none() {
                    let sibling = instances.ids().find(|id| {
                        !expanded.contains(id)
                            && instances.view(*id).key.definition() == &self.definition
                            && matches!(instances.roles[id.index()], InstanceRole::Closure { .. })
                    });
                    let Some(sibling) = sibling else {
                        break;
                    };
                    expanded.insert(sibling);
                    (instances, _) = discover_instance(
                        program,
                        context,
                        layouts,
                        instances,
                        sibling,
                        original.clone(),
                    )
                    .map_err(|e| format!("source recipe lexical discovery: {e:?}"))?;
                }
                let child = instances
                    .get(&key)
                    .ok_or("source recipe closure no longer discovered")?;
                if instances.roles[child.index()] != (InstanceRole::Closure { parent: current }) {
                    return Err("source recipe closure has a different lexical parent".into());
                }
                current = child;
            }
        }
        if let Some(selected) = selected {
            instances
                .bind_receipt_request(current, selected, layouts)
                .map_err(|e| format!("source recipe receipt request: {e:?}"))?;
        }
        Ok((instances, current))
    }
}

/// Canonical callable metadata for a library recipe, independent of a
/// program-local ABI row or any import-effects journal.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
enum RecipeShape {
    Definition,
    Closure,
    IterNext,
    IterDrop,
    Foreign,
}
impl From<hirsel_lir::abi_types::CallableShape> for RecipeShape {
    fn from(shape: hirsel_lir::abi_types::CallableShape) -> Self {
        use hirsel_lir::abi_types::CallableShape as S;
        match shape {
            S::Definition => Self::Definition,
            S::Closure => Self::Closure,
            S::IterNext => Self::IterNext,
            S::IterDrop => Self::IterDrop,
            S::Foreign => Self::Foreign,
        }
    }
}
impl From<RecipeShape> for hirsel_lir::abi_types::CallableShape {
    fn from(shape: RecipeShape) -> Self {
        match shape {
            RecipeShape::Definition => Self::Definition,
            RecipeShape::Closure => Self::Closure,
            RecipeShape::IterNext => Self::IterNext,
            RecipeShape::IterDrop => Self::IterDrop,
            RecipeShape::Foreign => Self::Foreign,
        }
    }
}

/// One native library request, with the captured environment named by content
/// rather than a requiring unit's transient LayoutId. This is discovery data;
/// the query owner must accept the requiring product and current read witnesses
/// before decoding it. Restore builds no source or library Value.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LibraryRecipe {
    schema: u32,
    definition: DefPath,
    encoding: Vec<u8>,
    body: u32,
    actuals: Actuals,
    callable: hirsel_types::TypeId,
    shape: RecipeShape,
    environment: Option<hirsel_syntax::ContentHash>,
    finalizer: bool,
}
impl LibraryRecipe {
    pub fn capture(
        instances: &Instances,
        id: InstanceId,
        table: &hirsel_lir::LayoutTable,
    ) -> Result<Self, String> {
        let InstanceRole::Library { body, finalizer } = instances.roles[id.index()] else {
            return Err("source instance is not a library recipe".into());
        };
        let view = instances.view(id);
        let mut sendless = NativeKeyTypes::new(&instances.types);
        Ok(Self {
            schema: 1,
            definition: view.key.definition().clone(),
            encoding: view.key.encoding().to_vec(),
            body: u32::try_from(body).map_err(|_| "library recipe body limit")?,
            actuals: sendless.actuals(&instances.actuals[id.index()])?,
            callable: sendless.ty(instances.callables[id.index()])?,
            shape: view.owner.into(),
            environment: instances.environments[id.index()]
                .map(|layout| hirsel_lir::LayoutKeys::rooted(table, layout).0),
            finalizer,
        })
    }
    pub fn key(&self) -> Result<InstanceKey, String> {
        InstanceKey::from_encoding(&self.definition, 0, self.encoding.clone())
            .map_err(|e| format!("library recipe key: {e:?}"))
    }
    pub fn encode(
        &self,
        records: &NativeRecords,
        engine: &impl NativeRecordSource,
    ) -> Result<Vec<u8>, String> {
        records
            .encode(engine, self)
            .ok_or_else(|| "library recipe encoding failed".into())
    }
    pub fn decode(
        records: &NativeRecords,
        engine: &impl NativeRecordSource,
        bytes: &[u8],
    ) -> Result<Self, String> {
        let recipe: Self = records
            .decode(engine, bytes)
            .ok_or("library recipe decoding failed")?;
        if recipe.schema != 1 {
            return Err("library recipe schema differs".into());
        }
        recipe.key()?;
        Ok(recipe)
    }
    pub fn restore(
        &self,
        program: &Program,
        layouts: &mut LayoutBuilder<'_>,
    ) -> Result<(Instances, InstanceId), String> {
        let body = hirsel_rc::library::BODIES
            .get(self.body as usize)
            .ok_or("library recipe body no longer registered")?;
        let declared = (body.path)();
        if declared != self.definition
            && (declared != hirsel_rc::library::native_path("operation_value")
                || !program.operations.contains_key(&self.definition)
                || !hirsel_rc::library::value::forwards(&self.definition))
        {
            return Err("library recipe body differs from selected implementation".into());
        }
        let keys = hirsel_lir::LayoutKeys::new(layouts.table());
        let environment = self
            .environment
            .map(|key| {
                keys.all()
                    .iter()
                    .position(|candidate| candidate.0 == key)
                    .map(|position| hirsel_lir::LayoutId(position as u32))
                    .ok_or("library recipe environment is not in its reached layout products")
            })
            .transpose()?;
        let receipt = if self.definition.is_core("arena.seal") {
            hirsel_rc::library::Request::operation(
                program,
                self.body as usize,
                &self.definition,
                self.actuals.arguments.clone(),
            )
            .map_err(|e| format!("reviewed library receipt: {e:?}"))?
            .receipt
        } else {
            None
        };
        let request = hirsel_rc::library::Request {
            receipt,
            body: self.body as usize,
            path: self.definition.clone(),
            actuals: self.actuals.clone(),
            callable: self.callable,
            shape: self.shape.into(),
            environment,
            finalizer: self.finalizer,
        };
        if request
            .key(&program.types, layouts)
            .map_err(|e| format!("library recipe actuals: {e:?}"))?
            != self.key()?
        {
            return Err("library recipe actuals differ from selected instance key".into());
        }
        let mut instances = Instances::new(program.types.clone(), program.memo.clone());
        let id = hirsel_rc::library::instantiate(layouts, &mut instances, request)
            .map_err(|e| format!("library recipe registration: {e:?}"))?;
        Ok((instances, id))
    }
}
