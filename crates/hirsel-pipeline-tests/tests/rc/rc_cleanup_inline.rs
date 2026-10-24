//! Inline cleanup uses the same active-owner and failure suffix planner as variants.
use crate::rc_support as support;
use hirsel_heapcheck::{Config, Event, NoOperations};
use hirsel_lir::stage::Ready;
use hirsel_lir::validate::Inputs;
use hirsel_rc::{cleanup::Finalizers, insert::Proofs, instance::InstanceId, placement::Options};
use support::*;

#[test]
fn untouched_inline_resources_drop_last_first_even_when_the_middle_finalizer_fails() {
    for failing in [false, true] {
        let source = format!(
            r#"resource type Trace {{ zero: u64 }}
drop(x) {{ let _ = 1 / x.zero }}
pub fn main() -> Result[(),u64] {{
  let xs: Inline[Trace,3] = [Trace(zero:1), Trace(zero:{}), Trace(zero:1)]
  Error(0)
}}
"#,
            u64::from(!failing)
        );
        let program = compile(&source);
        let lowered = hirsel_rc::import::lower(&program, &target()).unwrap();
        let operations = lowered.operation_registry(&program);
        let table = lowered.table;
        let rows = manifest(&program);
        let target = target();
        let inputs = Inputs {
            memo: Default::default(),
            discharge: None,
            operations: &operations,
            target: &target,
            layouts: &table,
            manifest: &rows,
        };
        let mut rc = Vec::new();
        for (i, unit) in lowered.units.into_iter().enumerate() {
            assert!(hirsel_lir::validate::check(&unit, &inputs).is_empty());
            let id = InstanceId(i as u32);
            let placed = hirsel_rc::placement::place(
                unit,
                &table,
                &Options::for_instance(true, &lowered.instances, id),
            )
            .unwrap();
            let r = hirsel_rc::insert::insert(
                placed,
                &table,
                &Proofs::for_instance(&lowered.instances, id),
            )
            .unwrap();
            assert!(hirsel_lir::validate::check(&r, &inputs).is_empty());
            rc.push(hirsel_rc::elide::elide(r, &table).unwrap());
        }
        let finalizers = Finalizers::new(&lowered.instances, &rc);
        let units: Vec<_> = rc
            .into_iter()
            .map(|u| hirsel_rc::ready::ready(u, &table, &finalizers).unwrap())
            .collect();
        for unit in &units {
            let violations = hirsel_lir::validate::check(unit, &inputs);
            assert!(violations.is_empty(), "{}\n{violations:?}", dump(unit));
        }
        let program = hirsel_heapcheck::Program::<Ready> {
            invocation_bodies: operations.invocation_bodies.clone(),
            invocations: operations.invocations.clone(),
            pretransfer_adapters: operations.pretransfer_adapters.clone(),
            target,
            layouts: table,
            units,
            statics: vec![],
        };
        let campaign = hirsel_heapcheck::fault_campaign(
            &program,
            &path("main"),
            &[],
            &mut || Box::new(NoOperations),
            &Config::default(),
        );
        assert!(
            campaign.first_violation().is_none(),
            "{:?}",
            campaign.first_violation()
        );
        let run = campaign.clean;
        let finalizers: Vec<_> = run
            .trace
            .iter()
            .filter_map(|e| match e {
                Event::Finalize { resource } => Some(resource.0),
                _ => None,
            })
            .collect();
        assert_eq!(finalizers, [2, 1, 0]);
        assert_eq!(
            !run.result.unwrap().observation.failures.is_empty(),
            failing
        );
    }
}
