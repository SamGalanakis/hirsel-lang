//! Native physical MEM-12 evidence. Expected bytes are handwritten; only the
//! function calling convention and descriptor registry come from the compiler.
use crate::rc_support::target;
use hirsel_driver::compiler_adapters;
use hirsel_driver::native::*;
use hirsel_engine::*;

const TYPES: &str = "pub flat type Item { text: String, n: u64 }\npub flat type Batch { items: List[Item] @ growable }\n";

#[test]
fn flat_native_copy_oracle_17_both_backends() {
    with_compiler_stack(|| {
        let mut source = TYPES.to_string();
        for (name, body) in [
            ("empty", "Item(text: \"\", n: 7)"),
            ("ascii", "Item(text: \"abc\", n: 7)"),
            ("utf8", "Item(text: \"é🐑\", n: 7)"),
        ] {
            source.push_str(&format!("pub fn {name}() -> Item {{ {body} }}\n"));
        }
        for (name, body) in [
            ("zero", "Batch(items: [])"),
            ("one", "Batch(items: [Item(text: \"é🐑\", n: 7)])"),
            (
                "growth",
                "let mut xs: List[Item] = []\n list.reserve(mut xs, 1)\n for n in 0u64..3u64 { list.push(mut xs, Item(text: \"é🐑\", n: n)) }\n Batch(items: take xs)",
            ),
        ] {
            source.push_str(&format!("pub fn {name}() -> Batch {{ {body} }}\n"));
        }
        source.push_str("pub fn clone_item(x: Item) -> Item in x { copy x }\npub fn clone_batch(x: Batch) -> Batch in x { copy x }\npub fn write_item(mut x: Item, n: u64) { x.n = n }\npub fn fail_item(x: Item, z: u64) -> u64 { let a = copy x\n let b = copy x\n a.n / z + b.n }\npub fn local_region() -> u64 { let x = Item(text: \"é🐑\", n: 7)\n let mut y = copy x\n let mut z = copy x\n y.n = 11\n z.n = 19\n x.n + y.n + z.n }\npub fn main() -> Result[(), ()] { Ok(()) }\n");
        let root = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
            .join("flat-native-copy-oracle-17");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("source.hir"), &source).unwrap();
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
        let file = engine
            .sources
            .add("flat-native-copy-oracle-17.hir", source)
            .unwrap();
        let checked = check_progress(&mut engine, file, &default_context()).unwrap();
        assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
        let snapshot = engine.export_native_input().unwrap();
        let input = engine.import_native_input(snapshot).unwrap();
        let target = target();
        let tools = Toolchain::in_tree_diagnostic(&root.join("runtime"), &[&target]).unwrap();
        let mut options = Options::new(target.clone());
        options.regions = true;
        let ready = build(&input, &options).unwrap();
        for record in &ready.table.layouts {
            if record.kind == hirsel_lir::layout_types::LayoutKind::Flat {
                assert!(
                    record.scan.is_empty() && record.drop.is_empty() && record.finalizer.is_none()
                );
                assert!(
                    record
                        .abi
                        .scalars
                        .iter()
                        .all(|(_, kind)| *kind != hirsel_lir::layout_types::ScalarKind::Pointer)
                );
            }
        }
        for backend in [Backend::Llvm, Backend::Cranelift] {
            let objects = emit_backend_objects(
                &ready,
                backend,
                OptLevel::Speed,
                Partition::Whole,
                false,
                None,
            )
            .unwrap();
            let dir = root.join(backend.name());
            std::fs::create_dir_all(&dir).unwrap();
            let mut calls = String::new();
            for name in [
                "empty",
                "ascii",
                "utf8",
                "zero",
                "one",
                "growth",
                "clone_item",
                "clone_batch",
                "write_item",
                "fail_item",
                "local_region",
            ] {
                let unit = ready
                    .units
                    .iter()
                    .map(|u| u.unit())
                    .find(|u| u.header.definition.0.name == name)
                    .unwrap();
                let abi = &ready.table.fn_abis[unit.header.abi.0 as usize];
                let export = objects
                    .iter()
                    .flat_map(|o| &o.exports)
                    .find(|e| e.definition.as_ref().is_some_and(|p| p.0.name == name))
                    .unwrap();
                calls.push_str(&c_call(
                    &ready.table,
                    abi,
                    &export.name,
                    &format!("call_{name}"),
                ));
            }
            let harness = dir.join("oracle.c");
            std::fs::write(
                &harness,
                include_str!("flat_native_copy_oracle_17/oracle.c").replace("@CALLS@", &calls),
            )
            .unwrap();
            let exe = dir.join("oracle");
            let mut command = std::process::Command::new(&tools.clang);
            command
                .arg(&harness)
                .arg("-DHIRSEL_RT_ENABLE_STATS")
                .arg("-I")
                .arg(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../hirsel-rt/include"));
            for (i, object) in objects.iter().enumerate() {
                let path = dir.join(format!("unit-{i}.o"));
                std::fs::write(&path, &object.bytes).unwrap();
                command.arg(path);
            }
            let linked = command
                .arg(tools.archive(&target.triple).unwrap())
                .args([
                    "-Wl,--wrap=hirsel_rt_v2_region_owner",
                    "-Wl,--wrap=hirsel_rt_v2_alloc",
                    "-Wl,--wrap=hirsel_rt_v2_release",
                    "-Wl,--wrap=hirsel_rt_v2_free",
                    "-Wl,--wrap=hirsel_rt_v2_region_exit",
                    "-Wl,--wrap=hirsel_rt_v2_block_grow",
                    "-lgcc_s",
                    "-lutil",
                    "-lrt",
                    "-lpthread",
                    "-lm",
                    "-ldl",
                    "-o",
                ])
                .arg(&exe)
                .output()
                .unwrap();
            assert!(
                linked.status.success(),
                "{backend:?}: {}",
                String::from_utf8_lossy(&linked.stderr)
            );
            let run = std::process::Command::new(&exe).output().unwrap();
            assert!(
                run.status.success(),
                "{backend:?}: {}",
                String::from_utf8_lossy(&run.stderr)
            );
            assert_eq!(run.stdout, b"native flat oracle: 24 copy/lifetime cells, 2 cleanup failures, 3 rejected controls, 4 replacements, compiler region checkpoint; zero live allocations\n");
            eprintln!("{backend:?}: {}", String::from_utf8_lossy(&run.stdout));
        }
    });
}
