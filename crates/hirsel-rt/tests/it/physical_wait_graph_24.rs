//! Finite physical abstraction of pool::call_home_in and role::acquire_home.
//! C is retained during a synchronous loan; home serves Call while waiting
//! for R, for both context entry and saved-frame claim. Actual progress tests
//! remain in pool::progress_tests and pool::progress_audit_45.
use std::collections::{BTreeMap, BTreeSet, VecDeque};

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct State {
    // 0 wants C; 1 wants R; 2 holds R; 3 queued Call; 4 Call ended; 5 done.
    worker: [u8; 3],
    // 0 waits for R; 1 merges under R; 2 finished.
    home: u8,
    permits: u8,
    role: Option<usize>,
    enqueued: u8,
    completed: u8,
    stopped: bool,
}

#[derive(Clone, Copy)]
enum Mutation {
    None,
    ReturnC,
    StopServingCalls,
}

fn successors(s: State, n: usize, capacity: u32, mutation: Mutation) -> Vec<State> {
    let mut out = Vec::new();
    if !s.stopped {
        out.push(State { stopped: true, ..s });
    }
    if s.home == 0 && s.role.is_none() {
        out.push(State {
            home: 1,
            role: Some(n),
            ..s
        });
    }
    if s.home == 1 {
        out.push(State {
            home: 2,
            role: None,
            ..s
        });
    }
    for i in 0..n {
        let bit = 1 << i;
        let mut t = s;
        match s.worker[i] {
            0 if s.stopped => t.worker[i] = 5,
            0 if s.permits.count_ones() < capacity => {
                t.permits |= bit;
                t.worker[i] = 1;
            }
            1 if s.stopped => {
                t.worker[i] = 5;
                t.permits &= !bit;
            }
            1 if s.role.is_none() => {
                t.role = Some(i);
                t.worker[i] = 2;
            }
            2 => {
                t.worker[i] = 3;
                t.enqueued |= bit;
                if matches!(mutation, Mutation::ReturnC) {
                    t.permits &= !bit;
                }
            }
            3 if s.home != 1
                && !(s.home == 0 && matches!(mutation, Mutation::StopServingCalls)) =>
            {
                // Drain even a canceled queued call before retiring its stack.
                // Home borrows the caller's C/R; it acquires neither resource.
                t.completed |= bit;
                t.worker[i] = 4;
            }
            4 if s.permits & bit != 0 || s.permits.count_ones() < capacity => {
                t.worker[i] = 5;
                t.role = None;
                t.permits &= !bit;
            }
            _ => continue,
        }
        out.push(t);
    }
    out
}

fn terminal(s: State, n: usize) -> bool {
    s.worker[..n].iter().all(|&p| p == 5) && s.home == 2
}

// Independent resource-owner wait graph, rather than a second transition rule.
// An assistable home is runnable, so it has no wait edge while a Call is queued.
fn wait_cycle(s: State, n: usize, capacity: u32, mutation: Mutation) -> bool {
    let mut wait = vec![vec![false; n + 1]; n + 1];
    for (i, row) in wait.iter_mut().take(n).enumerate() {
        match s.worker[i] {
            0 | 4
                if (s.worker[i] == 4 || !s.stopped)
                    && s.permits & (1 << i) == 0
                    && s.permits.count_ones() == capacity =>
            {
                for (j, edge) in row.iter_mut().take(n).enumerate() {
                    *edge = s.permits & (1 << j) != 0;
                }
            }
            1 if !s.stopped => {
                if let Some(owner) = s.role {
                    row[owner] = true;
                }
            }
            3 => row[n] = true,
            _ => {}
        }
    }
    let queued = s.worker[..n].contains(&3);
    if s.home == 0 && (!queued || matches!(mutation, Mutation::StopServingCalls)) {
        if let Some(owner) = s.role {
            wait[n][owner] = true;
        }
    }
    for k in 0..=n {
        for i in 0..=n {
            for j in 0..=n {
                wait[i][j] |= wait[i][k] && wait[k][j];
            }
        }
    }
    (0..=n).any(|i| wait[i][i])
}

fn explore(n: usize, capacity: u32, mutation: Mutation) -> (usize, usize) {
    let initial = State {
        worker: [0; 3],
        home: 0,
        permits: 0,
        role: None,
        enqueued: 0,
        completed: 0,
        stopped: false,
    };
    let mut queue = VecDeque::from([initial]);
    let mut graph = BTreeMap::new();
    let mut bad = 0;
    while let Some(s) = queue.pop_front() {
        if graph.contains_key(&s) {
            continue;
        }
        assert!(s.permits.count_ones() <= capacity);
        let owners: Vec<_> = (0..n).filter(|&i| (2..=4).contains(&s.worker[i])).collect();
        assert!(owners.len() <= 1);
        assert_eq!(
            s.role,
            owners.first().copied().or((s.home == 1).then_some(n))
        );
        for i in 0..n {
            assert_eq!(
                s.completed & (1 << i) != 0,
                s.worker[i] >= 4 && s.enqueued & (1 << i) != 0
            );
            if s.worker[i] == 5 {
                assert_eq!(s.permits & (1 << i), 0);
            }
        }
        let next = successors(s, n, capacity, mutation);
        if wait_cycle(s, n, capacity, mutation) || (next.is_empty() && !terminal(s, n)) {
            bad += 1;
        }
        if terminal(s, n) {
            assert_eq!((s.role, s.permits), (None, 0));
            assert_eq!(s.enqueued, s.completed);
        }
        queue.extend(next.iter().copied());
        graph.insert(s, next);
    }
    if matches!(mutation, Mutation::None) {
        // Every reachable state can drain to a terminal state. There is no
        // retry/stutter transition: phases and the stop bit only advance.
        let mut draining: BTreeSet<_> = graph.keys().copied().filter(|s| terminal(*s, n)).collect();
        loop {
            let before = draining.len();
            for (&s, next) in &graph {
                if !next.is_empty() && next.iter().all(|s| draining.contains(s)) {
                    draining.insert(s);
                }
            }
            if draining.len() == before {
                break;
            }
        }
        assert_eq!(
            draining.len(),
            graph.len(),
            "nonterminating physical component"
        );
    }
    (graph.len(), bad)
}

#[test]
fn finite_compute_role_home_graph_drains_and_detects_both_deadlock_mutations() {
    let mut states = 0;
    for workers in 1..=3 {
        for capacity in 1..=workers as u32 {
            let (visited, bad) = explore(workers, capacity, Mutation::None);
            assert_eq!(bad, 0, "workers={workers} C={capacity}");
            states += visited;
        }
    }
    assert!(states > 100);
    assert!(
        explore(2, 1, Mutation::ReturnC).1 > 0,
        "R -> C / C -> R counterexample"
    );
    assert!(
        explore(2, 1, Mutation::StopServingCalls).1 > 0,
        "R -> home / home -> R counterexample"
    );
    eprintln!("PHYSICAL_MODEL {states} states; widths 1..3, stop before/after each boundary");
}
