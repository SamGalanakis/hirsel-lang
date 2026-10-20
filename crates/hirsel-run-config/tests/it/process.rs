use hirsel_run_config::*;
use hirsel_run_schema::*;
use hirsel_run_schema::*;
use serde_json::{Value, json};

fn validate(process: Value) -> Result<ValidatedRun, RunRefusal> {
    let mut record = serde_json::to_value(RunConfig::empty(
        [0; 32],
        "probe@1/main".into(),
        LaunchKind::Main,
    ))
    .unwrap();
    record["io"] = json!({"dirs": [], "process": process});
    RunConfig::validate(&serde_json::to_vec(&record).unwrap())
}

fn invalid(process: Value, expected: &str) {
    assert!(
        matches!(validate(process), Err(RunRefusal::Invalid { field, .. }) if field == expected),
        "expected invalid {expected}"
    );
}

fn spawn(program: &str) -> Value {
    json!({"provider": "process.spawn@1", "rules": [{"program": program}]})
}

#[test]
fn programs_are_exact_canonical_absolute_paths() {
    for program in ["/bin/echo", "/usr/bin/git", "/opt/tool-1.2/bin/x"] {
        let run = validate(spawn(program)).unwrap();
        assert!(run.io_projections().contains("process"));
        assert_eq!(run.process_grant().unwrap().rules[0].program, program);
    }
    for program in [
        "",
        "/",
        "git",
        "bin/echo",
        "//bin/echo",
        "/bin//echo",
        "/bin/echo/",
        "/bin/./echo",
        "/bin/../bin/echo",
        "/bin\\echo",
        "/bin/e\0cho",
    ] {
        invalid(spawn(program), "io.process.rules.program");
    }
    invalid(
        json!({"provider": "process.spawn@1", "rules": [{"program": "/bin/a"}, {"program": "/bin/a", "env": ["HOME"]}]}),
        "io.process.rules",
    );
}

#[test]
fn rules_and_environment_names_are_sorted_and_unique() {
    let run = validate(json!({"provider": "process.spawn@1", "rules": [
        {"program": "/usr/bin/git", "env": ["PATH", "HOME"]},
        {"program": "/bin/echo"}
    ]}))
    .unwrap();
    let canonical = String::from_utf8(run.canonical().to_vec()).unwrap();
    assert!(canonical.contains(
        r#""process":{"provider":"process.spawn@1","rules":[{"program":"/bin/echo","env":[]},{"program":"/usr/bin/git","env":["HOME","PATH"]}]}"#
    ));
    for env in [
        json!([""]),
        json!(["A=B"]),
        json!(["A\u{0}"]),
        json!(["HOME", "HOME"]),
    ] {
        invalid(
            json!({"provider": "process.spawn@1", "rules": [{"program": "/bin/echo", "env": env}]}),
            "io.process.rules.env",
        );
    }
}

#[test]
fn scripted_runs_belong_only_to_the_script_provider() {
    let runs = json!([
        {"program": "/usr/bin/git", "args": ["status"], "outcome": {"output": {"status": {"exited": 0}, "stdout": [104, 105], "stderr": []}}},
        {"program": "/usr/bin/git", "args": [], "outcome": {"error": {"fault": 7}}},
        {"program": "/bin/false", "args": [], "outcome": {"output": {"status": {"signaled": 9}, "stdout": [], "stderr": []}}},
        {"program": "/bin/true", "args": [], "outcome": {"error": "output_limit"}}
    ]);
    let run = validate(json!({"provider": "process.script@1", "rules": [], "runs": runs})).unwrap();
    let grant = run.process_grant().unwrap();
    assert!(grant.rules.is_empty());
    assert_eq!(
        grant.runs.as_ref().unwrap()[1].outcome,
        ProcessOutcome::Error(ProcessFailure::Fault(7))
    );
    // Order is preserved: the first matching entry serves first.
    assert_eq!(grant.runs.as_ref().unwrap()[2].program, "/bin/false");
    invalid(
        json!({"provider": "process.spawn@1", "rules": [], "runs": []}),
        "io.process.runs",
    );
    invalid(
        json!({"provider": "process.script@1", "rules": []}),
        "io.process.runs",
    );
    assert!(matches!(
        validate(json!({"provider": "process.script@1", "rules": [], "runs": null})),
        Err(RunRefusal::Malformed { .. })
    ));
    invalid(
        json!({"provider": "process.script@1", "rules": [], "runs": [
            {"program": "git", "args": [], "outcome": {"error": "not_found"}}
        ]}),
        "io.process.runs.program",
    );
    assert!(matches!(
        validate(
            json!({"provider": "process.script@1", "rules": [], "runs": [
                {"program": "/bin/x", "args": [], "outcome": {"error": "exploded"}}
            ]})
        ),
        Err(RunRefusal::Malformed { .. })
    ));
}

#[test]
fn unknown_process_providers_refuse_instead_of_widening() {
    for provider in ["process.spawn@2", "process.script@2", "net.stream@1"] {
        assert_eq!(
            validate(json!({"provider": provider, "rules": []})),
            Err(RunRefusal::Unsupported {
                field: "io.process.provider".into()
            })
        );
    }
}

#[test]
fn scripted_children_belong_only_to_the_script_provider() {
    let children = json!([
        {"program": "/bin/cat", "args": [], "steps": [
            {"expect": [104, 105]}, {"print": [104, 105]}, {"print_err": []}, {"exit": {"exited": 0}}
        ]}
    ]);
    let run = validate(
        json!({"provider": "process.script@1", "rules": [], "runs": [], "children": children}),
    )
    .unwrap();
    let grant = run.process_grant().unwrap();
    let child = &grant.children.as_ref().unwrap()[0];
    assert_eq!(child.steps[0], ChildStep::Expect(b"hi".to_vec()));
    assert_eq!(child.steps[3], ChildStep::Exit(ExitStatus::Exited(0)));
    // Absent children are none; they are no substitute for runs.
    assert!(
        validate(json!({"provider": "process.script@1", "rules": [], "runs": []}))
            .unwrap()
            .process_grant()
            .unwrap()
            .children
            .is_none()
    );
    invalid(
        json!({"provider": "process.spawn@1", "rules": [], "children": []}),
        "io.process.children",
    );
    invalid(
        json!({"provider": "process.script@1", "rules": [], "runs": [], "children": [
            {"program": "cat", "args": [], "steps": []}
        ]}),
        "io.process.children.program",
    );
    assert!(matches!(
        validate(
            json!({"provider": "process.script@1", "rules": [], "runs": [], "children": [
                {"program": "/bin/cat", "args": [], "steps": ["stall"]}
            ]})
        ),
        Err(RunRefusal::Malformed { .. })
    ));
}

#[test]
fn scripts_time_out_but_never_cancel() {
    let run = |outcome: Value| {
        validate(
            json!({"provider": "process.script@1", "rules": [], "runs": [
                {"program": "/bin/sleep", "args": [], "outcome": outcome}
            ]}),
        )
    };
    assert_eq!(
        run(json!({"error": "timed_out"}))
            .unwrap()
            .process_grant()
            .unwrap()
            .runs
            .as_ref()
            .unwrap()[0]
            .outcome,
        ProcessOutcome::Error(ProcessFailure::TimedOut)
    );
    assert!(run(json!({"error": "input_closed"})).is_ok());
    invalid(
        json!({"provider": "process.script@1", "rules": [], "runs": [
            {"program": "/bin/sleep", "args": [], "outcome": {"error": {"canceled": 0}}}
        ]}),
        "io.process.runs.outcome",
    );
}
