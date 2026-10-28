fn matmul_stage_probes(source: &str) -> String {
    let probes = [
        (
            "          shared.freeze(take transposed)",
            r#"          failure.check(list.len(transposed) == n, Bounds())
          for j in 0u64..n {
            failure.check(list.len(transposed[j]) == n, Bounds())
            for i in 0u64..n {
              failure.check(core@1/f64.bits(transposed[j][i]) == core@1/f64.bits(matrix[i][j]), Arithmetic())
            }
          }
          shared.freeze(take transposed)"#,
        ),
        (
            "        list.push(mut jobs, Job(",
            r#"        failure.check(list.len(output) == end - first, Bounds())
        for row in output {
          failure.check(list.len(row) == n, Bounds())
          for cell in row { failure.check(core@1/f64.bits(cell) == 0u64, Arithmetic()) }
        }
        list.push(mut jobs, Job("#,
        ),
        (
            "      shared.freeze(take output)",
            r#"      failure.check(list.len(output) == moved_end - moved_first, Bounds())
      let n = shared.with(moved_a, fn(a: Matrix) -> u64 uses {} { list.len(a) })
      for row in output { failure.check(list.len(row) == n, Bounds()) }
      shared.freeze(take output)"#,
        ),
        (
            "  let mut c: Matrix = []",
            r#"  failure.check(list.len(chunks) == int.widen(width), Bounds())
  let mut c: Matrix = []"#,
        ),
        (
            "  Ok(take c)",
            r#"  let n = shared.with(borrowed_a, fn(a: Matrix) -> u64 uses {} { list.len(a) })
  failure.check(list.len(c) == n, Bounds())
  for row in c { failure.check(list.len(row) == n, Bounds()) }
  Ok(take c)"#,
        ),
    ];
    let mut source = source.to_owned();
    for (anchor, probe) in probes {
        assert_eq!(
            source.matches(anchor).count(),
            1,
            "Matmul stage anchor {anchor}"
        );
        source = source.replace(anchor, probe);
    }
    source
}

fn matmul_job_probes(root: &std::path::Path, width: u32, batches: usize) {
    use std::collections::BTreeSet;
    check_cpu_lifecycle(
        &root.join("stages.probe.lifecycle"),
        width as usize,
        false,
        batches,
    );
    let controls = fs::read_to_string(root.join("stages.probe.control")).unwrap();
    let mut identities = BTreeSet::new();
    let mut joined = 0;
    for line in controls.lines() {
        let fields: Vec<_> = line.split_whitespace().collect();
        assert_eq!(fields.len(), 6);
        let row: Vec<u64> = fields[1..]
            .iter()
            .map(|field| field.parse().unwrap())
            .collect();
        assert_eq!(row[1], u64::from(width));
        assert_eq!(
            row[4], 0,
            "Matmul must not withdraw or refuse a job: {line}"
        );
        match row[2] {
            5 => {
                assert!(identities.insert(row[3]));
            }
            1 => {
                assert_eq!(identities, (0..u64::from(width)).collect());
                identities.clear();
                joined += 1;
            }
            _ => {}
        }
    }
    assert_eq!(joined, batches);
    assert!(identities.is_empty());
}
