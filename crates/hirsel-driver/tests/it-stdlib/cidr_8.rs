//! Go netip vectors plus an independent per-bit oracle (no word/byte masks).
use hirsel_driver::*;
use hirsel_engine::*;
use std::path::PathBuf;

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(format!("tests/fixtures/cidr-8-{name}.hir"))
}

fn all_engines(name: &str) {
    with_compiler_stack(|| {
        let mut loaded = load_program(&fixture(name)).unwrap();
        loaded.check_for_execution().unwrap_or_else(|e| match e {
            ToolError::Query { error, .. } => panic!("{error:?}"),
            e => panic!("{e}"),
        });
        let reference = run_observed_snapshot(
            &mut loaded.engine,
            &RunConfiguration {
                entry: loaded.entry.clone(),
                hash_seed: 0,
                inputs: vec![],
            },
            SimulationConfig::default(),
        )
        .unwrap();
        assert!(
            matches!(
                reference.outcome,
                Ok(RunOutcome::Returned(hirsel_run_schema::Returned::Ok))
            ),
            "{name}: {:?}",
            reference.outcome
        );
        let expected = native::Observation::of_reference(reference.observation.as_ref().unwrap());
        let input = loaded.engine.export_native_input().unwrap();
        let validated = loaded.engine.import_native_input(input).unwrap();
        let options = native::Options::new(native::TargetSpec::x86_64_unknown_linux_gnu());
        let ready = native::build(&validated, &options).unwrap();
        let sites = native::Sites::new(validated.snapshot());
        let heap =
            native::run_heap_in(&ready, &loaded.entry, &sites, &native::HeapWorld::default())
                .unwrap();
        assert_eq!(heap.observation, expected);
        assert_eq!(heap.counters.live_heap_bytes, 0);
        let configs = [
            native::Configuration {
                options: options.clone(),
                engine: native::Engine::Native {
                    opt: native::OptLevel::Speed,
                    partition: native::Partition::Whole,
                    runtime: native::Runtime::Diagnostic,
                },
            },
            native::Configuration {
                options,
                engine: native::Engine::Cranelift {
                    partition: native::Partition::Whole,
                    runtime: native::Runtime::Diagnostic,
                },
            },
        ];
        let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("cidr-8-{name}"));
        let tools = native::Toolchain::for_matrix(&dir.join("runtime"), &configs).unwrap();
        let mut count = 0;
        for (config, outcome) in
            native::differential(&validated, &loaded.entry, &expected, &configs, &tools, &dir)
        {
            assert!(
                matches!(outcome, native::Outcome::Agree),
                "{name}: {config}: {outcome:?}"
            );
            count += 1;
        }
        assert_eq!(count, 2);
        eprintln!("cidr-8 {name}: interpreter + H + LLVM + Cranelift agree");
    });
}

#[test]
fn go_reference_vectors_all_engines() {
    all_engines("vectors");
}

#[test]
fn typed_errors_all_engines() {
    all_engines("errors");
}

fn literal(bytes: &[u8]) -> String {
    format!("V{}({bytes:?})", if bytes.len() == 4 { 4 } else { 6 })
}

fn bit(bytes: &[u8], index: usize) -> bool {
    bytes[index / 8] / 2u8.pow(7 - (index % 8) as u32) % 2 == 1
}

fn masked(bytes: &[u8], width: usize) -> Vec<u8> {
    let mut result = vec![0; bytes.len()];
    for index in 0..width {
        if bit(bytes, index) {
            result[index / 8] += 2u8.pow(7 - (index % 8) as u32);
        }
    }
    result
}

fn contains(a: &[u8], width: usize, b: &[u8]) -> bool {
    a.len() == b.len() && (0..width).all(|index| bit(a, index) == bit(b, index))
}

#[test]
fn every_width_masks_host_bits_roundtrips_and_membership() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/std_source/main.hir");
    let mut fixture = crate::source_std::TirFixture::new(&path).unwrap();

    let mut cases = 0;
    let mut programs = Vec::new();
    for len in [4, 16] {
        for width in 0..=len * 8 {
            let mut body = String::new();
            for a in [
                vec![0; len],
                vec![255; len],
                (0..len).map(|i| (i * 37 + 165) as u8).collect(),
            ] {
                body += "{\n";
                let ip = literal(&a);
                let expected = literal(&masked(&a, width));
                body += &format!(
                    "let Ok(p) = std@1/cidr.from_addr({ip}, {width}) else {{ return Error(()) }}\n\
                     core@1/test.check(t, std@1/cidr.addr(p) == {ip})\n\
                     core@1/test.check(t, std@1/cidr.bits(p) == {width}u64)\n\
                     let m = std@1/cidr.masked(p)\n\
                     core@1/test.check(t, std@1/cidr.addr(m) == {expected})\n\
                     core@1/test.check(t, std@1/cidr.bits(m) == {width}u64)\n\
                     core@1/test.check(t, std@1/cidr.addr(std@1/cidr.masked(std@1/cidr.masked(m))) == {expected})\n\
                     let Ok(round) = std@1/cidr.parse(std@1/cidr.format(p)) else {{ return Error(()) }}\n\
                     core@1/test.check(t, std@1/cidr.addr(round) == {ip} && std@1/cidr.bits(round) == {width}u64)\n"
                );
                // Flip first, boundary, host and last bits. A separate per-bit
                // oracle decides membership, rather than the masking oracle.
                for index in [
                    0,
                    width.saturating_sub(1),
                    width.min(len * 8 - 1),
                    len * 8 - 1,
                ] {
                    let mut b = a.clone();
                    b[index / 8] ^= 2u8.pow(7 - (index % 8) as u32);
                    body += &format!(
                        "core@1/test.check(t, std@1/cidr.contains(p, {}) == {})\n",
                        literal(&b),
                        contains(&a, width, &b)
                    );
                }
                body += "}\n";
                cases += 1;
            }
            programs.push((format!("width_{len}_{width}"), body));
        }
    }
    assert_eq!(programs.len(), 162);
    for chunk in programs.chunks(12) {
        let mut source = "pub fn main(t:Test)->Result[(),()]{Ok(())}\n".to_owned();
        let names = chunk
            .iter()
            .map(|(name, _)| name.clone())
            .collect::<Vec<_>>();
        for (name, body) in chunk {
            source.push_str(&format!(
                "pub fn {name}(t:Test)->Result[(),()]{{\n{body}\nOk(())\n}}\n"
            ));
        }
        let prepared = fixture.batch(&source, &names).unwrap();
        assert_eq!(prepared.len(), chunk.len());
        for (prepared, (_, body)) in prepared.iter().zip(chunk) {
            super::stdlib_net::execute_tir(prepared, body);
        }
    }
    assert_eq!(cases, 486);
    eprintln!("cidr-8: 486 prefix cases, 1944 per-bit membership comparisons");
}

fn execute_overlap_vectors(source: &str, cases: usize) {
    with_compiler_stack(|| {
        let path =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/std_source/main.hir");
        let prepared = crate::source_std::tir(&path, Some(source), None)
            .unwrap_or_else(|error| panic!("{error}\n{source}"));
        let config = RunConfiguration {
            entry: prepared.entry.clone(),
            hash_seed: 0,
            inputs: vec![],
        };
        for seed in 0..cases as u64 {
            let simulation = SimulationConfig {
                test_seed: seed,
                ..Default::default()
            };
            let run = testing::observe_tir(
                &prepared,
                &config,
                simulation.clone(),
                InterpreterAdapter::default().limits,
                false,
            )
            .unwrap();
            assert!(
                matches!(
                    run.outcome,
                    Ok(RunOutcome::Returned(hirsel_run_schema::Returned::Ok))
                ),
                "pair {seed}: {:?}\n{source}",
                run.outcome
            );
            let checked = testing::observe_tir(
                &prepared,
                &config,
                simulation,
                InterpreterAdapter::default().limits,
                true,
            )
            .unwrap();
            assert!(
                matches!(
                    checked.outcome,
                    Ok(RunOutcome::Returned(hirsel_run_schema::Returned::Ok))
                ),
                "pair {seed}: {:?}\n{source}",
                checked.outcome
            );
        }
    });
}

#[test]
fn generated_overlap_symmetry_and_containment() {
    let mut count = 0;
    let mut vectors = String::new();
    for len in [4, 16] {
        for seed in 0..32 {
            let a: Vec<u8> = (0..len).map(|i| (seed * 17 + i * 53) as u8).collect();
            let aw = seed * 7 % (len * 8 + 1);
            let bw = seed * 13 % (len * 8 + 1);
            for b in [a.clone(), masked(&a, bw), vec![0; len], vec![255; len]] {
                let expected = contains(&a, aw.min(bw), &b);
                vectors += &format!(
                    "({}, {aw}, {}, {bw}, {expected}),\n",
                    literal(&a),
                    literal(&b)
                );
                count += 1;
            }
        }
    }
    let source = format!(
        "pub fn main(t: Test) -> Result[(), ()] {{\n\
         let vectors: List[(IpAddr, i64, IpAddr, i64, bool)] = [{vectors}]\n\
         let (ip_a, aw, ip_b, bw, expected) = vectors[core@1/test.seed(t)]\n\
         let Ok(a) = std@1/cidr.from_addr(ip_a, aw) else {{ return Error(()) }}\n\
         let Ok(b) = std@1/cidr.from_addr(ip_b, bw) else {{ return Error(()) }}\n\
         core@1/test.check(t, std@1/cidr.overlaps(a, b) == expected)\n\
         core@1/test.check(t, std@1/cidr.overlaps(b, a) == expected)\n\
         core@1/test.check(t, std@1/cidr.overlaps(a, b) == (std@1/cidr.contains(a, std@1/cidr.addr(b)) || std@1/cidr.contains(b, std@1/cidr.addr(a))))\n\
         Ok(())\n}}"
    );
    execute_overlap_vectors(&source, count);
    assert_eq!(count, 256);
    eprintln!("cidr-8: 256 generated prefix pairs, symmetry and containment checked");
}
