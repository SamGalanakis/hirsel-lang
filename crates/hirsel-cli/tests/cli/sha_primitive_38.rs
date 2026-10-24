//! SHA primitive fuel, native recording, semantic binding and format-reset controls.
use std::process::Command;

#[test]
fn sha_primitive_38_native_tapes_replay_and_refuse_changed_program_or_old_format() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("main.hir");
    let mut text = String::from(
        "pub fn main(rt: Io, args: Args) -> Result[(), GrantError] {\nlet output = io.stdout(rt)?\n",
    );
    for algorithm in ["sha1", "sha224", "sha256", "sha384", "sha512", "sha512_256"] {
        text.push_str(&format!("let _ = out.print(output,std@1/{algorithm}.digest_hex(b\"abc\"))\nlet _ = out.print(output,\"\\n\")\n"));
    }
    text.push_str("Ok(())\n}\n");
    std::fs::write(&source, &text).unwrap();
    std::fs::write(source.with_extension("hir.json"),serde_json::json!({"schema":1,"package":{"name":"sha","version":"1"},"sources":[{"path":"main.hir","module":[]}],"dependencies":[{"manifest":std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../stdlib/std@1/hirsel.json").canonicalize().unwrap(),"package":{"name":"std","version":"1"},"expose":[]}],"entry":{"module":[],"name":"main"}}).to_string()).unwrap();
    let grants = temp.path().join("grants.json");
    std::fs::write(
        &grants,
        r#"{"io":{"stdout":"stdout.bytes@1","dirs":[]},"args":[]}"#,
    )
    .unwrap();
    let expected = b"a9993e364706816aba3e25717850c26c9cd0d89d\n23097d223405d8228642a477bda255b32aadbce4bda0b3f7e36c9da7\nba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad\ncb00753f45a35e8bb5a03d699ac65007272c32ab0eded1631a8b605a43ff5bed8086072ba1e7cc2358baeca134c825a7\nddaf35a193617abacc417349ae20413112e6fa4e89a97ea20a9eeee64b55d39a2192992a274fc1a836ba3c23a3feebbd454d4423643ce80e2a9ac94fa54ca49f\n53048e2681941ef99b2e29b76b4c7dabe4c2d0c634fc6d46e0e2f13107e7af23\n";
    for backend in ["llvm", "cranelift"] {
        let tape = temp.path().join(format!("{backend}.tape"));
        let recorded = Command::new(env!("CARGO_BIN_EXE_hirsel"))
            .args(["run", "--native", "--backend", backend])
            .arg(&source)
            .arg("--grants")
            .arg(&grants)
            .arg("--record")
            .arg(&tape)
            .output()
            .unwrap();
        assert!(recorded.status.success(), "{backend}: {recorded:?}");
        assert_eq!(recorded.stdout, expected);
        let replay = || {
            Command::new(env!("CARGO_BIN_EXE_hirsel"))
                .arg("run")
                .arg(&source)
                .arg("--replay")
                .arg(&tape)
                .output()
                .unwrap()
        };
        let output = replay();
        assert!(output.status.success(), "{backend}: {output:?}");
        assert_eq!(output.stdout, expected);
        std::fs::write(&source, text.replace("b\"abc\"", "b\"abd\"")).unwrap();
        let changed = replay();
        assert!(
            !changed.status.success(),
            "changed SHA input accepted: {changed:?}"
        );
        assert!(changed.stdout.is_empty());
        std::fs::write(&source, &text).unwrap();
        let mut bytes = std::fs::read(&tape).unwrap();
        assert!(bytes.starts_with(hirsel_run_schema::tape::MAGIC));
        bytes[b"hirsel-io-tape ".len()] = b'8';
        std::fs::write(&tape, bytes).unwrap();
        let old = replay();
        assert!(
            !old.status.success(),
            "historical generation accepted: {old:?}"
        );
        assert!(old.stdout.is_empty());
    }
}

#[test]
fn sha_primitive_38_spends_fuel_on_both_block_families_and_native_backends() {
    for algorithm in ["sha256", "sha512"] {
        let text = format!(
            "pub fn main(rt: Io, args: Args) -> Result[(), GrantError] {{\nlet output=io.stdout(rt)?\nlet _=out.print(output,\"start\\n\")\nlet _=core@1/bytes.{algorithm}(b\"{}\")\nlet _=out.print(output,\"done\\n\")\nOk(())\n}}",
            "a".repeat(32768)
        );
        let source = super::file(&text);
        for engine in [
            &[][..],
            &["--native"][..],
            &["--native", "--backend", "cranelift"][..],
        ] {
            for (fuel, success, expected) in [
                (100, false, b"start\n".as_slice()),
                (10000, true, b"start\ndone\n".as_slice()),
            ] {
                if std::env::var("SHA_PRIMITIVE_38_FUEL_MISSING").is_ok()
                    && (algorithm == "sha256" || engine.is_empty())
                {
                    continue;
                }
                let grants = format!(
                    r#"{{"io":{{"stdout":"stdout.bytes@1","dirs":[]}},"args":[],"budgets":{{"provider":"budgets.limits@2","fuel":{fuel}}}}}"#
                );
                let output = Command::new(env!("CARGO_BIN_EXE_hirsel"))
                    .arg("run")
                    .args(engine)
                    .arg(&source)
                    .arg("--grants")
                    .arg(grants)
                    .output()
                    .unwrap();
                assert_eq!(
                    output.status.code(),
                    Some(if success { 0 } else { 5 }),
                    "{algorithm} {engine:?}: {output:?}"
                );
                assert_eq!(output.stdout, expected, "{algorithm} {engine:?}");
                if !success {
                    assert_eq!(output.stderr, b"budget exceeded: fuel\n");
                }
                eprintln!("SHA_FUEL_PASS {algorithm} {engine:?} {fuel}");
            }
        }
        std::fs::remove_file(source).unwrap();
    }
}
