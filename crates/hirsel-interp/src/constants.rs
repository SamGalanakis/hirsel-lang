//! Compiled constant data is decoded once into program-lifetime storage.
use crate::{Error, Value, aggregate, invalid, scalar};
use hirsel_syntax::DefPath;
use hirsel_tir::{Materialized, TirProvider, ValueKind, ValueNode};
use std::collections::BTreeMap;

#[derive(Default)]
pub(crate) struct Constants(BTreeMap<DefPath, Value>);
impl Constants {
    pub(crate) fn initialize(
        provider: &mut (impl TirProvider + ?Sized),
        runtime: &aggregate::RuntimeTypes,
    ) -> Result<Self, Error> {
        let mut values = Self::default();
        for path in provider.constant_definitions().map_err(Error::Query)? {
            values.ensure(provider, &path, runtime)?;
        }
        Ok(values)
    }
    pub(crate) fn read(
        &mut self,
        provider: &mut (impl TirProvider + ?Sized),
        path: &DefPath,
        runtime: &aggregate::RuntimeTypes,
    ) -> Result<Value, Error> {
        self.ensure(provider, path, runtime)?;
        // The reference evaluator models read-only shares with semantic copies;
        // physical holder/allocation accounting belongs to the native oracle.
        Ok(self.0[path].clone())
    }
    pub(crate) fn read_instance(
        &mut self,
        provider: &mut (impl TirProvider + ?Sized),
        path: &DefPath,
        arguments: &[u64],
        runtime: &aggregate::RuntimeTypes,
    ) -> Result<Value, Error> {
        if arguments.is_empty() {
            return self.read(provider, path, runtime);
        }
        let key = hirsel_tir::constant_instance_path(path, arguments);
        if !self.0.contains_key(&key) {
            let data = provider
                .constant_instance(path, arguments)
                .map_err(Error::Query)?;
            self.0.insert(key.clone(), decode(&data, runtime)?);
        }
        Ok(self.0[&key].clone())
    }
    fn ensure(
        &mut self,
        provider: &mut (impl TirProvider + ?Sized),
        path: &DefPath,
        runtime: &aggregate::RuntimeTypes,
    ) -> Result<(), Error> {
        if !self.0.contains_key(path) {
            let data = provider.constant(path).map_err(Error::Query)?;
            self.0.insert(path.clone(), decode(&data, runtime)?);
        }
        Ok(())
    }
}
fn decode(data: &Materialized, runtime: &aggregate::RuntimeTypes) -> Result<Value, Error> {
    enum Work<'a> {
        Visit(&'a ValueNode),
        Construct(&'a ValueNode, usize),
    }
    let mut pending = vec![Work::Visit(&data.root)];
    let mut values = Vec::new();
    let mut reference_types = None;
    while let Some(work) = pending.pop() {
        match work {
            Work::Visit(node) => match &node.kind {
                ValueKind::Constant(value) => {
                    let row = data
                        .types
                        .get(node.ty)
                        .ok_or_else(|| invalid("materialized type"))?;
                    values.push(
                        scalar::constant(value, &row.kind)
                            .map_err(|_| invalid("materialized literal"))?,
                    );
                }
                ValueKind::FunctionReference(definition) => {
                    values.push(Value::FunctionReference(crate::value::NamedFunction {
                        definition: definition.clone(),
                        arguments: vec![],
                        types: reference_types
                            .get_or_insert_with(|| std::sync::Arc::new(data.types.clone()))
                            .clone(),
                        ty: node.ty,
                    }))
                }
                ValueKind::Construct { fields, .. } => {
                    pending.push(Work::Construct(node, fields.len()));
                    pending.extend(fields.iter().rev().map(Work::Visit));
                }
            },
            Work::Construct(node, count) => {
                let ValueKind::Construct { constructor, .. } = &node.kind else {
                    unreachable!()
                };
                let fields = values
                    .split_off(values.len() - count)
                    .into_iter()
                    .map(Some)
                    .collect();
                let row = data
                    .types
                    .get(node.ty)
                    .ok_or_else(|| invalid("materialized constructor type"))?;
                values.push(aggregate::construct(
                    constructor,
                    fields,
                    &row.kind,
                    runtime,
                    vec![],
                )?);
            }
        }
    }
    values
        .pop()
        .ok_or_else(|| invalid("empty materialized constant"))
}
