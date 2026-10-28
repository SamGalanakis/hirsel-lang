//! Real LLVM/Cranelift execution of pointer-free leaf resource hooks. The C
//! embedding collects copy-only rows; it never supplies a resource identity.
use crate::support::{self, *};
use hirsel_lir::emission::{EmitInputs, OptLevel, site_code};
use hirsel_lir::ir::*;
use hirsel_lir::layout_types::TargetSpec;
use hirsel_lir::ownership_trace::native::LeafPlan;
use hirsel_run_schema::ownership_trace::{
    Action,
    native::{self as trace, Log, Metadata, Row, Units},
};
use hirsel_types::Primitive;
use std::collections::BTreeMap;

fn fixture() -> support::programs::Program {
    let target = TargetSpec::x86_64_unknown_linux_gnu();
    let mut schema = Schema::new();
    let integer = schema.types.p(Primitive::I64);
    let shape = schema.types.record(&[integer]);
    let definition = path("Token");
    schema.owners.nominals.insert(
        hirsel_syntax::DefId::intern(&definition),
        hirsel_layout::layout::OwnerRepresentation::Defined(hirsel_tir::RepresentationRecord {
            definition: definition.clone(),
            representation_revision: hash("pointer-free token with empty drop"),
            generics: vec![],
            private_shape: shape,
            layout_promises: Default::default(),
            resource: true,
            finalizer: Some((path("Token_drop"), hirsel_tir::BodyId(0))),
            unsafe_ffi_package: false,
        }),
    );
    let token = schema.types.named(&definition, vec![]);
    let void = schema.types.unit();
    schema.name("token", token);
    schema.signature("entry", &[], integer);
    schema.signature(
        "drop Token",
        &[(hirsel_syntax::PassingMode::Borrow, token)],
        void,
    );
    let table = schema.build(&target);
    let manifest = manifest();
    let mut unit = Unit::new(&table, "ownership_leaf", "entry");
    let symbol = unit.symbol(SymbolKind::Type, "Token");
    let layout = table.layout("token");
    let slot = unit.slot(layout, SlotKind::Local);
    let address = unit.address(AddressBase::Slot(slot), &[], layout);
    unit.builder.push(ENTRY, Inst::StorageLive { slot });
    unit.prepare(ENTRY, address, None, SourceAnchorId(0));
    for _ in 0..2 {
        let payload = unit.int(ENTRY, hirsel_lir::layout_types::ScalarKind::I64, 7);
        let value = unit.construct(
            ENTRY,
            ConstructorKind::Record(symbol),
            layout,
            &[(0, payload)],
        );
        unit.builder.unit.values[value.index()].holding = Holding::Owned;
        unit.store(ENTRY, address, value, slot);
        let anchor = unit.cleanup_anchor(80, 90);
        unit.builder
            .push(ENTRY, Inst::FinalizeBegin { address, anchor });
        // Ready permits the known empty user body to be inlined away. Its
        // actual shielded finalization boundary still commits at FinalizeEnd.
        unit.builder
            .push(ENTRY, Inst::FinalizeEnd { address, anchor });
        unit.builder.push(
            ENTRY,
            Inst::StorageDead {
                slot,
                if_live: false,
            },
        );
        unit.builder.push(ENTRY, Inst::StorageLive { slot });
    }
    unit.builder.push(
        ENTRY,
        Inst::StorageDead {
            slot,
            if_live: false,
        },
    );
    let result = unit.int(ENTRY, hirsel_lir::layout_types::ScalarKind::I64, 7);
    unit.builder.ret(ENTRY, Some(result));
    let unit = seal(&table, &manifest, unit.finish());
    let mut drop = Unit::new(&table, "Token_drop", "drop Token");
    drop.param(layout);
    drop.slot(layout, SlotKind::Parameter(0));
    drop.builder.ret(ENTRY, None);
    let drop = seal(&table, &manifest, drop.finish());
    support::programs::Program {
        table,
        manifest,
        units: vec![unit, drop],
        statics: vec![],
        finalizers: BTreeMap::from([(layout, instance("Token_drop"))]),
    }
}

fn cycle_fixture() -> support::programs::Program {
    let mut program = fixture();
    let table = &program.table;
    let mut unit = Unit::new(table, "ownership_leaf", "entry");
    let layout = table.layout("token");
    let boolean = table
        .table
        .layouts
        .iter()
        .position(|row| {
            matches!(
                row.kind,
                hirsel_lir::layout_types::LayoutKind::Scalar(
                    hirsel_lir::layout_types::ScalarKind::Bool
                )
            )
        })
        .map(|i| hirsel_lir::layout_types::LayoutId(i as u32))
        .unwrap();
    let symbol = unit.symbol(SymbolKind::Type, "Token");
    let mut holders = Vec::new();
    for _ in 0..2 {
        let slot = unit.slot(layout, SlotKind::Local);
        let address = unit.address(AddressBase::Slot(slot), &[], layout);
        unit.builder.push(ENTRY, Inst::StorageLive { slot });
        unit.prepare(ENTRY, address, None, SourceAnchorId(0));
        holders.push((slot, address));
    }
    let mut values = Vec::new();
    for _ in 0..2 {
        let payload = unit.int(ENTRY, hirsel_lir::layout_types::ScalarKind::I64, 7);
        let value = unit.construct(
            ENTRY,
            ConstructorKind::Record(symbol),
            layout,
            &[(0, payload)],
        );
        unit.builder.unit.values[value.index()].holding = Holding::Owned;
        if let Some(&first) = values.first() {
            let failure = unit.builder.unit.tables.failures.len() - 1;
            let cleanup = unit.builder.block(BlockKind::FailureCleanup, &[]);
            let (slot, address) = holders[0];
            unit.store(cleanup, address, first, slot);
            let anchor = unit.cleanup_anchor(80, 90);
            unit.builder
                .push(cleanup, Inst::FinalizeBegin { address, anchor });
            unit.builder
                .push(cleanup, Inst::FinalizeEnd { address, anchor });
            unit.builder.push(
                cleanup,
                Inst::StorageDead {
                    slot,
                    if_live: false,
                },
            );
            unit.builder.term(cleanup, Terminator::Propagate);
            let edge = unit.builder.unit.tables.failures[failure].cleanup;
            unit.builder.unit.tables.edges[edge.index()].target = cleanup;
        }
        values.push(value);
    }
    let again = unit.constant(ENTRY, boolean, ConstantValue::Bool(true));
    let stop = unit.constant(ENTRY, boolean, ConstantValue::Bool(false));
    let x = unit.value(layout, Holding::Owned);
    let y = unit.value(layout, Holding::Owned);
    let flag = unit.plain(boolean);
    let body = unit.builder.block(BlockKind::Body, &[x, y, flag]);
    let left = unit.value(layout, Holding::Owned);
    let right = unit.value(layout, Holding::Owned);
    let done = unit.builder.block(BlockKind::Body, &[left, right]);
    let edge = unit.builder.edge(body, &[values[0], values[1], again]);
    unit.builder.term(ENTRY, Terminator::Jump(edge));
    let yes = unit.builder.edge(body, &[y, x, stop]);
    let no = unit.builder.edge(done, &[x, y]);
    unit.builder.term(
        body,
        Terminator::Branch {
            condition: flag,
            yes,
            no,
        },
    );
    for (value, (slot, address)) in [left, right].into_iter().zip(holders) {
        unit.store(done, address, value, slot);
        let anchor = unit.cleanup_anchor(80, 90);
        unit.builder
            .push(done, Inst::FinalizeBegin { address, anchor });
        unit.builder
            .push(done, Inst::FinalizeEnd { address, anchor });
        unit.builder.push(
            done,
            Inst::StorageDead {
                slot,
                if_live: false,
            },
        );
    }
    let result = unit.int(done, hirsel_lir::layout_types::ScalarKind::I64, 7);
    unit.builder.ret(done, Some(result));
    let unit = seal(table, &program.manifest, unit.finish());
    program.units[0] = unit;
    program
}

fn inputs(program: &support::programs::Program) -> EmitInputs<'_> {
    static EMPTY: std::collections::BTreeSet<hirsel_syntax::ContentHash> =
        std::collections::BTreeSet::new();
    static INPUT: std::sync::OnceLock<hirsel_lir::stage::InputIdentity> =
        std::sync::OnceLock::new();
    EmitInputs {
        ahead: None,
        continuation: None,
        alternate: false,
        verify_backend: true,
        index: None,
        input: INPUT.get_or_init(input),
        target: &program.table.target,
        layouts: &program.table.table,
        keys: &program.table.keys,
        manifest: &program.manifest,
        statics: &[],
        inline_instances: &EMPTY,
        available_externally: &EMPTY,
        callbacks: false,
        foreign: &hirsel_lir::emission::NO_FOREIGN,
        tracking: false,
        opt: OptLevel::None,
        measurement: false,
        fuel: false,
    }
}

fn metadata(program: &support::programs::Program) -> Units {
    let unit = &program.units[0];
    let plan = LeafPlan::prepare(unit, &program.table.table).unwrap();
    let markers = plan
        .markers()
        .map(|marker| {
            let location = |at: hirsel_lir::ownership_trace::native::NativeLocation| {
                (u64::from(at.kind) << 32) | u64::from(at.index)
            };
            (
                marker.id.0,
                Metadata {
                    empty_environment: false,
                    optional_carrier: false,
                    action: marker.kind,
                    callee: None,
                    cleanup: None,
                    cleanup_paths: BTreeMap::new(),
                    additional_drops: Vec::new(),
                    token_path: None,
                    owner_path: None,
                    token: location(marker.token),
                    owner: location(marker.owner),
                    site: site_code(
                        &unit.unit().header.definition,
                        &unit.unit().header.implementation_revision,
                        marker.site,
                    ),
                    layout: marker.layout.0,
                    source: hirsel_run_schema::Site {
                        definition: unit.unit().header.definition.to_string(),
                        file: "ownership_leaf.hir".into(),
                        line: u64::from(marker.site.start) + 1,
                        column: 1,
                    },
                    value_type: "fixture::Token".into(),
                },
            )
        })
        .collect();
    BTreeMap::from([(plan.plan().unit.0, markers)])
}

#[test]
fn ownership_native_emission_is_explicit_and_leaves_default_object_bytes_unchanged() {
    let header = std::fs::read_to_string(
        std::path::Path::new(support::native::RUNTIME_INCLUDE).join("hirsel_rt_ownership_trace.h"),
    )
    .unwrap();
    let version = header
        .lines()
        .find_map(|line| line.strip_prefix("#define HIRSEL_RT_V2_OWNERSHIP_TRACE_ABI_VERSION "))
        .unwrap()
        .parse::<u32>()
        .unwrap();
    assert_eq!(version, hirsel_lir::ownership_trace::native::ABI_VERSION);
    let program = fixture();
    let inputs = inputs(&program);
    let units = program.units();
    for backend in ["llvm", "cranelift"] {
        let ordinary = || {
            if backend == "llvm" {
                hirsel_llvm::emit_object(&units, &inputs).unwrap()
            } else {
                hirsel_cranelift::emit_object(&units, &inputs).unwrap()
            }
        };
        let before = ordinary();
        let traced = if backend == "llvm" {
            hirsel_llvm::emit_object_ownership(&units, &inputs).unwrap()
        } else {
            hirsel_cranelift::emit_object_ownership(&units, &inputs).unwrap()
        };
        let after = ordinary();
        assert_eq!(before.bytes, after.bytes);
        assert!(
            before
                .imports
                .iter()
                .all(|s| !s.name.contains("ownership_"))
        );
        assert_ne!(
            before.reproducibility.options,
            traced.reproducibility.options
        );
        for symbol in [
            "hirsel_rt_v2_ownership_enter",
            "hirsel_rt_v2_ownership_event",
            "hirsel_rt_v2_ownership_leave",
        ] {
            assert!(
                traced.imports.iter().any(|s| s.name == symbol),
                "{backend}: {symbol}"
            );
        }
    }
}

#[test]
fn ownership_native_unsupported_transports_and_template_emission_fail_closed() {
    let program = support::programs::heap_program(&TargetSpec::x86_64_unknown_linux_gnu());
    assert!(
        program
            .units
            .iter()
            .any(|unit| LeafPlan::prepare(unit, &program.table.table).is_err())
    );
    let args = inputs(&program);
    let units = program.units();
    assert!(hirsel_llvm::emit_object_ownership(&units, &args).is_err());
    assert!(hirsel_cranelift::emit_object_ownership(&units, &args).is_err());

    let leaf = fixture();
    let mut args = inputs(&leaf);
    args.alternate = true;
    assert!(hirsel_llvm::emit_object_ownership(&leaf.units(), &args).is_err());
    assert!(hirsel_cranelift::emit_object_ownership(&leaf.units(), &args).is_err());
}

#[test]
#[ignore = "native: links the existing diagnostic runtime and runs x86_64 code"]
fn ownership_native_equal_pointer_free_resources_have_distinct_real_lifetimes_on_both_backends() {
    run_equal_resources(false);
}

#[test]
#[ignore = "native: executes a parallel Resource swap with the diagnostic runtime"]
fn ownership_native_parallel_resource_swap_keeps_both_equal_identities_on_both_backends() {
    run_equal_resources(true);
}

fn run_equal_resources(cycle: bool) {
    let program = if cycle { cycle_fixture() } else { fixture() };
    let inputs = inputs(&program);
    let units = program.units();
    let metadata = metadata(&program);
    let mut traces = vec![];
    for backend in ["llvm", "cranelift"] {
        let object = if backend == "llvm" {
            hirsel_llvm::emit_object_ownership(&units, &inputs).unwrap()
        } else {
            hirsel_cranelift::emit_object_ownership(&units, &inputs).unwrap()
        };
        let fixture = if cycle { "swap" } else { "equal" };
        let dir = support::native::scratch(&format!("ownership-leaf-{fixture}-{backend}"));
        std::fs::write(
            dir.join("entries.h"),
            program.c_call("ownership_leaf", "call_leaf"),
        )
        .unwrap();
        let source = dir.join("ownership.c");
        let source_text = include_str!("../native/ownership.c");
        let source_text = if cycle {
            source_text.replace("count == 16", "count == 32")
        } else {
            source_text.to_owned()
        };
        std::fs::write(&source, source_text).unwrap();
        let glue = program.glue();
        let executable = support::native::link(
            &program.table.target,
            &dir,
            &[("unit.o", &object.bytes), ("glue.o", &glue)],
            &[source],
        );
        let output = support::native::run(&program.table.target, &executable);
        let mut rows = vec![];
        for line in output.lines() {
            let parts: Vec<_> = line.split_whitespace().collect();
            let number = |index: usize| parts[index].parse::<u64>().unwrap();
            rows.push(match parts[0] {
                "enter" => {
                    let mut unit = [0; 32];
                    assert_eq!(parts[2].len(), 64);
                    for (i, byte) in unit.iter_mut().enumerate() {
                        *byte = u8::from_str_radix(&parts[2][2 * i..2 * i + 2], 16).unwrap();
                    }
                    Row::Enter {
                        task: 0,
                        frame: number(1),
                        unit,
                    }
                }
                "leave" => Row::Leave {
                    task: 0,
                    frame: number(1),
                },
                "event" => Row::Event {
                    task: 0,
                    frame: number(1),
                    marker: number(2) as u32,
                    action: number(3) as u32,
                    token: number(4),
                    owner: number(5),
                    site: number(6),
                    layout: number(7) as u32,
                },
                _ => panic!("unexpected native trace row: {line}"),
            });
        }
        let log = Log {
            version: trace::VERSION,
            program: [49; 32],
            row_count: rows.len() as u64,
            rows,
            complete: true,
        };
        let trace = trace::decode_leaf(&log, [49; 32], &metadata, 100).unwrap();
        let analysis = trace.analyze([49; 32], 100).unwrap();
        assert_eq!(
            (analysis.created, analysis.dropped, analysis.live.len()),
            (4, 4, 0)
        );
        let identities: std::collections::BTreeSet<_> = trace
            .events
            .iter()
            .filter_map(|e| match &e.action {
                Action::Create { token, .. } => Some(token),
                _ => None,
            })
            .collect();
        assert_eq!(
            identities.len(),
            4,
            "equal payloads and reused slots cannot identify resources"
        );
        traces.push(trace);
    }
    assert_eq!(traces[0], traces[1]);
}

fn relocated_plans(
    program: &support::programs::Program,
) -> BTreeMap<hirsel_syntax::ContentHash, hirsel_lir::ownership_trace::Plan> {
    program
        .units
        .iter()
        .map(|unit| {
            let mut plan = LeafPlan::prepare(unit, &program.table.table)
                .unwrap()
                .plan()
                .clone();
            for observation in &mut plan.observations {
                observation.marker.id.0 += 100;
            }
            (unit.unit().digest(), plan)
        })
        .collect()
}

#[test]
fn ownership_relocated_emission_preserves_ids_and_binds_the_object_to_its_plan() {
    let program = fixture();
    let inputs = inputs(&program);
    let units = program.units();
    let plans = relocated_plans(&program);
    for backend in ["llvm", "cranelift"] {
        let emit = |plans: &BTreeMap<_, _>| {
            if backend == "llvm" {
                hirsel_llvm::emit_object_relocated_ownership(&units, &inputs, plans).unwrap()
            } else {
                hirsel_cranelift::emit_object_relocated_ownership(&units, &inputs, plans).unwrap()
            }
        };
        let ordinary = if backend == "llvm" {
            hirsel_llvm::emit_object(&units, &inputs).unwrap()
        } else {
            hirsel_cranelift::emit_object(&units, &inputs).unwrap()
        };
        let object = emit(&plans);
        let mut other = plans.clone();
        for plan in other.values_mut() {
            for observation in &mut plan.observations {
                observation.marker.id.0 += 100;
            }
        }
        let changed = emit(&other);
        assert_ne!(
            object.bytes, changed.bytes,
            "{backend}: original marker ids must reach emitted hooks"
        );
        assert_ne!(
            object.reproducibility.options,
            changed.reproducibility.options
        );
        assert_ne!(
            ordinary.reproducibility.options,
            object.reproducibility.options
        );
        assert!(
            object
                .imports
                .iter()
                .any(|symbol| symbol.name == "hirsel_rt_v2_ownership_event")
        );
        let after = if backend == "llvm" {
            hirsel_llvm::emit_object(&units, &inputs).unwrap()
        } else {
            hirsel_cranelift::emit_object(&units, &inputs).unwrap()
        };
        assert_eq!(ordinary.bytes, after.bytes, "{backend}: trace-OFF bytes");
        let ir = if backend == "llvm" {
            hirsel_llvm::emit_ir_relocated_ownership(&units, &inputs, &plans).unwrap()
        } else {
            hirsel_cranelift::emit_clif_relocated_ownership(&units, &inputs, &plans).unwrap()
        };
        for observation in plans[&program.units[0].unit().digest()].observations.iter() {
            let constant = if backend == "llvm" {
                format!("i32 {}", observation.marker.id.0)
            } else {
                format!("iconst.i32 {}", observation.marker.id.0)
            };
            assert!(
                ir.contains(&constant),
                "{backend}: missing {constant}: {ir}"
            );
        }
    }
}

#[test]
fn ownership_relocated_emission_refuses_missing_stale_or_extra_plans_on_both_backends() {
    let program = fixture();
    let inputs = inputs(&program);
    let units = program.units();
    let original = relocated_plans(&program);
    for mutation in ["missing", "stale", "extra", "dropped", "forged"] {
        let mut plans = original.clone();
        let key = program.units[0].unit().digest();
        match mutation {
            "missing" => {
                plans.remove(&key);
            }
            "stale" => plans.get_mut(&key).unwrap().unit = hash("different unit"),
            "extra" => {
                plans.insert(hash("foreign unit"), plans[&key].clone());
            }
            "dropped" => {
                plans.get_mut(&key).unwrap().observations.remove(0);
            }
            "forged" => {
                let extra = plans[&key].observations[0].clone();
                plans.get_mut(&key).unwrap().observations.push(extra);
            }
            _ => unreachable!(),
        }
        assert!(
            hirsel_llvm::emit_object_relocated_ownership(&units, &inputs, &plans).is_err(),
            "llvm {mutation}"
        );
        assert!(
            hirsel_cranelift::emit_object_relocated_ownership(&units, &inputs, &plans).is_err(),
            "cranelift {mutation}"
        );
    }
}

#[test]
#[ignore = "native: executes transported marker hooks on LLVM and Cranelift"]
fn ownership_relocated_native_execution_reports_the_original_marker_ids() {
    let program = fixture();
    let inputs = inputs(&program);
    let units = program.units();
    let plans = relocated_plans(&program);
    let mut outputs = vec![];
    for backend in ["llvm", "cranelift"] {
        let object = if backend == "llvm" {
            hirsel_llvm::emit_object_relocated_ownership(&units, &inputs, &plans).unwrap()
        } else {
            hirsel_cranelift::emit_object_relocated_ownership(&units, &inputs, &plans).unwrap()
        };
        let dir = support::native::scratch(&format!("ownership-relocated-{backend}"));
        std::fs::write(
            dir.join("entries.h"),
            program.c_call("ownership_leaf", "call_leaf"),
        )
        .unwrap();
        let source = dir.join("ownership.c");
        std::fs::write(&source, include_str!("../native/ownership.c")).unwrap();
        let glue = program.glue();
        let executable = support::native::link(
            &program.table.target,
            &dir,
            &[("unit.o", &object.bytes), ("glue.o", &glue)],
            &[source],
        );
        let output = support::native::run(&program.table.target, &executable);
        let ids: Vec<_> = output
            .lines()
            .filter_map(|line| {
                let mut words = line.split_whitespace();
                (words.next() == Some("event"))
                    .then(|| words.nth(1).unwrap().parse::<u32>().unwrap())
            })
            .collect();
        assert_eq!(
            ids,
            (100..106).chain(100..106).collect::<Vec<_>>(),
            "{backend}: {output}"
        );
        assert_eq!(output.lines().count(), 16, "{backend}: {output}");
        outputs.push(output);
    }
    assert_eq!(outputs[0], outputs[1]);
}
