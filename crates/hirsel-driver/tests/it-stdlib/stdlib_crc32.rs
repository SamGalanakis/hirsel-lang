//! Go hash/crc32/crc32_test.go:71-72 (ebe2feefc400): the two long golden rows.
//! These and cutoff/split/list checks run outside the corpus's 1M-step budget.
use hirsel_driver::*;
use hirsel_engine::*;
use std::path::Path;

fn execute(source: &str) {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/std_numeric.hir");
    let prepared = crate::source_std::tir(&fixture, Some(source), None).unwrap();
    let run = testing::observe_tir(
        &prepared,
        &RunConfiguration {
            entry: prepared.entry.clone(),
            hash_seed: 0,
            inputs: vec![],
        },
        SimulationConfig::default(),
        hirsel_interp::Limits {
            steps: Some(100_000_000),
            call_depth: 256,
            source: None,
        },
        false,
    )
    .unwrap();
    assert!(
        matches!(
            run.outcome,
            Ok(RunOutcome::Returned(hirsel_run_schema::Returned::Ok))
        ),
        "{:?}",
        run.outcome
    );
}

// Independent bit-at-a-time oracle: no lookup table or slicing recurrence.
fn checksum(data: &[u8], polynomial: u32) -> u32 {
    let mut crc = !0u32;
    for byte in data {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            crc = (crc >> 1) ^ if crc & 1 == 1 { polynomial } else { 0 };
        }
    }
    !crc
}

fn vectors(module: &str, polynomial: u32, long: [u32; 2]) {
    with_compiler_stack(|| {
        let mut source = format!(
            r#"fn check(t: Test, data: Bytes, expected: u32) -> () {{
  core@1/test.check(t, std@1/{module}.digest(data) == expected)
  core@1/test.check(t, std@1/{module}.digest_hex(data) == f"{{expected:08x}}")
  let n = core@1/bytes.len(data)
  let splits: Array[u64] = [0u64, 1u64, 7u64, 8u64, 9u64, 15u64, 16u64, 17u64, n / 2, n]
  for split in splits {{
    if split <= n {{
      let mut state = std@1/{module}.new()
      std@1/{module}.update(mut state, core@1/bytes.slice(data, 0, split))
      std@1/{module}.update(mut state, b"")
      std@1/{module}.update(mut state, core@1/bytes.slice(data, split, n))
      core@1/test.check(t, std@1/{module}.finish(take state) == expected)
    }}
  }}
}}
pub fn main(t: Test) -> Result[(), ()] {{
"#
        );
        let mut rows = vec![
            (b"01234567".repeat(1024), long[0]),
            (vec![b'a'; 1089], long[1]),
        ];
        for len in (0..=33).chain([63, 64, 65, 127, 128, 129, 255, 256, 257]) {
            let data: Vec<_> = (0..len).map(|i| (i * 37 + 128) as u8).collect();
            let expected = checksum(&data, polynomial);
            rows.push((data, expected));
        }
        for (data, expected) in rows {
            let literal: String = data.iter().map(|b| format!("\\x{b:02x}")).collect();
            source += &format!("check(t, b\"{literal}\", {expected}u32)\n");
        }
        source += "Ok(())\n}\n";
        execute(&source);
    });
}

#[test]
fn ieee_list_suffixes_continue_an_existing_checksum() {
    with_compiler_stack(|| {
        // Each suffix has an independent bitwise oracle, including empty and
        // beyond-end starts. A prefix outside the checksum catches offset bugs.
        let data: Vec<_> = (0..=255).collect();
        let mut source = String::from(
            "pub fn main(t: Test) -> Result[(), ()] {\nlet mut data: List[u8] = []\n\
             for i in 0u64..256u64 { core@1/list.push(mut data, core@1/int.truncate(i)) }\n",
        );
        for start in [
            0,
            1,
            7,
            8,
            15,
            16,
            17,
            223,
            224,
            239,
            240,
            241,
            247,
            248,
            249,
            255,
            256,
            257,
            usize::MAX,
        ] {
            let bytes = [b"prefix".as_slice(), &data[start.min(data.len())..]].concat();
            let expected = checksum(&bytes, 0xedb88320);
            source += &format!(
                "{{ let mut state = std@1/crc32.new()\n\
                 std@1/crc32.update(mut state, b\"prefix\")\n\
                 std@1/crc32.update_list(mut state, data, {start}u64)\n\
                 core@1/test.check(t, std@1/crc32.finish(take state) == {expected}u32) }}\n"
            );
        }
        source += "Ok(())\n}\n";
        execute(&source);
    });
}

#[test]
fn ieee_long_go_golden_and_slicing_boundaries() {
    vectors("crc32", 0xedb88320, [0x1010dab0, 0x772d04d7]);
}

#[test]
fn castagnoli_long_go_golden_and_slicing_boundaries() {
    vectors("crc32c", 0x82f63b78, [0x8a11661f, 0x5a6f5c45]);
}
