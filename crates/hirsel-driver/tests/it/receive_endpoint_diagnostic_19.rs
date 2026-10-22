//! Context-fixed invariant operands keep MODE-2 but explain the endpoint contract.
use hirsel_driver::*;
use hirsel_engine::*;

const DIRECT: &str = "// café 🐑\npub type Status { url: String }\npub fn main(tasks: Tasks, mut incoming: Receiver[unique Status], mut rows: List[Status]) -> Result[(), ChannelError] {\n  list.push(mut rows, channel.receive(tasks, mut incoming)?)\n  Ok(())\n}\n";

fn checked(source: &str) -> CheckOutput {
    let mut engine = crate::source_std::engine();
    let file = engine.sources.add("endpoint.hir", source.into()).unwrap();
    let output = check_progress(&mut engine, file, &default_context()).unwrap();
    assert_eq!(output.completion, CheckCompletion::Complete);
    let warm = check_progress(&mut engine, file, &default_context()).unwrap();
    assert_eq!(
        output
            .diagnostics
            .iter()
            .map(|d| d.to_json(&output.sources))
            .collect::<Vec<_>>(),
        warm.diagnostics
            .iter()
            .map(|d| d.to_json(&warm.sources))
            .collect::<Vec<_>>()
    );
    output
}

fn snapshot(source: &str, expected: serde_json::Value) -> CheckOutput {
    let output = checked(source);
    let [d] = &output.diagnostics[..] else {
        panic!("{:?}", output.diagnostics)
    };
    assert_eq!(d.rule.as_str(), "MODE-2");
    assert!(d.fix.is_none());
    let json = d.to_json(&output.sources);
    // Snapshot source fragments as well as words: no path or byte-offset normalization
    // can hide a label attached to the wrong expression (including after UTF-8).
    let snapshot = serde_json::json!({
        "message": json["message"],
        "primary": output.sources.slice(d.primary),
        "secondary": d.secondary.iter().map(|l| serde_json::json!([
            output.sources.slice(l.span), l.message
        ])).collect::<Vec<_>>(),
        "notes": json["notes"],
    });
    assert_eq!(snapshot, expected);
    let human = d.render_human(&output.sources);
    assert!(human.contains(d.message.as_str()));
    for label in &d.secondary {
        assert!(human.contains(&label.message));
    }
    for note in json["notes"].as_array().unwrap() {
        assert!(human.contains(note.as_str().unwrap()));
    }
    output
}

#[test]
fn receive_endpoint_diagnostic_19_receiver_snapshot() {
    with_compiler_stack(|| {
        snapshot(
            DIRECT,
            serde_json::json!({
                "message": "`channel.receive` requires `Receiver[Status]` here because the `value` argument of `list.push` expects `Status`; `incoming` has `Receiver[unique Status]`. Mutable or invariant payload contracts must match.",
                "primary": "mut incoming",
                "secondary": [
                    ["mut incoming", "provided: `incoming` has `Receiver[unique Status]`"],
                    ["channel.receive", "required: `channel.receive` requires `Receiver[Status]` under this result context"],
                    ["list.push", "required: the `value` argument of `list.push` expects `Status`"]
                ],
                "notes": ["help: Bind the result first (`let row = channel.receive(tasks, mut incoming)?`), then pass `take row` to `list.push`."]
            }),
        );
    });
}

#[test]
fn receive_endpoint_diagnostic_19_sender_snapshot() {
    with_compiler_stack(|| {
        let source = DIRECT.replace("Receiver", "Sender").replace(
            "channel.receive(tasks, mut incoming)",
            "from_sender(mut incoming)",
        );
        let source = format!(
            "fn from_sender[T](mut outgoing: Sender[T]) -> Result[T, ChannelError] {{ Error(Closed()) }}\n{source}"
        );
        snapshot(
            &source,
            serde_json::json!({
                "message": "`from_sender` requires `Sender[Status]` here because the `value` argument of `list.push` expects `Status`; `incoming` has `Sender[unique Status]`. Mutable or invariant payload contracts must match.",
                "primary": "mut incoming",
                "secondary": [
                    ["mut incoming", "provided: `incoming` has `Sender[unique Status]`"],
                    ["from_sender", "required: `from_sender` requires `Sender[Status]` under this result context"],
                    ["list.push", "required: the `value` argument of `list.push` expects `Status`"]
                ],
                "notes": ["help: Bind the result first (`let row = from_sender(mut incoming)?`), then pass `take row` to `list.push`."]
            }),
        );
    });
}

#[test]
fn receive_endpoint_diagnostic_19_real_count_conflict_keeps_copy_advice() {
    with_compiler_stack(|| {
        let source = "fn need(take xs: unique List[u64]) {}\nfn probe() { let xs = [1u64]; let alias = copy xs; need(take xs); let _ = alias }";
        snapshot(
            source,
            serde_json::json!({
                "message": "this operation requires a unique value: `xs` lost its deep uniqueness, but a unique value is required",
                "primary": "need(take xs)",
                "secondary": [
                    ["take xs", "parameter mode requirement"],
                    ["copy xs", "provided: copying `xs` shares counted storage reachable from `xs`"],
                    ["need", "required: this requires a unique value (MODE-2)"]
                ],
                "notes": ["help: End live shares before this operation, or use `unique x` to obtain a deep unique copy of counted data. Deep copying may allocate and preserves arena placement; `copy x` shares storage."]
            }),
        );
    });
}

#[test]
fn receive_endpoint_diagnostic_19_suggested_repair_checks_verifies_and_runs() {
    with_compiler_stack(|| {
        let source = r#"
type Status { url: String }
pub fn main(t: Test) -> Result[(), ChannelError] {
  let tasks = test.tasks(t)
  let (tx, rx): (Sender[unique Status], Receiver[unique Status]) = channel.open(tasks)
  let mut outgoing = take tx
  let sent: unique Status = Status(url: "received")
  match channel.send(tasks, mut outgoing, take sent) {
    Ok(()) => {},
    Error((_, e)) => return Error(e),
  }
  let mut incoming = take rx
  let mut rows: List[Status] = []
  list.push(mut rows, channel.receive(tasks, mut incoming)?)
  test.check(t, list.len(rows) == 1)
  test.check(t, rows[0].url == "received")
  Ok(())
}
"#;
        let output = checked(source);
        let [d] = &output.diagnostics[..] else {
            panic!("{:?}", output.diagnostics)
        };
        assert_eq!(d.rule.as_str(), "MODE-2");
        let repair = d
            .notes
            .iter()
            .find(|n| n.starts_with("help: Bind the result first (`"))
            .unwrap();
        let binding = repair.split('`').nth(1).unwrap();
        let taken = repair.split('`').nth(3).unwrap();
        let source = source.replace(
            "list.push(mut rows, channel.receive(tasks, mut incoming)?)",
            &format!("{binding}\n  list.push(mut rows, {taken})"),
        );
        let mut engine = crate::source_std::engine();
        let file = engine.sources.add("repaired.hir", source).unwrap();
        let output = check_progress(&mut engine, file, &default_context()).unwrap();
        assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
        let execution = run_observed_checked(
            &mut engine,
            &RunConfiguration {
                entry: entry_path(&default_context()),
                hash_seed: 0,
                inputs: vec![],
            },
            SimulationConfig::default(),
        )
        .unwrap();
        let observation =
            native::Observation::of_reference(execution.observation.as_ref().unwrap());
        assert_eq!(observation.exit_code, Some(0), "{observation:?}");
        assert!(observation.failures.is_empty(), "{observation:?}");
        let input = engine.export_native_input().unwrap();
        engine.import_native_input(input).unwrap();
    });
}

#[test]
fn receive_endpoint_diagnostic_19_other_invariant_operands_and_controls() {
    with_compiler_stack(|| {
        let declaration = "fn from_list[T](mut xs: List[T]) -> Result[T, ()] { Error(()) }\n";
        let direct = format!(
            "{declaration}fn probe(mut xs: List[unique String], mut rows: List[String]) -> Result[(), ()] {{ list.push(mut rows, from_list(mut xs)?); Ok(()) }}"
        );
        let output = checked(&direct);
        let [d] = &output.diagnostics[..] else {
            panic!("{:?}", output.diagnostics)
        };
        assert_eq!(d.rule.as_str(), "MODE-2");
        assert!(d.message.contains("requires `List[String]`"), "{d:?}");
        assert!(d.message.contains("has `List[unique String]`"), "{d:?}");
        let repaired = direct.replace(
            "list.push(mut rows, from_list(mut xs)?)",
            "let row = from_list(mut xs)?; list.push(mut rows, take row)",
        );
        assert!(checked(&repaired).diagnostics.is_empty());
        // The opposite direction cannot be fixed by a binding: counted data
        // still needs a real unique copy. No bind-then-take advice is offered.
        let reversed = format!(
            "{declaration}fn probe(mut xs: List[String], mut rows: List[unique String]) -> Result[(), ()] {{ list.push(mut rows, from_list(mut xs)?); Ok(()) }}"
        );
        let output = checked(&reversed);
        let [d] = &output.diagnostics[..] else {
            panic!("{:?}", output.diagnostics)
        };
        assert_eq!(d.rule.as_str(), "MODE-2");
        assert!(
            d.to_json(&output.sources)["notes"]
                .to_string()
                .contains("deep unique copy")
        );
        assert!(!d.notes.iter().any(|n| n.contains("Bind the result")));
        // A matching element context retains its original acceptance.
        assert!(
            checked(&DIRECT.replace("mut rows: List[Status]", "mut rows: List[unique Status]"))
                .diagnostics
                .is_empty()
        );
        // The fresh local must avoid an existing binder.
        let collision = DIRECT.replace("  list.push", "  let row = 1u64\n  list.push");
        let output = checked(&collision);
        let [d] = &output.diagnostics[..] else {
            panic!("{:?}", output.diagnostics)
        };
        assert!(d.notes.iter().any(|n| n.contains("let row1 =")), "{d:?}");
    });
}
