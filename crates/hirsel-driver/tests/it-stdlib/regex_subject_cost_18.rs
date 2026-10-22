//! Private engine differential checks run without exporting selectors from std.
use hirsel_driver::*;
use hirsel_engine::*;
use std::path::{Path, PathBuf};

fn private_fixture(directory: &Path) -> PathBuf {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let std_root = root.join("stdlib/std@1").canonicalize().unwrap();
    let mut manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(std_root.join("hirsel.json")).unwrap()).unwrap();
    // Check the unchanged regex owners as a private root, not every exported
    // std module. Keep exact source builders for their only text dependency.
    manifest["sources"]
        .as_array_mut()
        .unwrap()
        .retain(|source| {
            let module = source["module"].as_array().unwrap();
            module == &vec![serde_json::json!("regex")]
                || module == &vec![serde_json::json!("ascii")]
                || module
                    == &vec![
                        serde_json::json!("internal"),
                        serde_json::json!("substring"),
                    ]
        });
    for source in manifest["sources"].as_array_mut().unwrap() {
        source["path"] = std_root
            .join(source["path"].as_str().unwrap())
            .into_os_string()
            .into_string()
            .unwrap()
            .into();
    }
    let text = std::fs::read_to_string(std_root.join("text.hir")).unwrap();
    let builders = text.split("// Pads the text").next().unwrap();
    std::fs::write(directory.join("text.hir"), builders).unwrap();
    manifest["sources"]
        .as_array_mut()
        .unwrap()
        .push(serde_json::json!({
            "path": directory.join("text.hir"), "module": ["text"]
        }));
    std::fs::write(directory.join("subject.hir"), SUBJECT_VECTORS).unwrap();
    manifest["sources"]
        .as_array_mut()
        .unwrap()
        .push(serde_json::json!({
            "path": directory.join("subject.hir"), "module": ["regex"]
        }));
    manifest["entry"] = serde_json::json!({"module": ["regex"], "name": "subject_small"});
    let path = directory.join("hirsel.json");
    std::fs::write(&path, serde_json::to_vec_pretty(&manifest).unwrap()).unwrap();
    path
}

fn check_subject(backend: Option<native::Backend>, entries: &[&str]) {
    with_compiler_stack(|| {
        let temp = tempfile::tempdir().unwrap();
        let path = private_fixture(temp.path());
        let mut manifest: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        for &entry in entries {
            eprintln!("subject {entry} {backend:?}");
            manifest["entry"]["name"] = entry.into();
            std::fs::write(&path, serde_json::to_vec(&manifest).unwrap()).unwrap();
            let outcome = match backend {
                None => run_path_with_step_budget(
                    &path,
                    SimulationConfig::default(),
                    Some(100_000_000),
                    Verify::On,
                    &mut Vec::new(),
                    &mut Vec::new(),
                )
                .unwrap(),
                Some(selected) => native::run_path_native_backend(
                    &path,
                    HostLaunch::empty(),
                    None,
                    selected,
                    native::Output::Streams {
                        stdout: &mut Vec::new(),
                        stderr: &mut Vec::new(),
                    },
                    Verify::On,
                )
                .unwrap()
                .unwrap(),
            };
            assert!(
                matches!(
                    outcome,
                    RunOutcome::Returned(hirsel_run_schema::Returned::Ok)
                ),
                "{entry} {backend:?}: {outcome:?}"
            );
        }
    });
}

#[test]
fn subject_cost_interpreter() {
    check_subject(None, &["subject_search"]);
}
#[test]
fn subject_cost_llvm() {
    check_subject(Some(native::Backend::Llvm), &["subject_search"]);
}
#[test]
fn subject_cost_cranelift() {
    check_subject(Some(native::Backend::Cranelift), &["subject_search"]);
}

// Keep construction independent so a search-only repair need not repeat the
// already-proved constructor matrix and its large boundary cases.
#[test]
fn subject_constructor_interpreter() {
    check_subject(None, &["subject_small", "subject_bounds"]);
}
#[test]
fn subject_constructor_llvm() {
    check_subject(
        Some(native::Backend::Llvm),
        &["subject_small", "subject_bounds"],
    );
}
#[test]
fn subject_constructor_cranelift() {
    check_subject(
        Some(native::Backend::Cranelift),
        &["subject_small", "subject_bounds"],
    );
}

const SUBJECT_VECTORS: &str = r#"// rust-lang/regex 80b9d14e: utf8.toml:1-60, iter.toml:1-90,
// regression.toml:65-110,767-800. Scalar/non-overlapping semantics only.
@serializable
pub type SubjectFailure { stage: u64 }

fn subject_constructor_case(src: String, budget: u64) -> bool uses {} {
  let mut actual_work = budget
  let actual = make_subject(src, mut actual_work)
  let mut expected_work = budget
  let expected = subject_oracle(src, mut expected_work)
  actual == expected && actual_work == expected_work
}

pub fn subject_small() -> Result[(), SubjectFailure] uses {} {
  let samples: List[String] = ["", "abc", "☃", "a☃b", "Kkſs", "\r\n", "________________________________________________________________", "________________________________________________________________abc", "________________________________________________________________☃", "☃________________________________________________________________"]
  for src in samples {
    let mut budget = 0u64
    while budget <= 80 {
      if !subject_constructor_case(src, budget) { return Error(SubjectFailure(stage: 1)) }
      budget += 1
    }
  }
  // Every ASCII code point, including NUL, CR/LF and DEL.
  let mut bytes: List[u8] = []
  for i in 0u64..128 { core@1/list.push(mut bytes, core@1/int.narrow(i)) }
  if !subject_constructor_case(text.finish_valid(take bytes), 129) { return Error(SubjectFailure(stage: 2)) }
  Ok(())
}

pub fn subject_bounds() -> Result[(), SubjectFailure] uses {} {
  let lengths: List[u64] = [63,64,65,16383,16384,16385]
  for n in lengths {
    let mut bytes: List[u8] = []
    for i in 0u64..n { core@1/list.push(mut bytes, 97u8) }
    let src = text.finish_valid(take bytes)
    let budgets: List[u64] = [0,n-1,n,n+1,n+2,18446744073709551615]
    for budget in budgets {
      if !subject_constructor_case(src, budget) { return Error(SubjectFailure(stage: 3)) }
    }
  }
  Ok(())
}

fn subject_search_case(pattern: String, tail: String) -> bool uses {} {
  let mut bytes: List[u8] = []
  text.append(mut bytes, "________________________________________________________________")
  text.append(mut bytes, tail)
  let src = text.finish_valid(take bytes)
  let Ok(re) = compile(pattern, 10000) else { return false }
  let mut work = 18446744073709551615u64
  let Ok(subject) = make_subject(src, mut work) else { return false }
  let mut old_work = 18446744073709551615u64
  let Ok(old_subject) = subject_oracle(src, mut old_work) else { return false }
  let haystack = core@1/string.as_bytes(src)
  let mut cache = dfa_cache(re, true, 65024)
  let mut tiny = dfa_cache(re, true, 1)
  let mut captures = dfa_cache(re, true, 65024)
  let mut scratch = capture_scratch()
  let starts: List[u64] = [0,32,64,core@1/list.len(subject.scalars)]
  for start in starts {
    let mut oracle_work = old_work
    let expected = pike_find_from(re, old_subject, start, false, mut oracle_work)
    let mut actual_work = work
    let actual = find_from(re, subject, start, true, haystack, mut cache, mut actual_work)
    if actual != expected { return false }
    let mut retry_work = work
    let retry = find_from(re, subject, start, true, haystack, mut tiny, mut retry_work)
    if retry != expected || retry_work != oracle_work { return false }
    oracle_work = old_work
    let expected_capture = pike_find_from(re, old_subject, start, true, mut oracle_work)
    actual_work = work
    let actual_capture = capture_find_from(re, subject, start, re.prefilter.enabled, haystack, mut captures, mut scratch, mut actual_work)
    if actual_capture != expected_capture { return false }
    // Non-admitted calls, even after a quit, retain exact Pike charges.
    actual_work = 100
    oracle_work = 100
    if find_from(re, subject, start, false, haystack, mut tiny, mut actual_work) != pike_find_from(re, old_subject, start, false, mut oracle_work) || actual_work != oracle_work { return false }
  }
  true
}

pub fn subject_search() -> Result[(), SubjectFailure] uses {} {
  let vectors: List[(String,String)] = [
    ("", "☃"), ("a", "aaa"), ("a", "aba"), ("(?:)*", "abc"),
    ("b|(?:)+", "abc"), ("ab?|$", "az"), ("z*azb", "azb"),
    ("([a-f]){2}(?P<foo>[x-z])", "abx"), ("(a){0}(a)", "a"),
    ("a|ab", "ab"), ("ab|a", "ab"), ("(?i)(kelvin|longs)", "Kelvin longſ kelvin longs"),
    ("\\b([0-9]+)\\b", "β123 123β 123"), ("(?m)^abc$", "\nabc\n"),
    ("(abcdefg|abcdefgh)", "abcdefgh"), ("(a+?)(a*)", "aaaa")
  ]
  for (pattern, tail) in vectors {
    if !subject_search_case(pattern,tail) { return Error(SubjectFailure(stage: 4)) }
  }
  Ok(())
}

fn subject_oracle(text: String, mut work: u64) -> Result[Input, MatchError] uses {} {
  let mut source_input = Input(scalars: [], offsets: [], at: 0)
  let mut byte = 0u64
  for c in core@1/string.scalars(text) {
    spend(mut work)?
    core@1/list.push(mut source_input.scalars, c)
    core@1/list.push(mut source_input.offsets, byte)
    byte += scalar_width(c)
  }
  spend(mut work)?
  core@1/list.push(mut source_input.offsets, byte)
  Ok(take source_input)
}
"#;
