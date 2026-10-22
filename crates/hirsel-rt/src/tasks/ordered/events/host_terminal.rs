//! Closed host prefixes reuse the owned request/result phase records. The
//! external observer runs only after every worker left; it never samples a
//! parked TaskCtx or calls an adapter. Task-local stops use the head callback.
use super::*;

pub(super) fn home_results(
    s: &mut super::super::super::Scheduler,
    snapshots: BTreeMap<u32, crate::io_home::Snapshot>,
) -> Result<(), &'static str> {
    for (task, snapshot) in snapshots {
        if let Some(discovery) = snapshot.discovery {
            home_discovery(s, task, &snapshot.result, discovery);
        }
        record_host_initial_for(s, task, &snapshot.result);
        let path = s.task(task).path.clone();
        let Logical::Ordered(driver) = &mut s.logical else {
            return Err("missing home result scheduler");
        };
        let log = driver
            .events
            .as_mut()
            .ok_or("missing home result collector")?;
        let ordinal = log.ordinals[&path]
            .checked_sub(1)
            .ok_or("missing home result ordinal")?;
        let key = (path.clone(), ordinal);
        let done = snapshot.result != Err(crate::io::AdapterError::Blocked);
        if done {
            let initial = log.host_initials[&key].clone();
            assert!(log.completed.insert(key.clone()));
            log.host_completions.push(initial.clone());
            log.return_home(
                &key,
                &initial,
                driver.frontier.as_ref().unwrap().retired(),
                Stretch::new(driver.budgets),
            );
        }
        log.host_outputs.insert(key.clone(), snapshot.output);
        // Non-output attempts already have their normal phase projection.
        // Output uses the existing exact-prefix certificate instead.
        if log.host_entries.contains_key(&key) && done {
            log.certify_host(&path, ordinal)?;
        }
    }
    Ok(())
}

impl Log {
    pub(super) fn terminal_hosts(&mut self) -> Result<Subevents, &'static str> {
        if !self.host_entries.is_empty() || !self.invalid_home.is_empty() {
            return Err("incomplete terminal host entry");
        }
        let mut phases = self.terminal_home_phases()?;
        for key in &self.host_certified {
            if !self.canonical.iter().any(|event| {
                event.task == key.0
                    && event.ordinal == key.1
                    && matches!(event.outcome, CanonicalOutcome::HostResult(_))
            }) {
                return Err("missing certified terminal host result");
            }
            if phases.insert(key.clone(), true).is_some() {
                return Err("conflicting terminal host phases");
            }
        }
        let events = phases
            .into_iter()
            .map(|((creator, ordinal), completed)| {
                (
                    CanonicalItem::HostOperation(ObjectId { creator, ordinal }),
                    CanonicalOutcome::HostAbandoned { completed },
                )
            })
            .collect();
        // All validation precedes mutation. Unsupported observations cannot
        // be made complete by discarding an unproved phase.
        self.home_phases.clear();
        self.host_certified.clear();
        Ok(events)
    }
}

pub(super) fn external(
    s: &mut super::super::super::Scheduler,
    stop: &crate::stop::Stop,
) -> Result<(), &'static str> {
    let Logical::Ordered(driver) = &mut s.logical else {
        return Ok(());
    };
    let Some(log) = driver.events.as_mut() else {
        return Ok(());
    };
    // A local terminal or the source-free Blocked verdict already owns its cut.
    if log
        .canonical
        .last()
        .is_some_and(|event| event.item == CanonicalItem::Terminal)
    {
        return Ok(());
    }
    // These causes require their original failure/wait/charge evidence.
    if !matches!(stop, crate::stop::Stop::Requested) {
        return Err("missing task-local terminal evidence");
    }
    let mut events = log.terminal_hosts()?;
    events.push((
        CanonicalItem::Terminal,
        CanonicalOutcome::Stopped(stop.clone().outcome()),
    ));
    let next = log.ordinals.entry(Vec::new()).or_default();
    let ordinal = *next;
    *next = next.checked_add(1).ok_or("terminal ordinal overflow")?;
    let boundary = Boundary {
        task: Vec::new(),
        ordinal,
        op: Some("stop"),
        transition: Transition::Stop,
        retirement: driver
            .frontier
            .as_ref()
            .ok_or("missing terminal frontier")?
            .retired(),
        before: Stretch::new(driver.budgets),
        credit: 0,
        native_outcome: None,
    };
    log.subevents(&boundary, "stop", events);
    log.boundaries.push(boundary);
    Ok(())
}
