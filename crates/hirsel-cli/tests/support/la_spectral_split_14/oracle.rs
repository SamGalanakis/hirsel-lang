use std::{path::PathBuf, process::Command};

const AFTER: &str = include_str!("../../../../../bench/langarena/clbg.hir");
const BEFORE: &str = include_str!("before.hir");

// Independent upstream eval_a_times_u/eval_at_times_u: map in j order, sum.
// Keep the floating denominator from spectralnorm.rs:21-24, not integer algebra.
fn reference(u: &[f64], transposed: bool) -> Vec<f64> {
    (0..u.len())
        .map(|i| {
            u.iter()
                .enumerate()
                .map(|(j, &v)| {
                    let (row, column) = if transposed { (j, i) } else { (i, j) };
                    let ij = (row + column) as f64;
                    (1.0 / (ij * (ij + 1.0) / 2.0 + row as f64 + 1.0)) * v
                })
                .fold(0.0, |total, value| total + value)
        })
        .collect()
}

fn vectors() -> Vec<Vec<f64>> {
    let mut vectors = vec![
        vec![],
        vec![1.0],
        vec![-0.0],
        vec![1.0, 1.0],
        vec![3.0, -2.0, 0.5, 7.0],
        vec![1e100, -1e100, 1e-100, 0.25],
        vec![f64::from_bits(1), -f64::from_bits(1), 0.0],
    ];
    for n in [3, 7, 8, 9, 15, 16, 17, 31, 32, 33, 44, 45, 46] {
        vectors.push((0..n).map(|i| (i % 7) as f64 * 0.25 - 0.75).collect());
    }
    vectors
}

fn program() -> String {
    // Observe the shipped module's private functions and actual step/checksum.
    let kernel = AFTER.split("pub fn execute").next().unwrap();
    let before = BEFORE
        .replace("fn a(", "fn before_a(")
        .replace("a(j, i)", "before_a(j, i)")
        .replace("a(i, j)", "before_a(i, j)")
        .replace("multiply(", "before_multiply(")
        .replace("ata(", "before_ata(");
    let mut text = format!(
        "{kernel}\n{before}\n{}\npub fn main(rt: Io) -> Result[(), String] {{\n let Ok(out) = io.stdout(rt) else {{ return Error(\"stdout\") }}\n let mut data: List[u8] = []\n",
        include_str!("snapshot.hir")
    );
    for (vector_index, u) in vectors().into_iter().enumerate() {
        let values = u
            .iter()
            .map(|v| format!("core@1/f64.from_bits({}u64)", v.to_bits()))
            .collect::<Vec<_>>()
            .join(", ");
        text.push_str(&format!(
            " let vector_{vector_index}: List[f64] = [{values}]\n"
        ));
        for (result_index, expression) in [
            "before_multiply(u, false)",
            "multiply(u)",
            "before_multiply(u, true)",
            "multiply_transposed(u)",
            "before_ata(u)",
            "ata(u)",
        ]
        .into_iter()
        .enumerate()
        {
            let expression = expression.replace("(u", &format!("(vector_{vector_index}"));
            text.push_str(&format!(
                " let result_{vector_index}_{result_index} = {expression}\n word(mut data, list.len(result_{vector_index}_{result_index}))\n for value in result_{vector_index}_{result_index} {{ word(mut data, core@1/f64.bits(value)) }}\n"
            ));
        }
    }
    // test.js size=45, ten measured iterations following two warmups.
    // Run the actual step, preserving its ata sequence and temporary lifetimes.
    text.push_str(
        r#"
 let mut ones: List[f64] = []
 for _ in 0u64..45u64 { list.push(mut ones, 1.0) }
 let planets = create_bodies()
 let initial_energy = energy(planets)
 let mut s = State(name: "CLBG::Spectralnorm", n: 0, w: 0, h: 0,
   value_out: 0, pixels: [], u: copy ones, v: copy ones, planets: planets,
   initial_energy: initial_energy)
 for i in 0u64..12u64 { step(mut s, i) }
 word(mut data, int.widen(checksum(s)))
 let mut u = copy ones
 let mut v = take ones
 for _ in 0u64..12u64 {
   v = before_ata(u)
   u = before_ata(v)
 }
 s.u = take u
 s.v = take v
 word(mut data, int.widen(checksum(s)))
 match out.write(out, bytes.from_list(take data)) {
   Ok(_) => Ok(()), Error(_) => Error("stdout"),
 }
}
"#,
    );
    text
}

struct Fixture(tempfile::TempDir);
impl Fixture {
    fn new() -> Self {
        let dir = tempfile::Builder::new()
            .prefix("la-spectral-split-14-")
            .tempdir()
            .unwrap();
        std::fs::write(dir.path().join("main.hir"), program()).unwrap();
        std::fs::write(
            dir.path().join("harness.hir"),
            include_str!("../../../../../bench/langarena/harness.hir"),
        )
        .unwrap();
        let stdlib = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../stdlib/std@1/hirsel.json")
            .canonicalize()
            .unwrap();
        let manifest = serde_json::json!({"schema":1,"package":{"name":"la_spectral_split_14","version":"1"},"entry":{"module":[],"name":"main"},"sources":[{"path":"main.hir","module":[]},{"path":"harness.hir","module":["harness"]}],"dependencies":[{"manifest":stdlib,"package":{"name":"std","version":"1"},"expose":[]}]});
        std::fs::write(dir.path().join("hirsel.json"), manifest.to_string()).unwrap();
        std::fs::write(
            dir.path().join("grants.json"),
            r#"{"io":{"stdout":"stdout.bytes@1","dirs":[]},"args":[]}"#,
        )
        .unwrap();
        Self(dir)
    }
    fn words(&self, backend: Option<&str>) -> Vec<u64> {
        let mut command = if let Some(backend) = backend {
            let binary = self.0.path().join("native");
            let built = Command::new(env!("CARGO_BIN_EXE_hirsel"))
                .args(["build", "--backend", backend])
                .arg(if backend == "llvm" {
                    "--release"
                } else {
                    "--debug"
                })
                .arg(self.0.path().join("hirsel.json"))
                .arg("-o")
                .arg(&binary)
                .env("HIRSEL_DAEMON", "off")
                .env("HIRSEL_STD_OBJECTS", "none")
                .output()
                .unwrap();
            assert!(
                built.status.success(),
                "{backend}: {}",
                String::from_utf8_lossy(&built.stderr)
            );
            let mut run = Command::new(binary);
            run.arg("--hirsel-grants")
                .arg(self.0.path().join("grants.json"));
            run
        } else {
            let mut run = Command::new(env!("CARGO_BIN_EXE_hirsel"));
            run.args(["run", "--max-steps", "unbounded", "--grants"])
                .arg(self.0.path().join("grants.json"))
                .arg(self.0.path().join("hirsel.json"));
            run
        };
        let output = command.env("HIRSEL_DAEMON", "off").output().unwrap();
        assert!(
            output.status.success(),
            "{backend:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(output.stderr.is_empty(), "{output:?}");
        assert_eq!(output.stdout.len() % 8, 0);
        output
            .stdout
            .chunks_exact(8)
            .map(|b| u64::from_le_bytes(b.try_into().unwrap()))
            .collect()
    }
}

fn check(backend: Option<&str>) {
    let words = Fixture::new().words(backend);
    let mut actual = words.as_slice();
    for u in vectors() {
        let forward = reference(&u, false);
        let transposed = reference(&u, true);
        let ata = reference(&forward, true);
        for expected in [&forward, &forward, &transposed, &transposed, &ata, &ata] {
            assert_eq!(actual[0], expected.len() as u64);
            actual = &actual[1..];
            let bits: Vec<_> = expected
                .iter()
                .map(|v| {
                    assert!(v.is_finite());
                    v.to_bits()
                })
                .collect();
            assert_eq!(&actual[..bits.len()], bits, "{backend:?}, input={u:?}");
            actual = &actual[bits.len()..];
        }
    }
    // Independent hand-derived orientation probe, in addition to upstream oracle.
    assert_eq!(reference(&[1.0, 1.0], false), [1.5, 1.0 / 3.0 + 0.2]);
    assert_eq!(reference(&[1.0, 1.0], true), [1.0 + 1.0 / 3.0, 0.7]);
    assert_eq!(actual, [1_052_168_784, 1_052_168_784]);
}

#[test]
fn la_spectral_split_14_h_exact_bits_and_checksum() {
    check(None);
}
#[test]
fn la_spectral_split_14_llvm_exact_bits_and_checksum() {
    check(Some("llvm"));
}
#[test]
fn la_spectral_split_14_cranelift_exact_bits_and_checksum() {
    check(Some("cranelift"));
}
