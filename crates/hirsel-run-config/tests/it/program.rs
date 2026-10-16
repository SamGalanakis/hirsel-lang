use hirsel_run_config::{
    LaunchKind, ProgramNode, ProgramRevision, RevisionError, RunConfig, RunRefusal,
};

fn node(name: &str, body: &[u8], dependencies: &[&str]) -> ProgramNode {
    ProgramNode {
        identity: name.into(),
        content: vec![body.to_vec()],
        dependencies: dependencies.iter().map(|s| (*s).into()).collect(),
    }
}
fn graph() -> Vec<ProgramNode> {
    vec![
        node("app@1/main", b"helper()", &["app@1/helper"]),
        node(
            "app@1/helper",
            b"Ok(answer)",
            &["app@1/answer", "core@1/Result", "$runtime"],
        ),
        node("app@1/answer", b"42", &[]),
        node("core@1/Result", b"Ok(T)|Error(E)", &[]),
        node("$runtime", b"runtime implementation", &[]),
    ]
}
fn revision(nodes: Vec<ProgramNode>) -> ProgramRevision {
    ProgramRevision::encode("app@1/main", nodes).unwrap()
}

#[test]
fn private_constant_core_and_runtime_edits_change_revision() {
    let base = revision(graph());
    for index in 1..5 {
        let mut changed = graph();
        changed[index].content[0].push(b'!');
        assert_ne!(base, revision(changed), "component {index}");
    }
}

#[test]
fn selection_order_duplicates_and_unreachable_nodes_preserve_revision() {
    let base = revision(graph());
    let mut equivalent = graph();
    equivalent.reverse();
    equivalent[3].dependencies.reverse();
    equivalent[3].dependencies.push("$runtime".into());
    equivalent.push(equivalent[3].clone());
    equivalent.push(node("unused@1/item", b"arbitrary", &["absent"]));
    assert_eq!(base, revision(equivalent));
}

#[test]
fn cycles_are_encoded_as_edges_and_edge_changes_change_revision() {
    let mut recursive = graph();
    recursive[1].dependencies.push("app@1/main".into());
    let base = revision(recursive.clone());
    recursive.reverse();
    assert_eq!(base, revision(recursive));
    assert_ne!(base, revision(graph()));
}

#[test]
fn missing_and_conflicting_components_refuse_with_complete_twins() {
    assert!(ProgramRevision::encode("app@1/main", graph()).is_ok());
    assert_eq!(
        ProgramRevision::encode("absent", graph()),
        Err(RevisionError::Missing {
            identity: "absent".into()
        })
    );
    let mut missing = graph();
    missing.pop();
    assert_eq!(
        ProgramRevision::encode("app@1/main", missing),
        Err(RevisionError::Missing {
            identity: "$runtime".into()
        })
    );
    let mut conflicting = graph();
    conflicting.push(node("app@1/helper", b"Error(())", &[]));
    assert_eq!(
        ProgramRevision::encode("app@1/main", conflicting),
        Err(RevisionError::Conflict {
            identity: "app@1/helper".into()
        })
    );
}

#[test]
fn content_fields_are_length_framed_and_ordered() {
    let mut first = node("app@1/main", b"a", &[]);
    first.content.push(b"bc".to_vec());
    let mut second = first.clone();
    second.content = vec![b"ab".to_vec(), b"c".to_vec()];
    assert_ne!(revision(vec![first.clone()]), revision(vec![second]));
    let base = revision(vec![first.clone()]);
    first.content.reverse();
    assert_ne!(base, revision(vec![first]));
}

#[test]
fn validated_run_binds_semantic_revision_and_entry_before_launch() {
    let base = revision(graph());
    let run = RunConfig::for_program(&base, LaunchKind::Main)
        .validated()
        .unwrap();
    assert!(run.check_revision(&base, LaunchKind::Main).is_ok());
    let mut changed = graph();
    changed[1].content = vec![b"Error(())".to_vec()];
    assert_eq!(
        run.check_revision(&revision(changed), LaunchKind::Main),
        Err(RunRefusal::Binding {
            field: "program".into()
        })
    );
    assert_eq!(
        run.check_revision(&base, LaunchKind::Test),
        Err(RunRefusal::Binding {
            field: "launch".into()
        })
    );
    let mut other = RunConfig::for_program(&base, LaunchKind::Main);
    other.entry = "app@1/other".into();
    assert_eq!(
        other
            .validated()
            .unwrap()
            .check_revision(&base, LaunchKind::Main),
        Err(RunRefusal::Binding {
            field: "entry".into()
        })
    );
}

#[test]
fn revision_matches_an_independent_sha256_golden_vector() {
    let encoded = revision(vec![node("app@1/main", b"Ok(())", &[])]);
    assert_eq!(encoded.digest(), GOLDEN);
}

// Independently framed with Python struct.pack("<Q") and hashlib.sha256.
const GOLDEN: [u8; 32] = [
    11, 255, 19, 28, 225, 86, 141, 119, 22, 182, 91, 168, 216, 220, 120, 48, 174, 165, 244, 197,
    57, 55, 38, 176, 65, 42, 234, 68, 6, 110, 127, 103,
];
