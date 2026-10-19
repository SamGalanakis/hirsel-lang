//! Oracle O1's salt half: what the compiler prints does not depend on how
//! session ids (Symbol, DefId) were assigned. Each salt runs in its own process.
use hirsel_driver::*;
use std::path::{Path, PathBuf};

/// Every 149th corpus program in path order: a fixed sample across every chapter.
fn sample() -> Vec<PathBuf> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        let mut entries: Vec<_> = std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().path())
            .collect();
        entries.sort();
        for path in entries {
            if path.is_dir() {
                walk(&path, out);
            } else if path.extension().is_some_and(|e| e == "hir") {
                out.push(path);
            }
        }
    }
    let mut all = vec![];
    walk(
        &PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../conformance"),
        &mut all,
    );
    all.into_iter().step_by(149).collect()
}

/// Each sampled program's check diagnostics (JSON, as `hirsel check --json`
/// prints them) and, when it is accepted, its TIR export, in sample order.
fn transcript() -> String {
    let mut out = String::new();
    for path in sample() {
        out.push_str(&format!("== {}\n", path.display()));
        let program = hirsel_engine::with_compiler_stack(|| {
            let mut program = load_program(&path)?;
            let checked = program
                .check(hirsel_engine::LoweringPolicy::Comptime)
                .map_err(|error| program.error(error))?;
            let mut text = format!("completion {:?}\n", checked.completion);
            for diagnostic in &checked.diagnostics {
                text.push_str(&diagnostic.to_json(&checked.sources).to_string());
                text.push('\n');
            }
            if program.check_for_execution().is_ok() {
                let entry = program.entry.clone();
                match program.engine.export_tir(&entry) {
                    Ok(tir) => text.push_str(&tir),
                    Err(error) => {
                        text.push_str(&format!("export error: {}\n", program.error(error)))
                    }
                }
            }
            Ok::<_, hirsel_engine::ToolError>(text)
        });
        match program {
            Ok(text) => out.push_str(&text),
            Err(error) => out.push_str(&format!("error: {error}\n")),
        }
    }
    out
}

#[test]
fn diagnostics_and_tir_export_do_not_depend_on_id_assignment() {
    let Some(runs) = hirsel_intern::under_salts(
        "id_salt::diagnostics_and_tir_export_do_not_depend_on_id_assignment",
        &[1, 2, 3],
        transcript,
    ) else {
        return;
    };
    // The sample holds rejected programs and accepted ones with a TIR export.
    assert!(runs[0].contains("\"severity\":\"error\""));
    assert!(
        runs[0].matches("(Export\n").count() >= 10,
        "too few TIR exports:\n{}",
        runs[0]
    );
    for (seed, run) in [2, 3].into_iter().zip(&runs[1..]) {
        if let Some((line, (a, b))) = runs[0]
            .lines()
            .zip(run.lines())
            .enumerate()
            .find(|(_, (a, b))| a != b)
        {
            panic!("salt {seed} differs from salt 1 at line {line}:\n  1: {a}\n  {seed}: {b}");
        }
        assert_eq!(
            run.len(),
            runs[0].len(),
            "salt {seed} output length differs"
        );
    }
}
