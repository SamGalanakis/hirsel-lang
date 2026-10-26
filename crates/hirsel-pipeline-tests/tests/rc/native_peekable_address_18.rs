//! Stable resource views retain their source address through match transport.
//! Reuse the Rust f6c1107f peekable vectors already ported in SEM-5; Hirsel's
//! explicit fill and fused end remain authoritative (stdlib/std@1/peekable.hir).
use crate::rc_support::*;

const RESOURCE: &str = r#"
resource type Item { n: u64 }
drop(item) {}
fn step(mut n: u64) -> Option[Item] { Some(Item(n: n)) }
pub fn main() -> Result[(), ()] {
  let mut p = std@1/peekable.new(iter.from(0u64, step))
  let _ = std@1/peekable.fill(mut p)
  Ok(())
}
"#;

#[test]
fn resource_option_address_and_forged_controls() {
    use hirsel_lir::ir::{AddressBase, AddressId, AddressInfo, Holding, Inst, PoolRange};
    use hirsel_lir::layout_types::CopyRecipe;
    use hirsel_lir::validate::ViolationKind;
    let (program, mut lowered) = lower_valid(RESOURCE);
    let fill = unit_named(&lowered, "fill");
    let (view, address) = fill
        .instructions
        .iter()
        .find_map(|inst| {
            let Inst::Read {
                result, address, ..
            } = *inst
            else {
                return None;
            };
            let info = &fill.values[result.index()];
            let layout = fill.types[info.ty.index()].layout;
            (matches!(info.holding, Holding::Borrowed(_))
                && lowered.table.layouts[layout.0 as usize].copy == CopyRecipe::NotCopyable)
                .then_some((result, address))
        })
        .expect("a borrowed resource-bearing cache");
    let ty = fill.values[view.index()].ty;
    // Recreate the old producer fault independently: an inline resource view
    // must never become an addressable snapshot, even when it is borrowed.
    let fill = lowered
        .units
        .iter_mut()
        .find(|u| u.header.definition.0.name == "fill")
        .unwrap();
    let bad = AddressId(fill.addresses.len() as u32);
    fill.addresses.push(AddressInfo {
        source_place: None,
        unchecked: vec![],
        base: AddressBase::Value(view),
        projection: PoolRange::EMPTY,
        ty,
    });
    assert!(
        violations(&program, &lowered)
            .iter()
            .flat_map(|(_, v)| v)
            .any(|v| v.kind == ViolationKind::AddressType(bad))
    );
    // A real slot projection also has to match its declared type.
    let fill = lowered
        .units
        .iter_mut()
        .find(|u| u.header.definition.0.name == "fill")
        .unwrap();
    fill.addresses.pop();
    let wrong = fill
        .types
        .iter()
        .position(|t| t.layout != fill.types[ty.index()].layout)
        .unwrap();
    fill.addresses[address.index()].ty = hirsel_lir::ir::LirTypeId(wrong as u32);
    assert!(
        violations(&program, &lowered)
            .iter()
            .flat_map(|(_, v)| v)
            .any(|v| v.kind == ViolationKind::AddressType(address))
    );
}

#[test]
fn original_cleanup_and_upstream_vectors_on_all_engines() {
    use hirsel_driver::native::*;
    use hirsel_engine::{
        RunConfiguration, SimulationConfig, run_observed_snapshot, with_compiler_stack,
    };
    let options = Options::new(target());
    let matrix: Vec<_> = [
        Engine::Heap { campaign: true },
        Engine::Native {
            opt: OptLevel::None,
            partition: Partition::Whole,
            runtime: Runtime::Diagnostic,
        },
        Engine::Native {
            opt: OptLevel::Default,
            partition: Partition::Whole,
            runtime: Runtime::Diagnostic,
        },
        Engine::Cranelift {
            partition: Partition::Whole,
            runtime: Runtime::Diagnostic,
        },
    ]
    .into_iter()
    .map(|engine| Configuration {
        options: options.clone(),
        engine,
    })
    .collect();
    let directory =
        std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("native-peekable-address-18");
    std::fs::create_dir_all(&directory).unwrap();
    let toolchain = Toolchain::for_matrix(&directory.join("runtime"), &matrix).unwrap();
    // Original files and sidecars, including exact failure coordinates and
    // ordered finalizer output. No resource is replaced with a plain value.
    let cases: &[(&str, &[u8], bool)] = &[
        (
            "DROP/DROP-2-iter-peekable-7-fill-fail.hir",
            b"item\nsource\n",
            true,
        ),
        (
            "DROP/DROP-2-iter-peekable-7-fill-run.hir",
            b"item\nsource\n",
            false,
        ),
        (
            "DROP/DROP-2-iter-peekable-7-next-fail.hir",
            b"item\nsource\n",
            true,
        ),
        (
            "DROP/DROP-2-iter-peekable-7-next-run.hir",
            b"item\nsource\n",
            false,
        ),
        (
            "DROP/DROP-2-iter-peekable-7-owned-run.hir",
            b"cached\nsource\nsource\ntransferred\nsource\nunpulled\n",
            false,
        ),
        // Empty, repeated peek and next-after-peek: the existing upstream port.
        ("SEM/SEM-5-iter-peekable-7-vectors-run.hir", b"", false),
        // A scoped contents view ends before the cached item is transferred.
        ("OWN/OWN-3-iter-peekable-7-scoped-run.hir", b"", false),
    ];
    let compiler =
        hirsel_driver::COMPILER
            .with_owned_engines()
            .with_config(hirsel_engine::CompileConfig {
                store: hirsel_engine::store::MemStore::shared(),
                ..Default::default()
            });
    let mut executions = 0;
    for &(relative, output, fails) in cases {
        with_compiler_stack(|| {
            let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../conformance")
                .join(relative);
            let mut loaded = compiler.load_program(&path).unwrap();
            loaded.check_for_execution().unwrap();
            let run = run_observed_snapshot(
                &mut loaded.engine,
                &RunConfiguration {
                    entry: loaded.entry.clone(),
                    hash_seed: 0,
                    inputs: vec![],
                },
                SimulationConfig::default(),
            )
            .unwrap();
            let reference = Observation::of_reference(run.observation.as_ref().unwrap());
            assert_eq!(
                reference.output, output,
                "{relative}: finalization order/count"
            );
            assert_eq!(
                reference.failures.len(),
                usize::from(fails),
                "{relative}: {reference:?}"
            );
            assert_eq!(reference.exit_code, if fails { None } else { Some(0) });
            if fails {
                let failure = &reference.failures[0];
                assert_eq!(failure.rule, "MEM-14");
                assert_eq!(failure.definition.0.name, "step");
                assert_eq!(failure.line, 23);
            }
            let input = loaded.engine.export_native_input().unwrap();
            let input = loaded.engine.import_native_input(input).unwrap();
            let directory = directory.join(path.file_stem().unwrap());
            std::fs::create_dir_all(&directory).unwrap();
            let outcomes = differential(
                &input,
                &loaded.entry,
                &reference,
                &matrix,
                &toolchain,
                &directory,
            );
            assert_eq!(outcomes.len(), 4);
            for (configuration, outcome) in outcomes {
                assert!(
                    matches!(outcome, Outcome::Agree),
                    "{relative}: {configuration}: {outcome:?}"
                );
                eprintln!("{relative}: {configuration}: agrees");
                executions += 1;
            }
        });
    }
    assert_eq!(executions, 28);
}
