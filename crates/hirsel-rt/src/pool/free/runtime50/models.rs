//! Negative controls for the SC-granularity models of the save, claim and
//! section-handshake protocols. Every interleaving of the actors' steps is
//! explored by re-executing each schedule prefix (stateless model checking).
//! The save and claim models call the production `TaskState` and
//! `claim::Request` transitions, each one atomic CAS. The handshake model
//! steps the gate's SC accesses (section.rs `enter`/`close`) over per-thread
//! store buffers (TSO), the GenMC gate.c shape. Weak memory beyond that is
//! proofs/free's Loom and GenMC (`check.sh`); this does not replace them.
use hirsel_free::claim::{Arbiter, Claim, Request, Requests};
use hirsel_free::schedule::TaskState;
use std::sync::{Arc, Mutex};

struct Spec<'a, S> {
    init: &'a dyn Fn() -> S,
    actors: usize,
    enabled: &'a dyn Fn(&S, usize) -> bool,
    step: &'a dyn Fn(&mut S, usize),
    invariant: &'a dyn Fn(&S) -> Result<(), String>,
    terminal: &'a dyn Fn(S) -> Result<(), String>,
}

/// Every execution, or the first violating schedule.
fn explore<S>(spec: &Spec<'_, S>) -> Result<u64, (Vec<usize>, String)> {
    fn visit<S>(
        spec: &Spec<'_, S>,
        prefix: &mut Vec<usize>,
        executions: &mut u64,
    ) -> Result<(), (Vec<usize>, String)> {
        let mut state = (spec.init)();
        for &actor in prefix.iter() {
            (spec.step)(&mut state, actor);
        }
        (spec.invariant)(&state).map_err(|e| (prefix.clone(), e))?;
        let enabled: Vec<usize> = (0..spec.actors)
            .filter(|&a| (spec.enabled)(&state, a))
            .collect();
        if enabled.is_empty() {
            *executions += 1;
            return (spec.terminal)(state).map_err(|e| (prefix.clone(), e));
        }
        drop(state);
        for actor in enabled {
            prefix.push(actor);
            visit(spec, prefix, executions)?;
            prefix.pop();
        }
        Ok(())
    }
    let mut executions = 0;
    visit(spec, &mut Vec::new(), &mut executions)?;
    Ok(executions)
}

// Save: the owner parks its running frame while two notifiers wake it and a
// worker starts whatever queue token exists. One token or running owner at a
// time, and no wake is lost (tokio runtime/task/state.rs shape).
struct Save {
    state: TaskState,
    duplicate: bool,
    tokens: u32,
    running: u32,
    owner: u8,
    notifiers: [bool; 2],
    worker: u8,
    refused: bool,
}

fn save(duplicate: bool) -> Result<u64, (Vec<usize>, String)> {
    let init = move || {
        let state = TaskState::default();
        assert!(state.notify() && state.start());
        Save {
            state,
            duplicate,
            tokens: 0,
            running: 1,
            owner: 0,
            notifiers: [false; 2],
            worker: 0,
            refused: false,
        }
    };
    let enabled = |s: &Save, actor: usize| match actor {
        0 => s.owner == 0,
        1 | 2 => !s.notifiers[actor - 1],
        _ => match s.worker {
            0 => s.tokens > 0,
            1 => true,
            _ => false,
        },
    };
    let step = |s: &mut Save, actor: usize| match actor {
        0 => {
            if s.state.suspend(false) {
                s.tokens += 1;
            }
            s.running -= 1;
            s.owner = 1;
        }
        1 | 2 => {
            let granted = s.state.notify();
            // Mutation: a wake that publishes without winning the state CAS.
            if granted || (s.duplicate && s.owner == 1 && s.worker == 0) {
                s.tokens += 1;
            }
            s.notifiers[actor - 1] = true;
        }
        _ => {
            if s.worker == 0 {
                s.tokens -= 1;
                if s.state.start() {
                    s.running += 1;
                } else {
                    s.refused = true;
                }
                s.worker = 1;
            } else {
                s.state.complete();
                s.running -= 1;
                s.worker = 2;
            }
        }
    };
    let invariant = |s: &Save| {
        if s.refused {
            return Err("a queue token's start found another owner".into());
        }
        if s.tokens + s.running > 1 {
            return Err(format!(
                "{} tokens and {} running owners",
                s.tokens, s.running
            ));
        }
        Ok(())
    };
    let terminal = |s: Save| {
        if s.worker != 2 {
            return Err("a wake was lost at park".into());
        }
        if s.state.notify() {
            return Err("a completed frame accepted a wake".into());
        }
        Ok(())
    };
    explore(&Spec {
        init: &init,
        actors: 4,
        enabled: &enabled,
        step: &step,
        invariant: &invariant,
        terminal: &terminal,
    })
}

// Claim: one request; the home claims and finishes it, a cancellation
// withdraws it, a stop sweeps the table. Exactly one of claim and withdraw
// wins, a claimed request at the stop is in doubt, and the record frees its
// buffer once (claim.rs; GenMC claim.c).
struct Owned(Arc<Mutex<u32>>);
impl Drop for Owned {
    fn drop(&mut self) {
        *self.0.lock().unwrap() += 1;
    }
}

struct ClaimModel {
    arbiter: Arbiter,
    requests: Requests<Owned>,
    request: Option<Arc<Request<Owned>>>,
    freed: Arc<Mutex<u32>>,
    blind: bool,
    issuer: u8,
    canceller: u8,
    stopper: bool,
    claimed: bool,
    withdrew: u32,
    in_doubt: bool,
    seen_prepared: bool,
}

fn claim(blind: bool) -> Result<u64, (Vec<usize>, String)> {
    let init = move || ClaimModel {
        arbiter: Arbiter::new(false, 1),
        requests: Requests::default(),
        request: None,
        freed: Arc::new(Mutex::new(0)),
        blind,
        issuer: 0,
        canceller: 0,
        stopper: false,
        claimed: false,
        withdrew: 0,
        in_doubt: false,
        seen_prepared: false,
    };
    let enabled = |s: &ClaimModel, actor: usize| match actor {
        0 => s.issuer < 3,
        1 => s.request.is_some() && s.canceller < if s.blind { 2 } else { 1 },
        _ => !s.stopper,
    };
    let step = |s: &mut ClaimModel, actor: usize| match actor {
        0 => match s.issuer {
            0 => {
                let request = s.requests.prepare(1, Owned(s.freed.clone()));
                if request.state() == Claim::Withdrawn {
                    // Prepared after the sweep closed the table.
                    s.withdrew += 1;
                }
                s.request = Some(request);
                s.issuer = 1;
            }
            1 => {
                s.claimed = s.request.as_ref().unwrap().claim();
                s.issuer = if s.claimed { 2 } else { 3 };
            }
            _ => {
                // The call returned: its done phase, or quarantine.
                if s.arbiter.section(0).is_ok() {
                    s.request.as_ref().unwrap().finish();
                    s.requests.retire(1);
                }
                s.issuer = 3;
            }
        },
        1 => {
            let request = s.request.as_ref().unwrap();
            if !s.blind {
                if request.withdraw() {
                    s.withdrew += 1;
                    s.requests.retire(1);
                }
                s.canceller = 1;
            } else if s.canceller == 0 {
                // Mutation: read Prepared, then withdraw without arbitration.
                s.seen_prepared = request.state() == Claim::Prepared;
                s.canceller = 1;
            } else {
                if s.seen_prepared {
                    s.withdrew += 1;
                }
                s.canceller = 2;
            }
        }
        _ => {
            let terminal = s.arbiter.stop(0, &[&s.requests]).expect("one stop");
            s.withdrew += terminal.sweep.withdrawn.len() as u32;
            s.in_doubt = !terminal.sweep.in_doubt.is_empty();
            s.stopper = true;
        }
    };
    let invariant = |s: &ClaimModel| {
        if u32::from(s.claimed) + s.withdrew > 1 {
            return Err("claim and withdraw both won".into());
        }
        Ok(())
    };
    let terminal = |s: ClaimModel| {
        if u32::from(s.claimed) + s.withdrew != 1 {
            return Err("the request was neither claimed nor withdrawn".into());
        }
        let state = s.request.as_ref().unwrap().state();
        if s.claimed && state == Claim::ClaimedForIssue && !s.in_doubt {
            return Err("a claimed request at the stop was not in doubt".into());
        }
        let freed = s.freed.clone();
        drop(s);
        if *freed.lock().unwrap() != 1 {
            return Err("the record did not free its buffer exactly once".into());
        }
        Ok(())
    };
    explore(&Spec {
        init: &init,
        actors: 3,
        enabled: &enabled,
        step: &step,
        invariant: &invariant,
        terminal: &terminal,
    })
}

// Section handshake: the worker's SC `active := 1; read cut` against the
// cutter's SC `cut := 1; read active`, over TSO store buffers. Actors 2 and 3
// drain each thread's oldest buffered store. The cutter must never scan while
// the worker's section is open; WEAK_ENTER makes `active := 1` a plain store.
#[derive(Default)]
struct Gate {
    weak: bool,
    memory: [u32; 2],
    buffers: [Vec<(usize, u32)>; 2],
    worker: u8,
    cutter: u8,
    sections: u8,
    overlap: bool,
}
const ACTIVE: usize = 0;
const CUT: usize = 1;

impl Gate {
    fn load(&self, thread: usize, var: usize) -> u32 {
        self.buffers[thread]
            .iter()
            .rev()
            .find(|(v, _)| *v == var)
            .map_or(self.memory[var], |&(_, value)| value)
    }
    fn store(&mut self, thread: usize, var: usize, value: u32, sc: bool) {
        self.buffers[thread].push((var, value));
        if sc {
            // The SC store is followed by a full fence (sync.rs sc_store).
            for (var, value) in std::mem::take(&mut self.buffers[thread]) {
                self.memory[var] = value;
            }
        }
    }
}

fn gate(weak: bool) -> Result<u64, (Vec<usize>, String)> {
    let init = move || Gate {
        weak,
        ..Gate::default()
    };
    let enabled = |s: &Gate, actor: usize| match actor {
        // Two sections, with the backoff wait enabled only once the cut reopens.
        0 => s.worker < 6 && (s.worker != 5 || s.load(0, CUT) == 0),
        1 => s.cutter < 4 && (s.cutter != 1 || s.load(1, ACTIVE) == 0),
        thread => !s.buffers[thread - 2].is_empty(),
    };
    let step = |s: &mut Gate, actor: usize| match actor {
        0 => {
            s.worker = match s.worker {
                0 => {
                    let sc = !s.weak;
                    s.store(0, ACTIVE, 1, sc);
                    1
                }
                1 if s.load(0, CUT) == 0 => 2,
                1 => 4,
                2 => {
                    // Append and publish inside the section.
                    if s.cutter == 2 {
                        s.overlap = true;
                    }
                    3
                }
                3 => {
                    s.store(0, ACTIVE, 0, false);
                    s.sections += 1;
                    if s.sections == 2 { 6 } else { 0 }
                }
                4 => {
                    s.store(0, ACTIVE, 0, false);
                    5
                }
                _ => 0,
            }
        }
        1 => {
            s.cutter = match s.cutter {
                0 => {
                    s.store(1, CUT, 1, true);
                    1
                }
                1 => {
                    // Scan: every slot was seen inactive after the close.
                    if s.worker == 2 || s.worker == 3 {
                        s.overlap = true;
                    }
                    2
                }
                2 => {
                    s.store(1, CUT, 0, true);
                    3
                }
                _ => 4,
            }
        }
        thread => {
            let (var, value) = s.buffers[thread - 2].remove(0);
            s.memory[var] = value;
        }
    };
    let invariant = |s: &Gate| {
        if s.overlap {
            Err("the cutter scanned an open section".into())
        } else {
            Ok(())
        }
    };
    let terminal = |s: Gate| {
        if s.worker != 6 || s.cutter != 4 {
            return Err("the handshake stalled".into());
        }
        Ok(())
    };
    explore(&Spec {
        init: &init,
        actors: 4,
        enabled: &enabled,
        step: &step,
        invariant: &invariant,
        terminal: &terminal,
    })
}

#[test]
fn runtime50_model_negative_controls() {
    let sc = gate(false).expect("the SC section handshake");
    let saves = save(false).expect("the production save/notify protocol");
    let claims = claim(false).expect("the production claim/withdraw protocol");
    let (weak, weak_reason) = gate(true).expect_err("a weakened gate must fail");
    let (duplicate, duplicate_reason) = save(true).expect_err("a duplicate owner must fail");
    let (blind, blind_reason) = claim(true).expect_err("an unarbitrated withdrawal must fail");
    assert!(sc > 1 && saves > 1 && claims > 1);
    eprintln!(
        "RUNTIME50 models: SC gate {sc}, save {saves}, claim {claims} executions pass; \
         weakened gate {weak_reason} at {weak:?}; duplicate owner {duplicate_reason} at \
         {duplicate:?}; unarbitrated withdrawal {blind_reason} at {blind:?}"
    );
}
