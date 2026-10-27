//! Shared test support: compile a source program with the real driver and
//! adapt its validated `NativeInput` into the plain `Program` this crate reads
//! (the driver wires the same adapter).
use hirsel_driver::compiler_adapters;
use hirsel_engine::{
    DefinitionQueries, InstanceKind, NativeInput, QueryEngine, check_progress, default_context,
    default_environment, with_compiler_stack,
};
use hirsel_layout::layout::{OwnerRepresentation, Owners};

use hirsel_lir::stage::InputIdentity;
use hirsel_rc::instance::{Program, Requirement, RequirementKind};
use hirsel_syntax::{ContentHash, DefPath};
use hirsel_types::{NameSite, SignatureKind};

/// Each item's anchor base in `source`; LIR and H sites are relative to it.
pub fn item_bases(source: &str) -> std::collections::BTreeMap<DefPath, u32> {
    let source = source.to_string();
    with_compiler_stack(move || {
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
        let file = engine.sources.add("test.hir", source).unwrap();
        let parsed = engine.parse_file(file, &default_context()).unwrap().value;
        parsed
            .items
            .iter()
            .map(|syntax| {
                let (_, base) = engine.sources.anchor(syntax.anchor).unwrap();
                (syntax.item.definition.clone(), base)
            })
            .collect()
    })
}

/// Check `source`, export its native snapshot and import it again, so every
/// program a test uses passed the driver's revalidation.
pub fn compile(source: &str) -> Program {
    try_compile(source).unwrap_or_else(|e| panic!("{source}\n{e}"))
}

/// `compile`, reporting a program the driver does not accept.
pub fn try_compile(source: &str) -> Result<Program, String> {
    let source = source.to_string();
    with_compiler_stack(move || {
        let mut engine = if source.contains("std@1/") {
            let mut loaded = crate::source_std::load_program_source(
                &std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("../hirsel-driver/tests/fixtures/std_numeric.hir"),
                Some(&source),
            )
            .map_err(|e| format!("load: {e:?}"))?;
            loaded
                .check_for_execution()
                .map_err(|e| format!("check: {e:?}"))?;
            loaded.engine
        } else {
            let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
            let file = engine.sources.add("test.hir", source.clone()).unwrap();
            let result = check_progress(&mut engine, file, &default_context())
                .map_err(|e| format!("check: {e:?}"))?;
            if let Some(d) = result
                .diagnostics
                .iter()
                .find(|d| d.severity == hirsel_diag::Severity::Error)
            {
                return Err(format!("diagnostic: {d:?}"));
            }
            engine
        };
        let input = engine
            .export_native_input()
            .map_err(|e| format!("export: {e:?}"))?;
        let validated = engine
            .import_native_input(input)
            .map_err(|e| format!("import: {e:?}"))?;
        Ok(program(validated.snapshot()))
    })
}

pub fn program(input: &NativeInput) -> Program {
    let kind = |k: InstanceKind| match k {
        InstanceKind::Call => RequirementKind::Call,
        InstanceKind::Reference => RequirementKind::Reference,
        InstanceKind::Drop => RequirementKind::Drop,
        InstanceKind::Representation => RequirementKind::Representation,
        InstanceKind::Materialized => RequirementKind::Materialized,
    };
    let mut owners = Owners::default();
    for (path, record) in &input.representations {
        match record {
            Some(record) => {
                owners.nominals.insert(
                    hirsel_syntax::DefId::intern(path),
                    OwnerRepresentation::Defined(record.clone()),
                );
            }
            None => {
                let signature = &input.signatures[path];
                if let SignatureKind::Type(t) = &signature.kind
                    && t.capability.is_none()
                    && let Some(shape) = t.representation
                {
                    // A library nominal whose shape is public (Result, Option):
                    // the driver exports no record for it yet, so the adapter
                    // derives one from the public signature.
                    owners.nominals.insert(
                        hirsel_syntax::DefId::intern(path),
                        OwnerRepresentation::Defined(hirsel_tir::RepresentationRecord {
                            definition: path.clone(),
                            representation_revision: signature.fingerprint,
                            generics: t.generics.clone(),
                            private_shape: shape,
                            layout_promises: t.layout.clone(),
                            resource: t.resource,
                            finalizer: None,
                            unsafe_ffi_package: signature.metadata.unsafe_ffi,
                        }),
                    );
                } else if let SignatureKind::Type(t) = &input.signatures[path].kind
                    && (t.capability.is_some() || *path == hirsel_syntax::DefPath::core("Instant"))
                {
                    owners.nominals.insert(
                        hirsel_syntax::DefId::intern(path),
                        OwnerRepresentation::Handle {
                            revision: input.signatures[path].fingerprint,
                            resource: t.resource,
                        },
                    );
                }
            }
        }
    }
    Program {
        input: InputIdentity {
            interface_version: hirsel_lir::INTERFACE_VERSION,
            digest: ContentHash::of_parts("hirsel-rc.test-input", [b"test".as_slice()]),
        },
        types: input.types.clone(),
        memo: std::sync::Arc::new(hirsel_types::TypeMemo::new()),
        definitions: input.definitions.clone(),
        signatures: input.signatures.clone(),
        revisions: input
            .source_owners
            .iter()
            .map(|(p, o)| (p.clone(), o.implementation_revision))
            .collect(),
        binders: input
            .source_owners
            .iter()
            .map(|(p, o)| (p.clone(), o.syntax.owner().site(NameSite::Item)))
            .collect(),
        operations: input
            .operations
            .keys()
            .map(|k| {
                (
                    k.definition.clone(),
                    hirsel_core_contract::OperationId {
                        definition: k.definition.clone(),
                        declaration_digest: k.declaration_digest,
                    },
                )
            })
            .collect(),
        requirements: input
            .instances
            .edges
            .iter()
            .map(|e| Requirement {
                owner: e.owner.clone(),
                owner_binder: e.owner_binder,
                site: e.site,
                kind: kind(e.kind),
                callee: e.callee.clone(),
                binder: e.binder,
                arguments: e.arguments.clone(),
            })
            .collect(),
        owners,
        constants: input.constants.clone(),
        instance_bodies: input.instance_bodies.clone(),
    }
}

/// A path in the standalone test package.
pub fn path(name: &str) -> DefPath {
    let mut p = hirsel_engine::entry_path(&default_context());
    p.0.name = name.into();
    p
}

use hirsel_lir::ir::{
    ArgumentValue, Callee, EdgeId, FailureId, Inst, LirUnit, Terminator, ValueId,
};
use hirsel_lir::stage::Stage;
use hirsel_lir::validate::{Inputs, Violation};
use hirsel_rc::import::Lowered;
use std::fmt::Write;

pub fn target() -> hirsel_lir::layout_types::TargetSpec {
    hirsel_lir::layout_types::TargetSpec::x86_64_unknown_linux_gnu()
}

/// Lower a program for the reference target.
pub fn lower(source: &str) -> (Program, Lowered) {
    let program = compile(source);
    let lowered = hirsel_rc::import::lower(&program, &target())
        .unwrap_or_else(|e| panic!("{source}\nlower: {e:?}"));
    (program, lowered)
}

pub fn manifest(program: &Program) -> hirsel_core_contract::Manifest {
    let inventory = hirsel_engine::registry::BuiltinRegistry::shared().operation_inventory();
    let records = program.operations.values().map(|operation| {
        let key = hirsel_engine::OperationKey {
            definition: operation.definition.clone(),
            declaration_digest: operation.declaration_digest,
        };
        inventory
            .get(&key)
            .expect("source operation has its exact registry declaration")
    });
    let manifest = hirsel_driver::native::manifest_for_operations(records)
        .expect("source native capabilities pass the production review");
    manifest
        .check_coverage(program.operations.values().cloned())
        .unwrap();
    manifest
}

/// Every violation of every unit, with its unit's definition name.
pub fn violations(program: &Program, lowered: &Lowered) -> Vec<(String, Vec<Violation>)> {
    let manifest = manifest(program);
    let target = target();
    let inputs = Inputs {
        memo: Default::default(),
        discharge: None,
        operations: &lowered.operation_registry(program),
        target: &target,
        layouts: &lowered.table,
        manifest: &manifest,
    };
    lowered
        .units
        .iter()
        .map(|u| {
            (
                u.header.definition.0.name.to_string(),
                hirsel_lir::validate::check(u, &inputs),
            )
        })
        .filter(|(_, v)| !v.is_empty())
        .collect()
}

fn edge<S: Stage>(u: &LirUnit<S>, e: EdgeId) -> String {
    let e = &u.tables.edges[e.index()];
    let args = e.arguments.get(&u.pools.values).unwrap_or(&[]);
    format!("b{}{}", e.target.0, values(args))
}

fn values(vs: &[ValueId]) -> String {
    if vs.is_empty() {
        return String::new();
    }
    let v: Vec<String> = vs.iter().map(|v| format!("v{}", v.0)).collect();
    format!("({})", v.join(", "))
}

fn failure<S: Stage>(u: &LirUnit<S>, f: FailureId) -> String {
    let f = &u.tables.failures[f.index()];
    format!(
        "fail{} -> {}",
        f.rule.as_ref().map(|r| format!(" {r}")).unwrap_or_default(),
        edge(u, f.cleanup)
    )
}

/// A compact, stable text form of a unit: what the dump fixtures record.
pub fn dump<S: Stage>(u: &LirUnit<S>) -> String {
    let mut out = String::new();
    let _ = writeln!(
        out,
        "unit {} params {}",
        u.header.definition.0.name,
        values(&u.parameters)
    );
    for (i, s) in u.slots.iter().enumerate() {
        let _ = writeln!(
            out,
            "  slot s{i}: {:?} scope {} ty t{}",
            s.kind, s.scope.0, s.ty.0
        );
    }
    for (i, a) in u.addresses.iter().enumerate() {
        let steps = a.projection.get(&u.pools.projections).unwrap_or(&[]);
        let _ = writeln!(out, "  addr a{i}: {:?}{:?}", a.base, steps);
    }
    for (b, block) in u.blocks.iter().enumerate() {
        let params = block.parameters.get(&u.pools.values).unwrap_or(&[]);
        let _ = writeln!(out, "  b{b} {:?}{}:", block.kind, values(params));
        for inst in block.instructions.get(&u.instructions).unwrap_or(&[]) {
            let text = match *inst {
                Inst::Address {
                    address,
                    failure: f,
                } => {
                    format!("prepare a{} {}", address.0, failure(u, f))
                }
                Inst::Read {
                    result,
                    address,
                    reaching,
                    mode,
                } => {
                    format!(
                        "v{} = read {mode:?} a{} @v{}",
                        result.0, address.0, reaching.0
                    )
                }
                Inst::Copy {
                    result,
                    address,
                    reaching,
                    failure: f,
                } => format!(
                    "v{} = copy a{} @v{} {}",
                    result.0,
                    address.0,
                    reaching.0,
                    failure(u, f)
                ),
                Inst::Store {
                    address,
                    value,
                    version,
                } => {
                    format!("a{} := v{} as v{}", address.0, value.0, version.0)
                }
                Inst::Drop {
                    address,
                    if_live,
                    failure: f,
                    ..
                } => format!(
                    "drop{} a{} {}",
                    if if_live { "?" } else { "" },
                    address.0,
                    failure(u, f)
                ),
                Inst::Binary {
                    result,
                    op,
                    left,
                    right,
                    failure: f,
                } => format!(
                    "v{} = {op:?} v{} v{} {}",
                    result.0,
                    left.0,
                    right.0,
                    failure(u, f)
                ),
                Inst::Const { result, constant } => format!(
                    "v{} = const {:?}",
                    result.0,
                    u.tables.constants[constant.index()].value
                ),
                other => format!("{other:?}"),
            };
            let _ = writeln!(out, "    {text}");
        }
        let term = match &u.terminators[b] {
            Terminator::Jump(e) => format!("jump {}", edge(u, *e)),
            Terminator::Branch { condition, yes, no } => {
                format!("branch v{} {} {}", condition.0, edge(u, *yes), edge(u, *no))
            }
            Terminator::Switch {
                value,
                arms,
                otherwise,
            } => {
                let arms: Vec<String> = arms
                    .get(&u.pools.switch_arms)
                    .unwrap_or(&[])
                    .iter()
                    .map(|a| format!("{} => {}", a.value, edge(u, a.edge)))
                    .collect();
                format!(
                    "switch v{} [{}] else {}",
                    value.0,
                    arms.join(", "),
                    edge(u, *otherwise)
                )
            }
            Terminator::Recover {
                arena,
                caught,
                failed,
            } => format!(
                "recover v{} {} else {}",
                arena.0,
                edge(u, *caught),
                edge(u, *failed)
            ),
            Terminator::Isolate { start, caught } => {
                format!("isolate v{} {}", start.0, edge(u, *caught))
            }
            Terminator::Call {
                call,
                normal,
                failure: f,
            } => {
                let c = &u.tables.calls[call.index()];
                let callee = match c.callee {
                    Callee::Direct(s) => u.tables.symbols[s.index()].definition.0.name.to_string(),
                    Callee::Indirect(v) => format!("*v{}", v.0),
                    Callee::Bound { code, closure } => format!(
                        "{}@v{}",
                        u.tables.symbols[code.index()].definition.0.name,
                        closure.0
                    ),
                    Callee::Operation(o) => {
                        format!("op {}", u.tables.operations[o.index()].definition.0.name)
                    }
                };
                let args: Vec<String> = c
                    .arguments
                    .get(&u.pools.call_arguments)
                    .unwrap_or(&[])
                    .iter()
                    .map(|a| match a.value {
                        ArgumentValue::Value(v) => {
                            format!("p{}={:?} v{}", a.parameter, a.passing, v.0)
                        }
                        ArgumentValue::Address { address, version } => {
                            format!("p{}=mut a{} as v{}", a.parameter, address.0, version.0)
                        }
                    })
                    .collect();
                format!(
                    "{}call {callee}({}) -> {} {}",
                    c.result.map(|r| format!("v{} = ", r.0)).unwrap_or_default(),
                    args.join(", "),
                    normal.map_or_else(|| "diverges".to_string(), |n| edge(u, n)),
                    failure(u, *f)
                )
            }
            Terminator::Return(v) => format!(
                "return{}",
                v.map(|v| format!(" v{}", v.0)).unwrap_or_default()
            ),
            Terminator::Raise(f) => format!("raise {}", failure(u, *f)),
            Terminator::RejectTransfer => "reject pending transfer".into(),
            Terminator::Propagate => "propagate".into(),
            Terminator::InvariantFault { reason } => format!("invariant fault: {reason}"),
            Terminator::Unreachable => "unreachable".into(),
        };
        let _ = writeln!(out, "    {term}");
    }
    out
}

/// The instances of a definition named `name`, in discovery order.
pub fn instances_of(lowered: &Lowered, name: &str) -> Vec<hirsel_rc::instance::InstanceId> {
    lowered
        .instances
        .ids()
        .filter(|&id| lowered.instances.view(id).key.definition().0.name == name)
        .collect()
}

/// The violations of one unit.
pub fn unit_violations(
    program: &Program,
    lowered: &Lowered,
    id: hirsel_rc::instance::InstanceId,
) -> Vec<Violation> {
    let manifest = manifest(program);
    let target = target();
    let inputs = Inputs {
        memo: Default::default(),
        discharge: None,
        operations: &lowered.operation_registry(program),
        target: &target,
        layouts: &lowered.table,
        manifest: &manifest,
    };
    hirsel_lir::validate::check(&lowered.units[id.index()], &inputs)
}

/// Direct calls in a unit whose callee is the unit's own instance.
pub fn self_calls<S: Stage>(u: &LirUnit<S>) -> usize {
    u.tables
        .calls
        .iter()
        .filter(|c| match c.callee {
            Callee::Direct(s) => {
                let s = &u.tables.symbols[s.index()];
                s.definition == u.header.definition && s.instance == Some(u.header.instance)
            }
            _ => false,
        })
        .count()
}

/// Whether the unit's control flow has a cycle (a loop).
pub fn has_cycle<S: Stage>(u: &LirUnit<S>) -> bool {
    let n = u.blocks.len();
    let succ = |b: usize| -> Vec<usize> {
        let edges: Vec<EdgeId> = match &u.terminators[b] {
            Terminator::Jump(e) => vec![*e],
            Terminator::Branch { yes, no, .. } => vec![*yes, *no],
            Terminator::Switch {
                arms, otherwise, ..
            } => arms
                .get(&u.pools.switch_arms)
                .unwrap_or(&[])
                .iter()
                .map(|a| a.edge)
                .chain([*otherwise])
                .collect(),
            Terminator::Call { normal, .. } => normal.iter().copied().collect(),
            _ => Vec::new(),
        };
        edges
            .iter()
            .map(|e| u.tables.edges[e.index()].target.index())
            .collect()
    };
    // 0 unvisited, 1 on stack, 2 done
    let mut color = vec![0u8; n];
    let mut stack = vec![(u.entry.index(), 0usize)];
    color[u.entry.index()] = 1;
    while let Some((b, i)) = stack.pop() {
        let s = succ(b);
        if i < s.len() {
            stack.push((b, i + 1));
            match color[s[i]] {
                1 => return true,
                0 => {
                    color[s[i]] = 1;
                    stack.push((s[i], 0));
                }
                _ => {}
            }
        } else {
            color[b] = 2;
        }
    }
    false
}

/// Every failure edge of a unit: its rule and how many owned values it
/// carries to its cleanup.
pub fn failure_edges<S: Stage>(u: &LirUnit<S>) -> Vec<(Option<String>, usize)> {
    u.tables
        .failures
        .iter()
        .map(|f| {
            let e = &u.tables.edges[f.cleanup.index()];
            (f.rule.clone(), e.arguments.len as usize)
        })
        .collect()
}

/// Lower, require every unit to validate, and return the program's units.
pub fn lower_valid(source: &str) -> (Program, Lowered) {
    let (program, lowered) = lower(source);
    let v = violations(&program, &lowered);
    if !v.is_empty() {
        let dumps: Vec<String> = lowered.units.iter().map(dump).collect();
        panic!("{source}\n{}\n{v:#?}", dumps.join("\n"));
    }
    (program, lowered)
}

pub fn unit_named<'a>(lowered: &'a Lowered, name: &str) -> &'a LirUnit<hirsel_lir::stage::Value> {
    &lowered.units[instances_of(lowered, name)[0].index()]
}

#[path = "../../../hirsel-driver/tests/support/history.rs"]
pub mod history;
