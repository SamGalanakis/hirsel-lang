//! Helpers of hirsel-lower's lowering tests: definitions lowered from source by the
//! driver, and execution of lowered definitions on the reference interpreter.
#![allow(dead_code)]
use hirsel_engine::{DefinitionQueries, QueryEngine};
use hirsel_interp::{Execution, Limits};
use hirsel_span::SourceMap;
use hirsel_syntax::{DefPath, FileContext, ItemPath, PackageId};
use hirsel_tir::{Definition, DefinitionRef, RunConfiguration, TirProvider};
use hirsel_types::QueryError;
use std::sync::Arc;

pub fn path(name: &str) -> DefPath {
    DefPath(ItemPath {
        package: PackageId {
            name: "fixture".into(),
            version: "1".into(),
        },
        modules: vec![],
        name: name.into(),
    })
}

pub struct Provider(pub Vec<Definition>);
impl TirProvider for Provider {
    fn constant(&mut self, _path: &DefPath) -> Result<hirsel_tir::MaterializedRef, QueryError> {
        Err(QueryError::NotImplemented)
    }
    fn constant_definitions(&mut self) -> Result<Vec<DefPath>, QueryError> {
        Ok(vec![])
    }
    fn definition(&mut self, path: &DefPath) -> Result<DefinitionRef, QueryError> {
        self.0
            .iter()
            .find(|d| &d.path == path)
            .cloned()
            .map(Arc::new)
            .ok_or(QueryError::NotImplemented)
    }
}

pub fn execute(definitions: Vec<Definition>, arguments: Vec<hirsel_interp::Value>) -> Execution {
    let mut sources = SourceMap::default();
    sources.add("fixture.hir", "x\n".repeat(100)).unwrap();
    hirsel_interp::run(
        &mut Provider(definitions),
        &sources,
        &RunConfiguration {
            entry: path("main"),
            hash_seed: 0,
            inputs: vec![],
        },
        arguments,
        Limits::default(),
    )
    .unwrap()
}

pub fn returned(execution: Execution) -> hirsel_interp::Value {
    match execution.outcome {
        hirsel_interp::Outcome::Returned { value, .. } => value,
        outcome => panic!("unexpected {outcome:?}"),
    }
}

pub fn bits(execution: Execution) -> u128 {
    match returned(execution) {
        hirsel_interp::Value::Integer(i) => i.bits(),
        value => panic!("expected integer, got {value:?}"),
    }
}

fn source_engine(source: &str) -> QueryEngine {
    let mut engine = QueryEngine::new(
        hirsel_engine::default_environment(),
        hirsel_driver::compiler_adapters(),
    );
    let context = FileContext {
        package: path("main").0.package,
        modules: vec![],
    };
    let file = engine.sources.add("source.hir", source.into()).unwrap();
    let parsed = engine.parse_file(file, &context).unwrap();
    assert!(parsed.diagnostics.is_empty());
    engine
}

pub fn source_definition(source: &str) -> Definition {
    let mut engine = source_engine(source);
    (*engine.lower_to_tir(&path("main")).unwrap().value).clone()
}

pub fn execute_source(source: &str) -> Execution {
    let mut engine = source_engine(source);
    engine.lower_to_tir(&path("main")).unwrap();
    let runtime = engine.runtime_types().unwrap();
    let sources = engine.sources.clone();
    hirsel_interp::run_with_runtime(
        &mut engine,
        &sources,
        &RunConfiguration {
            entry: path("main"),
            hash_seed: 0,
            inputs: vec![],
        },
        vec![],
        Limits::default(),
        &mut hirsel_interp::NoOperations,
        &runtime,
    )
    .unwrap()
}
