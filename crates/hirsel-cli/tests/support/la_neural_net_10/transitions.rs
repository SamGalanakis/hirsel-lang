#[path = "reference.rs"]
mod reference;
use std::{path::PathBuf, process::Command};

const BEFORE: &str = include_str!("before.hir");
const AFTER: &str = include_str!("../../../../../bench/langarena/neural_net.hir");

fn source(kernel: &str, points: &[usize]) -> String {
    // Observe the actual kernel and initialization, replacing only measure.
    let (kernel, execute) = kernel.split_once("pub fn execute").unwrap();
    let init = execute
        .split_once("  let mut s =")
        .unwrap()
        .1
        .split_once("  harness.measure")
        .unwrap()
        .0;
    let mut result = format!(
        "{kernel}\n{}\npub fn main(rt: Io) -> Result[(), String] {{\n let out = match io.stdout(rt) {{ Ok(o) => o, Error(_) => return Error(\"stdout\"), }}\n  let mut s ={init}",
        include_str!("snapshot.hir")
    );
    let mut previous = 0;
    for point in points {
        if *point > previous {
            result.push_str(&format!(" for i in {previous}u64..{point}u64 {{\n match i % 4u64 {{\n 0u64 => train(mut s, 0.0, 0.0, 0.0),\n 1u64 => train(mut s, 1.0, 0.0, 1.0),\n 2u64 => train(mut s, 0.0, 1.0, 1.0),\n _ => train(mut s, 1.0, 1.0, 0.0),\n }}\n }}\n"));
        }
        result.push_str(" snapshot(out, s) ?\n");
        previous = *point;
    }
    result.push_str(" Ok(())\n}\n");
    result
}

struct Fixture(PathBuf);
impl Fixture {
    fn new(name: &str, program: &str) -> Self {
        let dir =
            std::env::temp_dir().join(format!("la-neural-net-10-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("main.hir"), program).unwrap();
        std::fs::write(
            dir.join("harness.hir"),
            include_str!("../../../../../bench/langarena/harness.hir"),
        )
        .unwrap();
        let stdlib = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../stdlib/std@1/hirsel.json")
            .canonicalize()
            .unwrap();
        let manifest = serde_json::json!({"schema":1,"package":{"name":"la_neural_net_10","version":"1"},"entry":{"module":[],"name":"main"},"sources":[{"path":"main.hir","module":[]},{"path":"harness.hir","module":["harness"]}],"dependencies":[{"manifest":stdlib,"package":{"name":"std","version":"1"},"expose":[]}]});
        std::fs::write(dir.join("hirsel.json"), manifest.to_string()).unwrap();
        std::fs::write(
            dir.join("grants.json"),
            r#"{"io":{"stdout":"stdout.bytes@1","dirs":[]},"args":[]}"#,
        )
        .unwrap();
        Self(dir)
    }
    fn run(&self, backend: Option<&str>, width: usize) -> Vec<Vec<u64>> {
        let mut command = if let Some(backend) = backend {
            let binary = self.0.join(format!("native-{backend}"));
            let built = Command::new(env!("CARGO_BIN_EXE_hirsel"))
                .args(["build", "--backend", backend])
                .arg(if backend == "llvm" {
                    "--release"
                } else {
                    "--debug"
                })
                .arg(self.0.join("hirsel.json"))
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
            run.arg("--hirsel-grants").arg(self.0.join("grants.json"));
            run
        } else {
            let mut run = Command::new(env!("CARGO_BIN_EXE_hirsel"));
            run.args(["run", "--max-steps", "1000000", "--grants"])
                .arg(self.0.join("grants.json"))
                .arg(self.0.join("hirsel.json"));
            run
        };
        let output = command
            .env("HIRSEL_DAEMON", "off")
            .env("HIRSEL_STD_OBJECTS", "none")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(output.stdout.len() % 8, 0);
        let words: Vec<u64> = output
            .stdout
            .chunks_exact(8)
            .map(|s| u64::from_le_bytes(s.try_into().unwrap()))
            .collect();
        assert_eq!(words.len() % width, 0);
        words.chunks_exact(width).map(<[u64]>::to_vec).collect()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}

#[test]
fn initialization_and_ordered_first_four_updates() {
    let points = [0, 1, 2, 3, 4, 8];
    let before = Fixture::new("before", &source(BEFORE, &points)).run(None, 112);
    let after = Fixture::new("after", &source(AFTER, &points)).run(None, 112);
    assert_eq!(before, after, "every IEEE bit must survive access changes");
    let rust = reference::vectors(&points);
    assert_eq!(
        after[0], rust[0],
        "seed, initialization draw order and mapping"
    );
    for (step, (actual, expected)) in after.iter().zip(&rust).enumerate() {
        for (field, (&a, &e)) in actual.iter().zip(expected).enumerate() {
            assert!(f64::from_bits(a).is_finite() && f64::from_bits(e).is_finite());
            // Hirsel's fdlibm exp (<1 ULP, std/f64_exp.hir:32) and the
            // host's Rust exp differ on these three propagated fields only.
            // Pin the baseline discrepancy, rather than relaxing every field.
            let libm_field = matches!((points[step], field), (2, 38 | 39) | (8, 14));
            assert!(
                a == e || (libm_field && a.abs_diff(e) <= 2),
                "updates={} field={field}: {a:#x} vs {e:#x}",
                points[step]
            );
        }
    }
    // test.js's two iterations follow one warmup, retaining state.
    assert_eq!(reference::checksums()[2], 1_586_674_334);
}

#[test]
fn repeated_epochs_llvm_and_cranelift() {
    let points = [4000, 8000, 12000];
    let with_checksum = |kernel| {
        source(kernel, &points).replace(
            " snapshot(out, s) ?",
            " snapshot(out, s) ?\n checksum_word(out, s) ?",
        ) + "\nfn checksum_word(out: Stdout, s: State) -> Result[(), String] {\n let mut data: List[u8] = []\n bits(mut data, float.from_int(checksum(s)))\n match out.write(out, bytes.from_list(take data)) { Ok(_) => Ok(()), Error(_) => Error(\"stdout\"), }\n}\n"
    };
    let expected = reference::checksums();
    let mut llvm = None;
    for backend in ["llvm", "cranelift"] {
        let before = Fixture::new(&format!("epochs-before-{backend}"), &with_checksum(BEFORE))
            .run(Some(backend), 113);
        let after = Fixture::new(&format!("epochs-after-{backend}"), &with_checksum(AFTER))
            .run(Some(backend), 113);
        assert_eq!(before.len(), points.len());
        assert_eq!(
            before, after,
            "{backend} must retain every state bit across epochs"
        );
        for (state, &checksum) in after.iter().zip(&expected) {
            assert_eq!(state[112], f64::from(checksum).to_bits());
        }
        if let Some(llvm) = &llvm {
            assert_eq!(&after, llvm);
        } else {
            llvm = Some(after);
        }
    }
}
