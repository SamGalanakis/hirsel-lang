use hirsel_run_config::*;
use hirsel_run_schema::*;
use serde_json::{Value, json};

fn record(signals: Option<Value>) -> Value {
    let mut record = serde_json::to_value(RunConfig::empty(
        [0; 32],
        "probe@1/main".into(),
        LaunchKind::Main,
    ))
    .unwrap();
    record["io"] = json!({"dirs": []});
    if let Some(signals) = signals {
        record["io"]["signals"] = signals;
    }
    record
}

fn validate(signals: Value) -> Result<ValidatedRun, RunRefusal> {
    RunConfig::validate(&serde_json::to_vec(&record(Some(signals))).unwrap())
}

#[test]
fn every_signal_kind_permutation_has_the_same_canonical_bytes_and_identity() {
    let permutations = [
        ["hangup", "interrupt", "terminate"],
        ["hangup", "terminate", "interrupt"],
        ["interrupt", "hangup", "terminate"],
        ["interrupt", "terminate", "hangup"],
        ["terminate", "hangup", "interrupt"],
        ["terminate", "interrupt", "hangup"],
    ];
    let canonical =
        validate(json!({"provider": "signals.host@1", "kinds": permutations[0]})).unwrap();
    for kinds in permutations {
        let signals = json!({"provider": "signals.host@1", "kinds": kinds});
        let run = validate(signals.clone()).unwrap();
        assert_eq!(
            run.signals_grant().unwrap().kinds,
            [
                SignalKind::Hangup,
                SignalKind::Interrupt,
                SignalKind::Terminate
            ]
        );
        assert_eq!(run.canonical(), canonical.canonical(), "{kinds:?}");
        assert_eq!(run.identity(), canonical.identity(), "{kinds:?}");
        let standalone = validate_signals_grant(&serde_json::to_vec(&signals).unwrap()).unwrap();
        assert_eq!(
            standalone.kinds,
            [
                SignalKind::Hangup,
                SignalKind::Interrupt,
                SignalKind::Terminate
            ]
        );
    }
}

fn invalid(signals: Value, expected: &str) {
    assert!(
        matches!(validate(signals), Err(RunRefusal::Invalid { field, .. }) if field == expected),
        "expected invalid {expected}"
    );
}

#[test]
fn kinds_sort_by_name_and_project_signals() {
    let run = validate(
        json!({"provider": "signals.host@1", "kinds": ["terminate", "interrupt", "hangup"]}),
    )
    .unwrap();
    assert!(run.io_projections().contains("signals"));
    let grant = run.signals_grant().unwrap();
    assert_eq!(
        grant.kinds,
        [
            SignalKind::Hangup,
            SignalKind::Interrupt,
            SignalKind::Terminate
        ]
    );
    assert_eq!(grant.bits(), 0b111);
    assert!(std::str::from_utf8(run.canonical()).unwrap().contains(
        r#""signals":{"provider":"signals.host@1","kinds":["hangup","interrupt","terminate"]}"#
    ));
}

#[test]
fn an_absent_or_null_grant_keeps_the_record_bytes() {
    let without = RunConfig::validate(&serde_json::to_vec(&record(None)).unwrap()).unwrap();
    assert!(!without.io_projections().contains("signals"));
    assert!(
        !std::str::from_utf8(without.canonical())
            .unwrap()
            .contains("signals")
    );
    assert_eq!(
        validate(Value::Null).unwrap().identity(),
        without.identity()
    );
}

#[test]
fn kinds_are_nonempty_unique_and_known() {
    invalid(
        json!({"provider": "signals.host@1", "kinds": []}),
        "io.signals.kinds",
    );
    invalid(
        json!({"provider": "signals.host@1", "kinds": ["interrupt", "interrupt"]}),
        "io.signals.kinds",
    );
    assert!(matches!(
        validate(json!({"provider": "signals.host@1", "kinds": ["kill"]})),
        Err(RunRefusal::Malformed { .. })
    ));
}

#[test]
fn a_script_belongs_to_signals_script_only() {
    invalid(
        json!({"provider": "signals.host@1", "kinds": ["interrupt"], "script": []}),
        "io.signals.script",
    );
    invalid(
        json!({"provider": "signals.script@1", "kinds": ["interrupt"]}),
        "io.signals.script",
    );
    // A script may deliver a kind the grant does not observe: its default action ends the run.
    let run = validate(json!({"provider": "signals.script@1", "kinds": ["interrupt"], "script": ["hangup", "interrupt"]}))
        .unwrap();
    assert_eq!(
        run.signals_grant().unwrap().script,
        Some(vec![SignalKind::Hangup, SignalKind::Interrupt])
    );
    assert!(matches!(
        validate(json!({"provider": "signals.other@1", "kinds": ["interrupt"]})),
        Err(RunRefusal::Unsupported { field }) if field == "io.signals.provider"
    ));
}

#[test]
fn a_signals_grant_is_within_one_of_the_same_provider_observing_more() {
    let host =
        |kinds: Value| validate(json!({"provider": "signals.host@1", "kinds": kinds})).unwrap();
    let both = host(json!(["interrupt", "terminate"]));
    let one = host(json!(["interrupt"]));
    assert!(grants_within(&one, &both));
    assert!(!grants_within(&both, &one));
    let none = RunConfig::validate(&serde_json::to_vec(&record(None)).unwrap()).unwrap();
    assert!(grants_within(&none, &one));
    assert!(!grants_within(&one, &none));
    let scripted = |script: Value| {
        validate(json!({"provider": "signals.script@1", "kinds": ["interrupt"], "script": script}))
            .unwrap()
    };
    // Script data grants nothing; another provider is not within.
    assert!(grants_within(
        &scripted(json!(["interrupt"])),
        &scripted(json!([]))
    ));
    assert!(!grants_within(&scripted(json!([])), &one));
}

#[test]
fn the_standalone_grant_validates_like_the_record_member() {
    let grant = validate_signals_grant(br#"{"provider":"signals.script@1","kinds":["terminate","interrupt"],"script":["terminate"]}"#)
        .unwrap();
    assert_eq!(grant.kinds, [SignalKind::Interrupt, SignalKind::Terminate]);
    assert!(
        validate_signals_grant(br#"{"provider":"net.stream@1","kinds":["interrupt"]}"#).is_err()
    );
}

#[test]
fn a_signaled_run_exits_as_a_shell_reports_a_signal() {
    assert_eq!(RunOutcome::Signaled(SignalKind::Interrupt).exit_code(), 130);
    assert_eq!(RunOutcome::Signaled(SignalKind::Terminate).exit_code(), 143);
    assert_eq!(RunOutcome::Signaled(SignalKind::Hangup).exit_code(), 129);
}
