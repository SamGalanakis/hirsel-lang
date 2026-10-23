//! Source-instance recipes contain typed actuals and lexical owner keys, never
//! cached Values or source bodies. Decode is reserved for an instance miss.
use hirsel_engine::{NativeRecords, QueryEngine};
use hirsel_layout::{instance::InstanceKey, layout::LayoutBuilder};
use hirsel_rc::instance::{
    Actuals, DiscoveryContext, InstanceId, InstanceRole, Instances, Program, discover_instance,
};
use hirsel_syntax::DefPath;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

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
            steps.push(Step::of(&instances.view(current).key));
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
            actuals: instances.actuals[current.index()].clone(),
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
    pub fn encode(&self, records: &NativeRecords, engine: &QueryEngine) -> Result<Vec<u8>, String> {
        records
            .encode(engine, self)
            .ok_or_else(|| "source recipe encoding failed".into())
    }
    pub fn decode(
        records: &NativeRecords,
        engine: &QueryEngine,
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
        let source = context
            .sources
            .get(&self.definition)
            .ok_or("missing source recipe declaration")?;
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
        for (position, step) in self.steps.iter().enumerate() {
            let expected = step.key(&self.definition)?;
            if instances.view(current).key != expected {
                return Err("source recipe lexical instance differs".into());
            }
            // Reconstruct only original lexical discovery metadata. No import,
            // Value materialization or native pass is performed for ancestors.
            (instances, _) = discover_instance(
                program,
                context,
                layouts,
                instances,
                current,
                original.clone(),
            )
            .map_err(|e| format!("source recipe discovery: {e:?}"))?;
            if let Some(next) = self.steps.get(position + 1) {
                let key = next.key(&self.definition)?;
                let child = instances
                    .get(&key)
                    .ok_or("source recipe closure no longer discovered")?;
                if instances.roles[child.index()] != (InstanceRole::Closure { parent: current }) {
                    return Err("source recipe closure has a different lexical parent".into());
                }
                current = child;
            }
        }
        Ok((instances, current))
    }
}
