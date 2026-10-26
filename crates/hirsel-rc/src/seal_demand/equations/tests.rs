use super::*;

fn local(name: &str) -> LocalDemandEquations {
    LocalDemandEquations {
        schema: 1,
        owner: (DefPath::core(name), ContentHash([0; 32])),
        values: 6,
        parameters: vec![0],
        captures: vec![],
        flow: vec![],
        closures: vec![],
        calls: vec![],
        returns: vec![],
        seals: vec![],
    }
}

fn direct(to: &LocalDemandEquations) -> Call {
    Call {
        target: Some(Target {
            definition: to.owner.0.clone(),
            instance: Some(to.owner.1),
        }),
        environment: None,
        arguments: vec![(0, 0, true)],
        result: Some(1),
    }
}

#[test]
fn compact_demand_follows_recursive_return_and_cfg_aliases() {
    let mut entry = local("entry");
    let mut child = local("child");
    entry.calls.push(direct(&child));
    entry.flow = vec![(2, 1), (3, 2), (3, 4)];
    entry.seals = vec![3];
    child.calls.push(direct(&entry));
    child.returns = vec![0];
    let d = solve(&[entry, child], &[0]).unwrap();
    assert_eq!(
        d.values[0],
        [0, 1, 2, 3, 4].into_iter().map(ValueId).collect()
    );
    assert_eq!(d.values[1], [ValueId(0)].into_iter().collect());
}

#[test]
fn compact_demand_follows_returned_closure_and_captured_arena() {
    let mut entry = local("entry");
    let mut maker = local("maker");
    let mut callback = local("callback");
    callback.parameters.clear();
    callback.captures = vec![0];
    callback.seals = vec![0];
    maker.closures.push(Closure {
        value: 1,
        target: direct(&callback).target,
        captures: vec![0],
    });
    maker.returns = vec![1];
    entry.calls.push(direct(&maker));
    entry.calls.push(Call {
        target: None,
        environment: Some(1),
        arguments: vec![],
        result: None,
    });
    let d = solve(&[entry, maker, callback], &[0]).unwrap();
    for values in d.values {
        assert_eq!(values, [ValueId(0)].into_iter().collect());
    }
}

#[test]
fn compact_demand_no_seal_and_unreachable_template_stay_ordinary() {
    let mut entry = local("entry");
    let mut template = local("template");
    entry.calls.push(direct(&template));
    // This target is unavailable, but no reachable seal exists.
    assert!(solve(&[entry.clone()], &[0]).unwrap().values[0].is_empty());
    entry.calls.clear();
    template.seals = vec![0];
    assert!(
        solve(&[entry, template], &[0])
            .unwrap()
            .values
            .iter()
            .all(BTreeSet::is_empty)
    );
}

#[test]
fn compact_demand_refuses_missing_ambiguous_and_malformed_inputs() {
    let mut entry = local("entry");
    let child = local("child");
    entry.seals = vec![0];
    entry.calls.push(direct(&child));
    assert!(
        solve(&[entry.clone()], &[0])
            .unwrap_err()
            .contains("complete arena call target")
    );
    assert!(solve(&[entry.clone(), child.clone()], &[0]).is_ok());
    let mut other = child.clone();
    other.owner.1 = ContentHash([1; 32]);
    entry.calls[0].target.as_mut().unwrap().instance = None;
    assert!(solve(&[entry.clone(), child.clone(), other], &[0]).is_err());
    assert!(solve(&[child.clone(), child.clone()], &[0]).is_err());
    assert!(solve(&[child.clone()], &[1]).is_err());
    for mutant in 0..7 {
        let mut bad = child.clone();
        match mutant {
            0 => bad.schema = 2,
            1 => bad.flow = vec![(1, 0), (1, 0)],
            2 => bad.flow = vec![(6, 0)],
            3 => bad.seals = vec![0, 0],
            4 => bad.returns = vec![6],
            5 => bad.closures.push(Closure {
                value: 6,
                target: None,
                captures: vec![],
            }),
            _ => {
                let mut call = direct(&child);
                call.result = Some(6);
                bad.calls.push(call);
            }
        }
        assert!(solve(&[bad], &[0]).is_err(), "mutant {mutant}");
    }
}
