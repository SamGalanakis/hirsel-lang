//! Portable requests for the sole callable rewrite. These records are data;
//! the consumer still admits the selected original and validates its result.
use super::{CallableRequest, CallableTarget};
use hirsel_layout::instance::InstanceKey;
use hirsel_lir::{
    LayoutKeys,
    ir::{Symbol, SymbolKind},
    layout_types::LayoutTable,
    products::Binding,
};
use hirsel_syntax::{ContentHash, DefPath};
use serde::{Deserialize, Serialize};

/// Exact existing key encoding from closure_calls.rs. ABI content identities
/// are query reads, not a change to the callable instance's established name.
pub(super) fn specialization_key<'a>(
    source: &InstanceKey,
    targets: impl IntoIterator<Item = (usize, &'a DefPath, Option<ContentHash>, bool)>,
    flags: u64,
) -> InstanceKey {
    let mut encoded = Vec::new();
    for (parameter, definition, instance, environment) in targets {
        encoded.extend((parameter as u64).to_le_bytes());
        encoded.extend(definition.fingerprint().0);
        encoded.push(u8::from(instance.is_some()));
        encoded.extend(instance.unwrap_or_default().0);
        encoded.push(u8::from(environment));
    }
    if source.definition().is_core("task.async") {
        encoded.extend(flags.to_le_bytes());
    }
    source.derived(
        "hirsel.callable-target-specialization.v1",
        &[encoded.as_slice()],
    )
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Seed {
    parameter: u32,
    definition: DefPath,
    instance: Option<ContentHash>,
    abi: ContentHash,
    environment: bool,
}

/// The existing task_main capture specialization, distinct from parameter
/// seeds. This is request data; the consumer still admits original C and ABI.
#[derive(Clone, Debug)]
pub struct TaskCaptureRequest {
    pub key: InstanceKey,
    pub target: CallableTarget,
}
fn task_capture_key(
    source: &InstanceKey,
    definition: &DefPath,
    instance: Option<ContentHash>,
    environment: bool,
) -> InstanceKey {
    source.derived(
        "hirsel.task-capture-target-specialization.v1",
        &[
            &definition.fingerprint().0,
            &instance.unwrap_or_default().0,
            &[u8::from(environment)],
        ],
    )
}
impl TaskCaptureRequest {
    pub fn new(source: &InstanceKey, target: CallableTarget) -> Result<Self, String> {
        if !source.definition().is_core("$native.task_main")
            || target.symbol.kind != SymbolKind::Function
        {
            return Err(
                "task capture requires the original task_main and a function target".into(),
            );
        }
        let key = task_capture_key(
            source,
            &target.symbol.definition,
            target.symbol.instance,
            target.environment,
        );
        Ok(Self { key, target })
    }
}

/// Canonical portable capture binding. It retains neither an IR body nor a
/// numeric ABI selector, and grants no source or native admission authority.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskCaptureRecipe {
    schema: u32,
    definition: DefPath,
    body: u32,
    source: Vec<u8>,
    target: Seed,
}
impl TaskCaptureRecipe {
    pub fn capture(
        source: &InstanceKey,
        request: &TaskCaptureRequest,
        table: &LayoutTable,
    ) -> Result<Self, String> {
        let target = &request.target;
        let recipe = Self {
            schema: 1,
            definition: source.definition().clone(),
            body: source.body(),
            source: source.encoding().to_vec(),
            target: Seed {
                parameter: 0,
                definition: target.symbol.definition.clone(),
                instance: target.symbol.instance,
                abi: LayoutKeys::new(table)
                    .fn_abi_of(table, target.abi)
                    .ok_or("missing task capture target ABI")?,
                environment: target.environment,
            },
        };
        if target.symbol.kind != SymbolKind::Function || recipe.key()? != request.key {
            return Err("task capture recipe differs from selected request".into());
        }
        Ok(recipe)
    }
    pub fn source(&self) -> Result<InstanceKey, String> {
        if self.schema != 1
            || self.target.parameter != 0
            || !self.definition.is_core("$native.task_main")
        {
            return Err("noncanonical task capture recipe".into());
        }
        InstanceKey::from_encoding(&self.definition, self.body, self.source.clone())
            .map_err(|error| format!("task capture source key: {error:?}"))
    }
    fn target(&self, abi: hirsel_lir::FnAbiId) -> CallableTarget {
        CallableTarget {
            symbol: Symbol {
                kind: SymbolKind::Function,
                definition: self.target.definition.clone(),
                instance: self.target.instance,
            },
            abi,
            environment: self.target.environment,
        }
    }
    pub fn key(&self) -> Result<InstanceKey, String> {
        Ok(task_capture_key(
            &self.source()?,
            &self.target.definition,
            self.target.instance,
            self.target.environment,
        ))
    }
    pub fn bind(
        &self,
        expected: &InstanceKey,
        binding: &Binding,
    ) -> Result<TaskCaptureRequest, String> {
        let key = self.key()?;
        if &key != expected {
            return Err("task capture recipe has another requested owner".into());
        }
        let abi = binding
            .abi(self.target.abi)
            .ok_or("missing task capture target ABI")?;
        Ok(TaskCaptureRequest {
            key,
            target: self.target(abi),
        })
    }
}

/// No FnAbiId, TypeId, source body or Value is retained. The full original key
/// and canonical ABI hashes survive process/table rebinding. A recipe alone
/// grants neither source proof nor C authority nor admission under the caps.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CallableRecipe {
    schema: u32,
    definition: DefPath,
    body: u32,
    source: Vec<u8>,
    seeds: Vec<Seed>,
    spawn_flags: u64,
}
impl CallableRecipe {
    pub fn capture(
        source: &InstanceKey,
        request: &CallableRequest,
        table: &LayoutTable,
    ) -> Result<Self, String> {
        let keys = LayoutKeys::new(table);
        let seeds = request
            .seeds
            .iter()
            .map(|(parameter, target)| {
                if target.symbol.kind != SymbolKind::Function {
                    return Err("callable seed is not a function".to_string());
                }
                Ok(Seed {
                    parameter: u32::try_from(*parameter)
                        .map_err(|_| "callable parameter exceeds source representation")?,
                    definition: target.symbol.definition.clone(),
                    instance: target.symbol.instance,
                    abi: keys
                        .fn_abi_of(table, target.abi)
                        .ok_or("missing callable seed ABI")?,
                    environment: target.environment,
                })
            })
            .collect::<Result<Vec<_>, String>>()?;
        let recipe = Self {
            schema: 1,
            definition: source.definition().clone(),
            body: source.body(),
            source: source.encoding().to_vec(),
            seeds,
            spawn_flags: request.spawn_flags,
        };
        if recipe.key()? != request.key {
            return Err("callable recipe differs from requested derived instance".into());
        }
        Ok(recipe)
    }
    pub fn source(&self) -> Result<InstanceKey, String> {
        if self.schema != 1
            || self.seeds.is_empty()
            || self
                .seeds
                .windows(2)
                .any(|pair| pair[0].parameter >= pair[1].parameter)
            || self.spawn_flags & !(crate::spawn::IMMEDIATE | crate::spawn::OP_FREE) != 0
            || (!self.definition.is_core("task.async") && self.spawn_flags != 0)
        {
            return Err("noncanonical callable recipe".into());
        }
        InstanceKey::from_encoding(&self.definition, self.body, self.source.clone())
            .map_err(|error| format!("callable source key: {error:?}"))
    }
    pub fn key(&self) -> Result<InstanceKey, String> {
        let source = self.source()?;
        Ok(specialization_key(
            &source,
            self.seeds.iter().map(|seed| {
                (
                    seed.parameter as usize,
                    &seed.definition,
                    seed.instance,
                    seed.environment,
                )
            }),
            self.spawn_flags,
        ))
    }
    /// The requested owner is independently selected by the consumer. Do not
    /// turn an arbitrary recipe into a different current instance silently.
    pub fn bind(
        &self,
        expected: &InstanceKey,
        binding: &Binding,
    ) -> Result<CallableRequest, String> {
        let key = self.key()?;
        if &key != expected {
            return Err("callable recipe has another requested owner".into());
        }
        let seeds = self
            .seeds
            .iter()
            .map(|seed| {
                Ok((
                    seed.parameter as usize,
                    CallableTarget {
                        symbol: Symbol {
                            kind: SymbolKind::Function,
                            definition: seed.definition.clone(),
                            instance: seed.instance,
                        },
                        abi: binding.abi(seed.abi).ok_or("missing callable seed ABI")?,
                        environment: seed.environment,
                    },
                ))
            })
            .collect::<Result<Vec<_>, String>>()?;
        Ok(CallableRequest {
            key,
            seeds,
            spawn_flags: self.spawn_flags,
        })
    }
}
