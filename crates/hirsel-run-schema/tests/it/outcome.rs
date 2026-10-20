use hirsel_run_schema::{
    BudgetKind, Environmental, ExecutionDiagnostic, Failure, Returned, RunOutcome, Site,
    ToolingLimit, Wait,
};

#[test]
fn every_run_ending_has_one_exit_policy() {
    let diagnostic = ExecutionDiagnostic {
        rule: "unchecked precondition".into(),
        site: Site::default(),
        message: "stopped".into(),
    };
    let cases = [
        (RunOutcome::Returned(Returned::Ok), 0),
        (RunOutcome::Returned(Returned::Error), 1),
        (
            RunOutcome::Failed {
                primary: Failure {
                    rule: "LANG-9".into(),
                    site: Site::default(),
                    operation: None,
                },
                causes: vec![],
            },
            2,
        ),
        (RunOutcome::Refused("grant".into()), 3),
        (RunOutcome::Diverged("request".into()), 3),
        (RunOutcome::Tooling("output".into()), 3),
        (
            RunOutcome::ToolingLimit(ToolingLimit {
                kind: "Steps".into(),
                limit: 0,
                site: None,
            }),
            3,
        ),
        (RunOutcome::UndefinedBehavior(diagnostic.clone()), 3),
        (RunOutcome::Trapped(diagnostic), 3),
        (
            RunOutcome::Blocked(vec![Wait {
                spawn_path: vec![0],
                site: Site::default(),
                kind: "channel.receive".into(),
                targets: vec![0],
            }]),
            4,
        ),
        (RunOutcome::BudgetExceeded(BudgetKind::Fuel), 5),
        (RunOutcome::BudgetExceeded(BudgetKind::Memory), 5),
        (RunOutcome::Environmental(Environmental::CodecLimit), 6),
        (
            RunOutcome::Environmental(Environmental::Recording("tape".into())),
            6,
        ),
        (
            RunOutcome::Environmental(Environmental::Output("stdout".into())),
            6,
        ),
        (RunOutcome::Environmental(Environmental::Stopped), 6),
    ];
    for (outcome, code) in cases {
        assert_eq!(outcome.exit_code(), code, "{outcome:?}");
        if let RunOutcome::Blocked(waits) = outcome {
            assert_eq!(Wait::decode(&Wait::encode(&waits)).unwrap(), waits);
        }
    }
}
