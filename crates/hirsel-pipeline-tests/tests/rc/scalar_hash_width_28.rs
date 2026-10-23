//! Scalar-root hash specialization controls. Rust core/src/hash/sip.rs:245-335
//! and coretests/tests/hash/sip.rs:43-132 at f6c1107f (MIT, Rust Contributors).
//! Test expectations use an independent buffered reference, fixed upstream vectors,
//! and the unchanged byte ABI; no DefaultHasher framing is assumed.
use hirsel_driver::compiler_adapters;
use hirsel_driver::native::*;
use hirsel_engine::{
    QueryEngine, check_progress, default_context, default_environment, with_compiler_stack,
};
use hirsel_lir::layout::sequence_elements_offset;
use hirsel_lir::layout_types::{LayoutKind, SizeSpec};
use std::fmt::Write;

const SEEDS: [u64; 3] = [0, 1, u64::MAX];
const K1: u64 = 0x6869_7273_656c_2d37;

fn reference(data: &[u8], k0: u64, k1: u64) -> u64 {
    fn rounds(s: &mut [u64; 4], n: usize) {
        for _ in 0..n {
            s[0] = s[0].wrapping_add(s[1]);
            s[1] = s[1].rotate_left(13) ^ s[0];
            s[0] = s[0].rotate_left(32);
            s[2] = s[2].wrapping_add(s[3]);
            s[3] = s[3].rotate_left(16) ^ s[2];
            s[0] = s[0].wrapping_add(s[3]);
            s[3] = s[3].rotate_left(21) ^ s[0];
            s[2] = s[2].wrapping_add(s[1]);
            s[1] = s[1].rotate_left(17) ^ s[2];
            s[2] = s[2].rotate_left(32);
        }
    }
    let mut s = [
        0x736f6d6570736575 ^ k0,
        0x646f72616e646f6d ^ k1,
        0x6c7967656e657261 ^ k0,
        0x7465646279746573 ^ k1,
    ];
    let mut blocks = data.chunks_exact(8);
    for block in &mut blocks {
        let m = u64::from_le_bytes(block.try_into().unwrap());
        s[3] ^= m;
        rounds(&mut s, 1);
        s[0] ^= m;
    }
    let tail = blocks
        .remainder()
        .iter()
        .rev()
        .fold(0u64, |a, b| (a << 8) | u64::from(*b));
    let m = tail | ((data.len() as u64 & 255) << 56);
    s[3] ^= m;
    rounds(&mut s, 1);
    s[0] ^= m;
    s[2] ^= 255;
    rounds(&mut s, 3);
    s.into_iter().fold(0, |a, b| a ^ b)
}

const VECTORS: [u64; 64] = [
    0xabac0158050fc4dc,
    0xc9f49bf37d57ca93,
    0x82cb9b024dc7d44d,
    0x8bf80ab8e7ddf7fb,
    0xcf75576088d38328,
    0xdef9d52f49533b67,
    0xc50d2b50c59f22a7,
    0xd3927d989bb11140,
    0x369095118d299a8e,
    0x25a48eb36c063de4,
    0x79de85ee92ff097f,
    0x70c118c1f94dc352,
    0x78a384b157b4d9a2,
    0x306f760c1229ffa7,
    0x605aa111c0f95d34,
    0xd320d86d2a519956,
    0xcc4fdd1a7d908b66,
    0x9cf2689063dbd80c,
    0x8ffc389cb473e63e,
    0xf21f9de58d297d1c,
    0xc0dc2f46a6cce040,
    0xb992abfe2b45f844,
    0x7ffe7b9ba320872e,
    0x525a0e7fdae6c123,
    0xf464aeb267349c8c,
    0x45cd5928705b0979,
    0x3a3e35e3ca9913a5,
    0xa91dc74e4ade3b35,
    0xfb0bed02ef6cd00d,
    0x88d93cb44ab1e1f4,
    0x540f11d643c5e663,
    0x2370dd1f8c21d1bc,
    0x81157b6c16a7b60d,
    0x4d54b9e57a8ff9bf,
    0x759f12781f2a753e,
    0xcea1a3bebf186b91,
    0x2cf508d3ada26206,
    0xb6101c2da3c33057,
    0xb3f47496ae3a36a1,
    0x626b57547b108392,
    0xc1d2363299e41531,
    0x667cc1923f1ad944,
    0x65704ffec8138825,
    0x24f280d1c28949a6,
    0xc2ca1cedfaf8876b,
    0xc2164bfc9f042196,
    0xa16e9c9368b1d623,
    0x49fb169c8b5114fd,
    0x9f3143f8df074c46,
    0xc6fdaf2412cc86b3,
    0x7eaf49d10a52098f,
    0x1cf313559d292f9a,
    0xc44a30dda2f41f12,
    0x36fae98943a71ed0,
    0x318fb34c73f0bce6,
    0xa27abf3670a7e980,
    0xb4bcc0db243c6d75,
    0x23f8d852fdb71513,
    0x8f035f4da67d8a08,
    0xd89cd0e5b7e8f148,
    0xf6f4e6bcf7a644ee,
    0xaec59ad80f1837f2,
    0xc3b2f6154b6694e0,
    0x9d199062b7bbb3a8,
];
#[test]
fn upstream_vectors_and_seeded_byte_controls() {
    for (n, v) in VECTORS.iter().enumerate() {
        assert_eq!(
            reference(
                &(0..n as u8).collect::<Vec<_>>(),
                0x0706050403020100,
                0x0f0e0d0c0b0a0908
            ),
            *v,
            "vector {n}"
        );
    }
    for width in [1, 2, 4, 8, 16] {
        for seed in SEEDS {
            for word in words(width) {
                let bytes = word.to_le_bytes();
                let bytes = &bytes[..width];
                // SAFETY: bytes is readable for precisely the supplied length.
                let generic = unsafe {
                    hirsel_rt::hirsel_rt_v2_hash_bytes(seed, bytes.as_ptr(), bytes.len() as u64)
                };
                assert_eq!(generic, reference(bytes, seed, K1));
            }
        }
    }
}
fn words(width: usize) -> Vec<u128> {
    let mask = u128::MAX >> (128 - width * 8);
    let mut values = vec![
        0,
        1,
        mask,
        1u128 << (width * 8 - 1),
        (1u128 << (width * 8 - 1)) - 1,
        0x000102030405060708090a0b0c0d0e0f & mask,
        0x0f0e0d0c0b0a09080706050403020100 & mask,
    ];
    // Fixed independently generated words; no runtime or candidate RNG.
    values.extend(
        [
            0x206f0bf2ef6b22df80402aee10cba713,
            0x026dfc8cc195b354d4fe2865e4081f61,
            0xa69e44f2f36a36c5a98c14fafbba91a4,
            0x6cc64a115292ef7a0c5a35a9345849a7,
        ]
        .map(|n| n & mask),
    );
    let mut unique = Vec::new();
    for v in values {
        if !unique.contains(&v) {
            unique.push(v);
        }
    }
    unique
}
fn literal(word: u128, width: usize, signed: bool) -> String {
    let bits = width * 8;
    let ty = format!("{}{bits}", if signed { 'i' } else { 'u' });
    if signed && word & (1u128 << (bits - 1)) != 0 {
        let magnitude = (!word & (u128::MAX >> (128 - bits))) + 1;
        format!("-{magnitude}{ty}")
    } else {
        format!("{word}{ty}")
    }
}

#[test]
fn native_cached_hashes_exact_across_widths_and_seeds() {
    with_compiler_stack(|| {
        let mut source = String::new();
        let mut cases = Vec::new();
        for width in [1, 2, 4, 8, 16] {
            for signed in [false, true] {
                let ty = format!("{}{}", if signed { 'i' } else { 'u' }, width * 8);
                let name = format!("build_{ty}");
                let values = words(width);
                writeln!(
                    source,
                    "pub fn {name}() -> Map[{ty},u64] {{ let mut m: Map[{ty},u64] = map.empty()"
                )
                .unwrap();
                for (i, v) in values.iter().enumerate() {
                    writeln!(
                        source,
                        " map.insert(mut m,{},{}u64)",
                        literal(*v, width, signed),
                        i
                    )
                    .unwrap();
                }
                source.push_str(" take m }\n");
                cases.push((name, width, values));
            }
        }
        source.push_str("pub fn build_bool() -> Map[bool,u64] {let mut m: Map[bool,u64] = map.empty(); map.insert(mut m,false,0);map.insert(mut m,true,1);take m}\n");
        cases.push(("build_bool".into(), 1, vec![0, 1]));
        // Static maps have no placement hashes. An absent insertion warms an
        // owned copy; equal replacement can leave the hash cache cold.
        source.push_str("const NATIVE_COLD: Map[u32,u64] = build_u32()\npub fn cold_u32() -> Map[u32,u64] {let mut m = copy NATIVE_COLD; map.insert(mut m,2u32,999u64); take m}\n");
        let mut cold_words = words(4);
        assert!(!cold_words.contains(&2));
        cold_words.push(2);
        cases.push(("cold_u32".into(), 4, cold_words));
        source.push_str("pub fn build_composite() -> Map[(u64,u32),u64] {let mut m: Map[(u64,u32),u64] = map.empty(); map.insert(mut m,(72623859790382856u64,16909060u32),0);map.insert(mut m,(1u64,256u32),1); take m}\n");
        cases.push((
            "build_composite".into(),
            12,
            vec![
                72623859790382856 | (16909060u128 << 64),
                1 | (256u128 << 64),
            ],
        ));
        source.push_str(&history_source());
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
        let file = engine
            .sources
            .add("scalar-hash-width-28.hir", source)
            .unwrap();
        let checked = check_progress(&mut engine, file, &default_context()).unwrap();
        assert!(
            !checked
                .diagnostics
                .iter()
                .any(|d| d.severity == hirsel_diag::Severity::Error),
            "{:?}",
            checked.diagnostics
        );
        let input = engine.export_native_input().unwrap();
        let validated = engine.import_native_input(input).unwrap();
        let target = TargetSpec::x86_64_unknown_linux_gnu();
        let ready = build(&validated, &Options::new(target.clone())).unwrap();
        let root = tempfile::tempdir().unwrap();
        let tools =
            Toolchain::in_tree_diagnostic(&root.path().join("runtime"), &[&target]).unwrap();
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
            let mut c = String::from(
                "#include \"hirsel_rt_v2.h\"\n#include <assert.h>\n#include <stdint.h>\n#include <string.h>\n#include <stdio.h>\n#include <stdlib.h>\nstruct binding {const hirsel_rt_v2_layout *const *layouts;uint64_t count;};\nextern const struct binding program __asm__(\"hirsel.program.registry\");\n",
            );
            let mut checks = String::new();
            for (name, width, values) in &cases {
                let unit = ready
                    .units
                    .iter()
                    .map(|u| u.unit())
                    .find(|u| u.header.definition.0.name == *name)
                    .unwrap();
                let abi = &ready.table.fn_abis[unit.header.abi.0 as usize];
                let symbol = objects
                    .iter()
                    .flat_map(|o| &o.exports)
                    .find(|e| e.definition.as_ref().is_some_and(|p| p.0.name == *name))
                    .unwrap();
                c.push_str(&c_call(
                    &ready.table,
                    abi,
                    &symbol.name,
                    &format!("call_{name}"),
                ));
                let LayoutKind::Block { payload } =
                    ready.table.layouts[abi.result.as_ref().unwrap().layout.0 as usize].kind
                else {
                    panic!("map")
                };
                let map = &ready.table.layouts[payload.0 as usize];
                let field = &map.fields[2];
                let LayoutKind::Block { payload } =
                    ready.table.layouts[field.layout.0 as usize].kind
                else {
                    panic!("hashes")
                };
                let record = &ready.table.layouts[payload.0 as usize];
                let LayoutKind::Sequence { element } = record.kind else {
                    panic!("sequence")
                };
                let element = &ready.table.layouts[element.0 as usize];
                assert_eq!(element.size, SizeSpec::Fixed(8));
                let start = sequence_elements_offset(element).unwrap();
                let expected = SEEDS
                    .iter()
                    .map(|seed| {
                        values
                            .iter()
                            .map(|word| {
                                format!(
                                    "UINT64_C({})",
                                    reference(&word.to_le_bytes()[..*width], *seed, K1)
                                )
                            })
                            .collect::<Vec<_>>()
                            .join(",")
                    })
                    .map(|row| format!("{{{row}}}"))
                    .collect::<Vec<_>>()
                    .join(",");
                writeln!(checks,"{{ const uint64_t expected[3][{}] = {{{expected}}}; hirsel_rt_v2_header *m=0,*h=0; assert(call_{name}(ctx,0,&m)==0);memcpy(&h,(uint8_t*)hirsel_rt_v2_payload(m)+{},sizeof h);uint8_t *p=hirsel_rt_v2_payload(h);uint64_t count=0;memcpy(&count,p+{},8);if(count!={}){{fprintf(stderr,\"{name}: cached count %llu\\n\",(unsigned long long)count);abort();}}for(uint64_t i=0;i<count;++i){{uint64_t hash;memcpy(&hash,p+{start}+i*8,8);assert(hash==expected[run][i]);}} assert(hirsel_rt_v2_release(ctx,m,m->descriptor & HIRSEL_RT_V2_INDEX_MASK,0,0,0)==0);}}",values.len(),field.offset,record.fields[0].offset,values.len()).unwrap();
            }
            for unit in ready
                .units
                .iter()
                .map(|u| u.unit())
                .filter(|u| u.header.definition.0.name.starts_with("trace_"))
            {
                let name = &unit.header.definition.0.name;
                let abi = &ready.table.fn_abis[unit.header.abi.0 as usize];
                let symbol = objects
                    .iter()
                    .flat_map(|o| &o.exports)
                    .find(|e| e.definition.as_ref().is_some_and(|p| p.0.name == *name))
                    .unwrap();
                c.push_str(&c_call(
                    &ready.table,
                    abi,
                    &symbol.name,
                    &format!("call_{name}"),
                ));
                writeln!(
                    checks,
                    "{{uint8_t ok=0;assert(call_{name}(ctx,&ok)==0 && ok);}}"
                )
                .unwrap();
            }
            write!(c,"int main(void) {{const uint64_t seeds[3]={{0,1,UINT64_MAX}};for(unsigned run=0;run<3;++run){{hirsel_rt_v2_config cfg={{.abi_version=HIRSEL_RT_V2_ABI_VERSION,.flags=HIRSEL_RT_V2_CONFIG_DIAGNOSTIC,.hash_seed=seeds[run],.layouts=program.layouts,.layout_count=(uint32_t)program.count}};hirsel_rt_v2_ctx *ctx=0;assert(hirsel_rt_v2_ctx_create(&cfg,&ctx)==0);{checks}hirsel_rt_v2_audit audit;hirsel_rt_v2_task_exit(ctx,&audit);assert(audit.live_blocks==0);hirsel_rt_v2_ctx_destroy(ctx);}}puts(\"exact hashes agree\");}}\n").unwrap();
            let dir = root.path().join(backend.name());
            std::fs::create_dir_all(&dir).unwrap();
            let harness = dir.join("scalar.c");
            std::fs::write(&harness, c).unwrap();
            let paths: Vec<_> = objects
                .iter()
                .enumerate()
                .map(|(i, o)| {
                    let p = dir.join(format!("{i}.o"));
                    std::fs::write(&p, &o.bytes).unwrap();
                    p
                })
                .collect();
            let exe = dir.join("program");
            let linked = std::process::Command::new(&tools.clang)
                .arg(&harness)
                .arg("-I")
                .arg(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../hirsel-rt/include"))
                .args(paths)
                .arg(tools.archive(&target.triple).unwrap())
                .args([
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
                "{}",
                String::from_utf8_lossy(&linked.stderr)
            );
            let run = std::process::Command::new(exe).output().unwrap();
            assert!(
                run.status.success(),
                "{}",
                String::from_utf8_lossy(&run.stderr)
            );
            assert_eq!(run.stdout, b"exact hashes agree\n");
            eprintln!(
                "SCALAR_HASH_WIDTH_28 backend={} seeds=3 scalar_types=11 cold=true composite=true exact=true",
                backend.name()
            );
        }
    });
}

fn history_source() -> String {
    let mut source = String::new();
    let mut calls = String::new();
    for width in [1, 2, 4, 8, 16] {
        for signed in [false, true] {
            let ty = format!("{}{}", if signed { 'i' } else { 'u' }, width * 8);
            let values = words(width);
            writeln!(
                source,
                "pub fn trace_{ty}() -> bool {{let mut m: Map[{ty},u64] = map.empty()"
            )
            .unwrap();
            for (i, v) in values.iter().enumerate() {
                writeln!(
                    source,
                    "map.insert(mut m,{},{}u64)",
                    literal(*v, width, signed),
                    i + 10
                )
                .unwrap();
            }
            for (i, v) in values.iter().enumerate() {
                writeln!(
                    source,
                    "if map.get(m,{}) != Some({}u64) {{return false}}",
                    literal(*v, width, signed),
                    i + 10
                )
                .unwrap();
            }
            writeln!(
                source,
                "map.insert(mut m,{},99u64)",
                literal(values[1], width, signed)
            )
            .unwrap();
            writeln!(
                source,
                "map.remove(mut m,{})",
                literal(values[0], width, signed)
            )
            .unwrap();
            writeln!(
                source,
                "if map.get(m,{}) != None {{return false}}",
                literal(values[0], width, signed)
            )
            .unwrap();
            writeln!(
                source,
                "map.insert(mut m,{},100u64)",
                literal(values[0], width, signed)
            )
            .unwrap();
            let expected: Vec<_> = (1..values.len())
                .map(|i| if i == 1 { 99 } else { i + 10 })
                .chain([100])
                .collect();
            source.push_str("let mut at: u64 = 0\nfor (k,v) in m {\n");
            for (i, v) in expected.iter().enumerate() {
                writeln!(source, "if at == {i}u64 && v != {v}u64 {{return false}}").unwrap();
            }
            writeln!(source, "at += 1\n}}\nat == {}u64\n}}", values.len()).unwrap();
            write!(calls, "if !trace_{ty}() {{return Error(1u8)}}\n").unwrap();
        }
    }
    source.push_str("const COLD: Map[u32,u64] = cold()\nfn cold() -> Map[u32,u64] {let mut m: Map[u32,u64] = map.empty();map.insert(mut m,16909060,10);map.insert(mut m,67305985,20);take m}\n");
    source.push_str("pub fn main() -> Result[(),u8] {\n");
    source.push_str(&calls);
    source.push_str("let mut b: Map[bool,u64] = map.empty();map.insert(mut b,false,1);map.insert(mut b,true,2);map.insert(mut b,false,3);if map.get(b,false) != Some(3u64) || map.get(b,true) != Some(2u64) {return Error(2u8)}\nif map.get(COLD,16909060u32) != Some(10u64) || map.get(COLD,67305985u32) != Some(20u64) {return Error(3u8)}\nOk(())}\n");
    source
}

#[test]
fn map_histories_and_cold_statics_agree_on_four_engines() {
    let source = history_source();
    let dir = tempfile::tempdir().unwrap();
    let runner = crate::rc_support::history::Runner::with_matrix(
        dir.path(),
        [
            Engine::Heap { campaign: false },
            Engine::Native {
                opt: OptLevel::Speed,
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
            options: Options::new(TargetSpec::x86_64_unknown_linux_gnu()),
            engine,
        })
        .collect(),
    )
    .unwrap();
    let report = runner.run("scalar-hash-width-28-histories", &source);
    assert_eq!(report.cells.len(), 4, "{report:#?}");
    assert!(report.agrees(), "{source}\n{report:#?}");
    for cell in report.cells {
        assert!(
            cell.observation
                .as_ref()
                .is_some_and(|o| o.exit_code == Some(0) && o.failures.is_empty()),
            "{cell:#?}"
        );
        eprintln!(
            "SCALAR_HASH_WIDTH_28 engine={} histories=true cold=true",
            cell.engine
        );
    }
}
