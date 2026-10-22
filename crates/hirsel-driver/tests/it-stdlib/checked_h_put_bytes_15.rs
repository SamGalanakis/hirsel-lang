//! Borrowed inline cells use callee-relative provenance across a call.
use hirsel_driver::*;
use hirsel_engine::*;
use std::path::PathBuf;

fn parity(source: &str) {
    with_compiler_stack(|| {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/std_numeric.hir");
        let mut loaded =
            load_program_source(&path, Some(source)).unwrap_or_else(|error| panic!("{error}"));
        loaded
            .check_for_execution()
            .unwrap_or_else(|error| panic!("{error}"));
        let config = RunConfiguration {
            entry: loaded.entry.clone(),
            hash_seed: 0,
            inputs: vec![],
        };
        let plain = run_observed_snapshot(&mut loaded.engine, &config, SimulationConfig::default())
            .unwrap();
        let reference = native::Observation::of_reference(plain.observation.as_ref().unwrap());
        assert_eq!(reference.exit_code, Some(0), "{reference:?}");
        assert!(reference.failures.is_empty(), "{reference:?}");
        let checked =
            run_observed_checked(&mut loaded.engine, &config, SimulationConfig::default()).unwrap();
        assert_eq!(
            native::Observation::of_reference(checked.observation.as_ref().unwrap()),
            reference
        );
        let input = loaded.engine.export_native_input().unwrap();
        let validated = loaded.engine.import_native_input(input).unwrap();
        let options = native::Options::new(native::TargetSpec::x86_64_unknown_linux_gnu());
        let configurations = vec![
            native::Configuration {
                options: options.clone(),
                engine: native::Engine::Heap { campaign: false },
            },
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
        let scratch = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
            .join("checked-h-put-bytes-15")
            .join(format!(
                "{:?}",
                hirsel_syntax::ContentHash::of_bytes(source.as_bytes())
            ));
        std::fs::create_dir_all(&scratch).unwrap();
        let toolchain =
            native::Toolchain::for_matrix(&scratch.join("runtime"), &configurations).unwrap();
        let outcomes = native::differential(
            &validated,
            &loaded.entry,
            &reference,
            &configurations,
            &toolchain,
            &scratch,
        );
        assert_eq!(outcomes.len(), 3);
        for (configuration, outcome) in outcomes {
            assert!(
                matches!(outcome, native::Outcome::Agree),
                "{configuration}: {outcome:?}"
            );
        }
    });
}

const ENCODE: &str = r#"
fn put_u64(mut out: List[u8], value: u64) -> () uses {} {
  for i in 0u64..8 { list.push(mut out, int.truncate(value >> i * 8)) }
}
fn put_bytes(mut out: List[u8], data: Bytes) -> () uses {} {
  put_u64(mut out, bytes.len(data))
  for b in data { list.push(mut out, b) }
}
"#;

#[test]
fn minimal_store() {
    parity(
        r#"
pub fn main(t: Test) -> Result[(), store.StoreError] {
  let mut s = store.open(test.dir(t, []), store.default_config())?
  let mut tx = store.begin(mut s)
  txn.put(mut tx, b"a", b"1")
  let ticket = store.commit(mut s, take tx)?
  store.durable(mut s, ticket)?
  store.close(take s)
}
"#,
    );
}

#[test]
fn nested_writes() {
    parity(&format!(
        "{ENCODE}{}",
        r#"
type Write = Put(key: Bytes, value: Bytes) | Delete(key: Bytes)
fn encode(commits: List[List[Write]]) -> Bytes {
  let mut out: List[u8] = []
  for set in commits {
    put_u64(mut out, list.len(set))
    for w in set {
      match w {
        Put(key: k, value: v) => { put_bytes(mut out, k); put_bytes(mut out, v) },
        Delete(key: k) => put_bytes(mut out, k),
      }
    }
  }
  bytes.from_list(take out)
}
pub fn main(t: Test) -> Result[(), ()] {
  test.check(t, bytes.len(encode([[Put(key: b"a", value: b"1")]])) > 0)
  Ok(())
}
"#
    ));
}

#[test]
fn one_list_match() {
    parity(&format!(
        "{ENCODE}{}",
        r#"
type Write = Put(key: Bytes, value: Bytes)
fn encode(writes: List[Write]) -> Bytes {
  let mut out: List[u8] = []
  for w in writes { match w { Put(key: k, value: v) => { put_bytes(mut out, k); put_bytes(mut out, v) } } }
  bytes.from_list(take out)
}
pub fn main(t: Test) -> Result[(), ()] {
  test.check(t, bytes.len(encode([Put(key: b"a", value: b"1")])) > 0)
  Ok(())
}
"#
    ));
}

#[test]
fn list_bytes() {
    parity(&format!(
        "{ENCODE}{}",
        r#"
fn encode(writes: List[Bytes]) -> Bytes {
  let mut out: List[u8] = []
  for w in writes { put_bytes(mut out, w) }
  bytes.from_list(take out)
}
pub fn main(t: Test) -> Result[(), ()] {
  test.check(t, bytes.len(encode([b"a"])) > 0)
  Ok(())
}
"#
    ));
}

#[test]
fn direct_bytes() {
    parity(&format!(
        "{ENCODE}{}",
        r#"
pub fn main(t: Test) -> Result[(), ()] {
  let mut out: List[u8] = []
  put_bytes(mut out, b"a")
  test.check(t, list.len(out) == 9)
  Ok(())
}
"#
    ));
}

#[test]
fn match_no_loop() {
    parity(
        r#"
type Write = Put(key: Bytes)
fn read(data: Bytes) -> u64 { let _ = bytes.len(data); let mut n = 0u64; for b in data { n = n + 1 }; n }
fn encode(w: Write) -> u64 { match w { Put(key: k) => read(k) } }
pub fn main(t: Test) -> Result[(), ()] { test.check(t, encode(Put(key: b"a")) == 1); Ok(()) }
"#,
    );
}
#[test]
fn tiny_iter_match() {
    parity(
        r#"
type Write = Put(key: Bytes)
fn read(data: Bytes) -> u64 { let _ = bytes.len(data); let mut n = 0u64; for b in data { n = n + 1 }; n }
fn encode(writes: List[Write]) -> u64 {
  let mut n = 0u64
  for w in writes { match w { Put(key: k) => { n = n + read(k) } } }
  n
}
pub fn main(t: Test) -> Result[(), ()] { test.check(t, encode([Put(key: b"a")]) == 1); Ok(()) }
"#,
    );
}
#[test]
fn tiny_iter_record() {
    parity(
        r#"
type Write { key: Bytes }
fn read(data: Bytes) -> u64 { let _ = bytes.len(data); let mut n = 0u64; for b in data { n = n + 1 }; n }
fn encode(writes: List[Write]) -> u64 {
  let mut n = 0u64
  for w in writes { n = n + read(w.key) }
  n
}
pub fn main(t: Test) -> Result[(), ()] { test.check(t, encode([Write(key: b"a")]) == 1); Ok(()) }
"#,
    );
}

#[test]
fn tiny_double_len() {
    parity(
        r#"
type Write = Put(key: Bytes)
fn read(data: Bytes) -> u64 { bytes.len(data) + bytes.len(data) }
fn encode(writes: List[Write]) -> u64 {
  let mut n = 0u64
  for w in writes { match w { Put(key: k) => { n = n + read(k) } } }
  n
}
pub fn main(t: Test) -> Result[(), ()] { test.check(t, encode([Put(key: b"a")]) == 2); Ok(()) }
"#,
    );
}
