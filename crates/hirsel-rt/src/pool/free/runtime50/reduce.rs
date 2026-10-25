//! First-divergence shrinking: bounded delta debugging over a run's explicit
//! schedule choices, each candidate replayed through the production driver.
//!
//! `ddmin` ports picire's sequential DD (src/picire/dd.py:69-140 at 1e5b25c,
//! BSD-3-Clause; see third-party/NOTICES.md): Zeller and balanced splits,
//! combined subset/complement iteration with the complement offset, ddmin*,
//! a cache of passing configurations, and the deadline / test-count stop that
//! returns the smallest failing configuration found so far. Its vectors are
//! tests/test_api.py:16-94 and 110-133.
//!
//! A schedule is a run's choice values (`Controller::schedule`): every RNG
//! draw and the index of every `event` selection. Removing one shifts the
//! later values onto the next choice sites. Past the end every draw is 0 and
//! decisions go round-robin, so each candidate is a complete, fair run whose
//! transitions the production state enabled; with a constant 0, home could be
//! chosen forever while a loan it waits on is never served. A value out of
//! its site's bound makes the candidate invalid,
//! which is not a failure. A candidate matches only with the original's exact
//! failure class: the same first-divergence site, difference and competing
//! transition kinds, the same stranded lineages, or the same custody panic.
//!
//! Deviations from picire, each for the replayed predicate:
//! - A trial is pass, invalid, a different failure, or a match; ddmin sees a
//!   match as FAIL and the rest as PASS.
//! - The per-run assertion replay counts against `max_tests`, so the limit
//!   bounds every driver replay, not only uncached candidates.
//! - One exact-configuration cache stands for ConfigCache/ConfigTupleCache
//!   (passing outcomes only, picire's default); eviction after a failure only
//!   saves memory and is omitted.
//! - picire never tests the empty configuration and trusts its last
//!   granularity. Here every single removal of the result is tested, a failing
//!   one restarts ddmin, and `minimal` is set only when all of them pass.
//! - A driver panic is a custody failure class, not a reduction error.
use super::{Config, Divergence, Entry, Mutant, Mutation, Outcome, replay, run};
use std::collections::BTreeSet;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::time::Instant;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Verdict {
    Pass,
    Fail,
}

#[derive(Clone, Copy, Debug)]
pub(super) enum Split {
    /// Slice 1/n of the config, then 1/(n-1) of the rest (splitter.py:20-57).
    Zeller,
    /// Spread the division's residue over every chunk (splitter.py:60-87).
    Balanced,
}

#[derive(Clone, Copy, Debug)]
pub(super) enum Order {
    Forward,
    Backward,
    Skip,
}

impl Order {
    fn indices(self, n: usize) -> Vec<usize> {
        match self {
            Order::Forward => (0..n).collect(),
            Order::Backward => (0..n).rev().collect(),
            Order::Skip => Vec::new(),
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub(super) struct Options {
    pub(super) split: Split,
    /// The split ratio; `usize::MAX` splits to single units at once.
    pub(super) n: usize,
    pub(super) subset_first: bool,
    pub(super) subsets: Order,
    pub(super) complements: Order,
    pub(super) cache: bool,
    /// ddmin*: repeat whole iterations while one reduced the config.
    pub(super) star: bool,
}

impl Default for Options {
    /// picire's defaults: ZellerSplit(2), CombinedIterator(), ConfigCache().
    fn default() -> Self {
        Self {
            split: Split::Zeller,
            n: 2,
            subset_first: true,
            subsets: Order::Forward,
            complements: Order::Forward,
            cache: true,
            star: false,
        }
    }
}

/// LimitReduction (limit_reduction.py:14-46), checked before every test.
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct Limit {
    pub(super) deadline: Option<Instant>,
    pub(super) max_tests: Option<u64>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Stop {
    Deadline,
    MaxTests,
}

#[derive(Debug)]
pub(super) struct Reduced {
    /// The smallest failing configuration found; at a stop, a partial result.
    pub(super) config: Vec<usize>,
    pub(super) stopped: Option<Stop>,
    /// Every single removal of `config` was tested and none fails.
    pub(super) minimal: bool,
    pub(super) tests: u64,
}

struct Dd<'a> {
    test: &'a mut dyn FnMut(&[usize]) -> Verdict,
    options: Options,
    limit: Limit,
    tests: u64,
    passed: BTreeSet<Vec<usize>>,
}

/// Reduce `config` to a 1-minimal failing subsequence, or stop at `limit`.
pub(super) fn ddmin(
    test: &mut dyn FnMut(&[usize]) -> Verdict,
    config: Vec<usize>,
    options: Options,
    limit: Limit,
) -> Reduced {
    let mut dd = Dd {
        test,
        options,
        limit,
        tests: 0,
        passed: BTreeSet::new(),
    };
    let mut config = config;
    let stopped = 'reduce: loop {
        config = match dd.iterations(config) {
            Ok(config) => config,
            Err((partial, stop)) => {
                config = partial;
                break 'reduce Some(stop);
            }
        };
        for i in 0..config.len() {
            let mut removal = config.clone();
            removal.remove(i);
            match dd.cached(&removal) {
                Ok(Verdict::Pass) => {}
                Ok(Verdict::Fail) => {
                    config = removal;
                    continue 'reduce;
                }
                Err(stop) => break 'reduce Some(stop),
            }
        }
        break None;
    };
    Reduced {
        config,
        stopped,
        minimal: stopped.is_none(),
        tests: dd.tests,
    }
}

type Stopped = (Vec<usize>, Stop);

impl Dd<'_> {
    fn check_stop(&mut self) -> Result<(), Stop> {
        if self.limit.deadline.is_some_and(|d| Instant::now() >= d) {
            return Err(Stop::Deadline);
        }
        if let Some(left) = &mut self.limit.max_tests {
            if *left == 0 {
                return Err(Stop::MaxTests);
            }
            *left -= 1;
        }
        Ok(())
    }

    fn test(&mut self, config: &[usize]) -> Result<Verdict, Stop> {
        self.check_stop()?;
        self.tests += 1;
        Ok((self.test)(config))
    }

    /// `_lookup_cache`, else `_test_config` and its cache entry.
    fn cached(&mut self, config: &[usize]) -> Result<Verdict, Stop> {
        if self.options.cache && self.passed.contains(config) {
            return Ok(Verdict::Pass);
        }
        let verdict = self.test(config)?;
        if self.options.cache && verdict == Verdict::Pass {
            self.passed.insert(config.to_vec());
        }
        Ok(verdict)
    }

    fn split(&self, subsets: &[Vec<usize>]) -> Vec<Vec<usize>> {
        let config: Vec<usize> = subsets.concat();
        let length = config.len();
        let n = length.min(subsets.len().saturating_mul(self.options.n));
        match self.options.split {
            Split::Zeller => {
                let mut start = 0;
                (0..n)
                    .map(|i| {
                        let stop = start + (length - start) / (n - i);
                        let part = config[start..stop].to_vec();
                        start = stop;
                        part
                    })
                    .collect()
            }
            Split::Balanced => (0..n)
                .map(|i| config[length * i / n..length * (i + 1) / n].to_vec())
                .collect(),
        }
    }

    /// dd.py:69-140, `DD.__call__`.
    fn iterations(&mut self, mut config: Vec<usize>) -> Result<Vec<usize>, Stopped> {
        loop {
            let mut changed = false;
            let mut subsets = vec![config.clone()];
            let mut offset = 0;
            loop {
                match self.test(&config) {
                    Ok(verdict) => assert_eq!(verdict, Verdict::Fail, "a failing config replayed"),
                    Err(stop) => return Err((config, stop)),
                }
                if config.len() < 2 {
                    break;
                }
                if subsets.len() < 2 {
                    subsets = self.split(&subsets);
                }
                match self.reduce(&subsets, offset) {
                    Err(stop) => return Err((config, stop)),
                    Ok(Some((next, next_offset))) => {
                        changed = true;
                        subsets = next;
                        offset = next_offset;
                        config = subsets.concat();
                    }
                    Ok(None) if subsets.len() < config.len() => {
                        let next = self.split(&subsets);
                        offset = offset * next.len() / subsets.len();
                        subsets = next;
                    }
                    Ok(None) => break,
                }
            }
            if !self.options.star || !changed {
                return Ok(config);
            }
        }
    }

    /// dd.py:142-183, `_reduce_config`: the failing subset or complement.
    fn reduce(
        &mut self,
        subsets: &[Vec<usize>],
        offset: usize,
    ) -> Result<Option<(Vec<Vec<usize>>, usize)>, Stop> {
        let n = subsets.len();
        // CombinedIterator (iterator.py:62-103): `true` keeps subset i,
        // `false` removes it.
        let subsets_at = self
            .options
            .subsets
            .indices(n)
            .into_iter()
            .map(|i| (true, i));
        let complements_at = self
            .options
            .complements
            .indices(n)
            .into_iter()
            .map(|i| (false, i));
        let order: Vec<(bool, usize)> = if self.options.subset_first {
            subsets_at.chain(complements_at).collect()
        } else {
            complements_at.chain(subsets_at).collect()
        };
        for (keep, i) in order {
            if keep {
                if self.cached(&subsets[i])? == Verdict::Fail {
                    return Ok(Some((vec![subsets[i].clone()], 0)));
                }
                continue;
            }
            let i = (i + offset) % n;
            let rest: Vec<usize> = subsets
                .iter()
                .enumerate()
                .filter(|&(si, _)| si != i)
                .flat_map(|(_, s)| s.iter().copied())
                .collect();
            if self.cached(&rest)? == Verdict::Fail {
                // In the next run, start removing the following subset.
                let mut next = subsets.to_vec();
                next.remove(i);
                return Ok(Some((next, i)));
            }
        }
        Ok(None)
    }
}

/// How a checked replay's first divergent entry differs from its expectation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Differs {
    /// The same runnable members in another order.
    Order,
    /// The same runnable order, another selection.
    Selection,
    Members,
    Value,
    Entry,
    /// The expected transcript had ended.
    Extra,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum Failure {
    /// The site of the first divergent entry, how it differs and, for an
    /// `event` decision, the competing transition kinds as a bit set.
    Divergence {
        site: &'static str,
        differs: Differs,
        competing: u32,
    },
    /// The mutant's report: lineages left unfinished (`Outcome::stranded`).
    Stranded(Vec<(u32, bool, bool)>),
    /// A driver custody or teardown assertion.
    Custody(String),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum Trial {
    Pass,
    /// A choice out of its site's bound at this transcript index: the
    /// schedule names a transition the production state did not enable.
    Invalid(usize),
    Fail(Failure),
}

/// The driver run under test and its forced mutation.
#[derive(Clone, Copy, Debug)]
pub(super) struct Target {
    pub(super) config: Config,
    /// Checked against the same schedule's unmutated transcript.
    pub(super) mutation: Option<Mutation>,
}

fn site(entry: &Entry) -> &'static str {
    match entry {
        Entry::Random { site, .. }
        | Entry::Decision { site, .. }
        | Entry::Yield { site, .. }
        | Entry::Handoff { site, .. } => site,
        Entry::Selected { .. } => "selected",
    }
}

fn divergence(d: &Divergence) -> Failure {
    let competing = match &d.actual {
        Entry::Decision {
            site: "event",
            runnable,
            ..
        } => runnable.iter().fold(0, |set, e| set | 1 << (e >> 24)),
        _ => 0,
    };
    let differs = match (&d.expected, &d.actual) {
        (None, _) => Differs::Extra,
        (
            Some(Entry::Decision {
                site: was,
                runnable: expected,
                ..
            }),
            Entry::Decision { site, runnable, .. },
        ) if was == site => {
            let sorted = |r: &Vec<u32>| {
                let mut r = r.clone();
                r.sort_unstable();
                r
            };
            if expected == runnable {
                Differs::Selection
            } else if sorted(expected) == sorted(runnable) {
                Differs::Order
            } else {
                Differs::Members
            }
        }
        (Some(Entry::Random { site: was, .. }), Entry::Random { site, .. }) if was == site => {
            Differs::Value
        }
        _ => Differs::Entry,
    };
    Failure::Divergence {
        site: site(&d.actual),
        differs,
        competing,
    }
}

fn caught(f: impl FnOnce() -> Outcome) -> Result<Outcome, Failure> {
    catch_unwind(AssertUnwindSafe(f)).map_err(|panic| {
        let message = panic
            .downcast_ref::<&str>()
            .map(|s| s.to_string())
            .or_else(|| panic.downcast_ref::<String>().cloned())
            .unwrap_or_default();
        Failure::Custody(message)
    })
}

/// Replay `choices` through the production driver and classify the run.
pub(super) fn trial(target: &Target, choices: &[u64]) -> Trial {
    let mut expected = None;
    if target.mutation.is_some() {
        let reference = match caught(|| replay(target.config, choices, None, None)) {
            Ok(outcome) => outcome,
            Err(failure) => return Trial::Fail(failure),
        };
        if let Some(at) = reference.invalid {
            return Trial::Invalid(at);
        }
        expected = Some(reference.transcript);
    }
    let checked = match caught(|| replay(target.config, choices, expected, target.mutation)) {
        Ok(outcome) => outcome,
        Err(failure) => return Trial::Fail(failure),
    };
    match (&checked.divergence, checked.invalid) {
        (_, Some(at)) if checked.divergence.as_ref().is_none_or(|d| at <= d.index) => {
            Trial::Invalid(at)
        }
        (Some(d), _) => Trial::Fail(divergence(d)),
        _ if !checked.stranded.is_empty() => Trial::Fail(Failure::Stranded(checked.stranded)),
        _ => Trial::Pass,
    }
}

/// Only the original's exact failure class is a failure to ddmin.
pub(super) fn verdict(failure: &Failure, trial: &Trial) -> Verdict {
    match trial {
        Trial::Fail(f) if f == failure => Verdict::Fail,
        _ => Verdict::Pass,
    }
}

/// A transcript's explicit choices, as `Controller::schedule` replays them.
pub(super) fn choices(transcript: &[Entry]) -> Vec<u64> {
    transcript
        .iter()
        .filter_map(|entry| match entry {
            Entry::Random { value, .. } => Some(*value),
            Entry::Decision {
                site: "event",
                runnable,
                selected: Some(selected),
            } => runnable
                .iter()
                .position(|e| e == selected)
                .map(|i| i as u64),
            _ => None,
        })
        .collect()
}

#[derive(Debug, Default)]
pub(super) struct Counts {
    pub(super) matched: u64,
    pub(super) different: u64,
    pub(super) invalid: u64,
    pub(super) passed: u64,
}

pub(super) struct Shrunk {
    pub(super) failure: Failure,
    pub(super) choices: Vec<u64>,
    pub(super) reduced: Reduced,
    pub(super) counts: Counts,
}

/// Reduce a failing schedule, keeping its exact failure class. `original`
/// is only read.
pub(super) fn shrink(target: &Target, original: &[u64], limit: Limit) -> Shrunk {
    let Trial::Fail(failure) = trial(target, original) else {
        panic!("the original schedule must fail");
    };
    let mut counts = Counts::default();
    let mut test = |config: &[usize]| {
        let candidate: Vec<u64> = config.iter().map(|&i| original[i]).collect();
        let trial = trial(target, &candidate);
        match &trial {
            Trial::Fail(f) if *f == failure => counts.matched += 1,
            Trial::Fail(_) => counts.different += 1,
            Trial::Invalid(_) => counts.invalid += 1,
            Trial::Pass => counts.passed += 1,
        }
        verdict(&failure, &trial)
    };
    let reduced = ddmin(
        &mut test,
        (0..original.len()).collect(),
        Options::default(),
        limit,
    );
    let choices = reduced.config.iter().map(|&i| original[i]).collect();
    Shrunk {
        failure,
        choices,
        reduced,
        counts,
    }
}

// test_api.py:16-46.
fn interesting_a(c: &[u32]) -> bool {
    if c.contains(&7) && !c.contains(&2) {
        return false;
    }
    c.contains(&5) && c.contains(&8)
}
fn interesting_b(c: &[u32]) -> bool {
    !c.is_empty() && (1..=8).all(|d| c.contains(&d))
}
fn interesting_c(c: &[u32]) -> bool {
    [1, 2, 3, 4, 6, 8].iter().all(|d| c.contains(d))
}

type Case = (fn(&[u32]) -> bool, &'static [u32]);
const CONFIG: &[u32] = &[1, 2, 3, 4, 5, 6, 7, 8];
const CASES: [Case; 3] = [
    (interesting_a, &[5, 8]),
    (interesting_b, &[1, 2, 3, 4, 5, 6, 7, 8]),
    (interesting_c, &[1, 2, 3, 4, 6, 8]),
];

fn reduce(interesting: fn(&[u32]) -> bool, options: Options, limit: Limit) -> (Vec<u32>, Reduced) {
    let mut test = |config: &[usize]| {
        let content: Vec<u32> = config.iter().map(|&i| CONFIG[i]).collect();
        if interesting(&content) {
            Verdict::Fail
        } else {
            Verdict::Pass
        }
    };
    let reduced = ddmin(&mut test, (0..CONFIG.len()).collect(), options, limit);
    let output = reduced.config.iter().map(|&i| CONFIG[i]).collect();
    (output, reduced)
}

#[test]
fn runtime50_ddmin_vectors() {
    use Order::{Backward, Forward, Skip};
    use Split::{Balanced, Zeller};
    // test_api.py:58-94, TestReduction.test_dd: NoCache is `false`;
    // ConfigCache and ConfigTupleCache are both the exact cache.
    let combos = [
        (Balanced, true, Forward, Forward, false),
        (Zeller, true, Forward, Backward, true),
        (Balanced, false, Backward, Forward, true),
        (Zeller, false, Backward, Backward, false),
        (Balanced, true, Skip, Forward, true),
        (Zeller, true, Skip, Backward, true),
    ];
    let mut vectors = 0;
    for (interesting, expect) in CASES {
        for n in [2, usize::MAX] {
            for (split, subset_first, subsets, complements, cache) in combos {
                for star in [false, true] {
                    let options = Options {
                        split,
                        n,
                        subset_first,
                        subsets,
                        complements,
                        cache,
                        star,
                    };
                    let (output, reduced) = reduce(interesting, options, Limit::default());
                    assert_eq!(output, expect, "{options:?}");
                    assert!(reduced.minimal && reduced.stopped.is_none());
                    vectors += 1;
                }
            }
        }
    }
    assert_eq!(vectors, 72);
}

#[test]
fn runtime50_ddmin_limits() {
    // test_api.py:110-133, TestLimit: an expired deadline or no tests
    // left stops at once with the unreduced configuration.
    for (interesting, _) in CASES {
        for limit in [
            Limit {
                deadline: Some(Instant::now()),
                max_tests: None,
            },
            Limit {
                deadline: None,
                max_tests: Some(0),
            },
        ] {
            let (output, reduced) = reduce(interesting, Options::default(), limit);
            assert_eq!(output, CONFIG);
            assert!(reduced.stopped.is_some() && !reduced.minimal);
            assert_eq!(reduced.tests, 0);
        }
    }
    // A bounded count keeps the smallest failing configuration so far.
    let (complete, full) = reduce(interesting_a, Options::default(), Limit::default());
    assert_eq!(complete, [5, 8]);
    let mut partial = 0;
    for max in 1..full.tests {
        let limit = Limit {
            deadline: None,
            max_tests: Some(max),
        };
        let (output, reduced) = reduce(interesting_a, Options::default(), limit);
        assert_eq!(reduced.stopped, Some(Stop::MaxTests));
        assert_eq!(reduced.tests, max);
        assert!(interesting_a(&output), "a partial result still fails");
        assert!(!reduced.minimal);
        partial += u64::from(output.len() < CONFIG.len() && output != complete);
    }
    assert!(
        partial > 0,
        "some bound stops between the input and the result"
    );
}

/// A seed's transcript, its choices and their replay agree exactly.
#[test]
fn runtime50_schedule_replays_seeded_runs() {
    for seed in 0..16 {
        let config = Config::of(seed);
        let seeded = run(config, None, None);
        let schedule = choices(&seeded.transcript);
        let again = replay(config, &schedule, None, None);
        assert_eq!(again.invalid, None, "seed {seed}");
        assert_eq!(seeded.transcript, again.transcript, "seed {seed}");
        assert_eq!(seeded.projection, again.projection, "seed {seed}");
        let checked = replay(config, &schedule, Some(seeded.transcript), None);
        assert!(checked.divergence.is_none(), "seed {seed}");
    }
    // An out-of-bound choice is invalid, not a failure.
    let target = Target {
        config: Config::of(0),
        mutation: None,
    };
    assert_eq!(trial(&target, &[u64::MAX]), Trial::Invalid(0));
}

fn report(name: &str, original: &[u64], shrunk: &Shrunk) {
    eprintln!(
        "RUNTIME50 reduce {name}: {} -> {} choices, {} trials ({:?}), minimal {}, stopped {:?}, \
         {:?}; reduced {:?}",
        original.len(),
        shrunk.choices.len(),
        shrunk.reduced.tests,
        shrunk.counts,
        shrunk.reduced.minimal,
        shrunk.reduced.stopped,
        shrunk.failure,
        shrunk.choices
    );
}

/// The reduced schedule fails with the class twice, identically.
fn replays_exactly(target: &Target, shrunk: &Shrunk) {
    assert_eq!(
        trial(target, &shrunk.choices),
        Trial::Fail(shrunk.failure.clone())
    );
    let outcome = |choices: &[u64]| {
        let expected = target
            .mutation
            .map(|_| replay(target.config, choices, None, None).transcript);
        replay(target.config, choices, expected, target.mutation)
    };
    let (a, b) = (outcome(&shrunk.choices), outcome(&shrunk.choices));
    assert_eq!(a.transcript, b.transcript);
    assert_eq!(a.projection, b.projection);
    assert_eq!(
        a.divergence.as_ref().map(|d| d.index),
        b.divergence.as_ref().map(|d| d.index)
    );
    assert_eq!(a.stranded, b.stranded);
}

#[test]
fn runtime50_reduce_seed51_divergence() {
    // The protocol report's smallest mismatch: seed 51 under an
    // uncontrolled choice order, here at a fixed epoch.
    let target = Target {
        config: Config::of(51),
        mutation: Some(Mutation {
            epoch: 1,
            host_only: false,
        }),
    };
    let original = choices(&run(target.config, None, None).transcript);
    let kept = original.clone();
    let shrunk = shrink(&target, &original, Limit::default());
    report("seed 51 order", &original, &shrunk);
    assert_eq!(original, kept, "the unreduced original is preserved");
    assert_eq!(
        trial(&target, &original),
        Trial::Fail(shrunk.failure.clone())
    );
    assert!(matches!(
        shrunk.failure,
        Failure::Divergence {
            site: "event",
            differs: Differs::Order,
            ..
        }
    ));
    assert!(shrunk.reduced.minimal && shrunk.choices.len() < original.len());
    replays_exactly(&target, &shrunk);
}

fn save_ack(seed: u64) -> Target {
    Target {
        config: Config {
            stop: false,
            mutant: Some(Mutant::MissingSaveAck),
            ..Config::of(seed)
        },
        mutation: None,
    }
}

#[test]
fn runtime50_reduce_missing_save_ack() {
    // The first seed whose mutant strands lineage 0 behind a deferred
    // completion: its causal chain (host call, unacknowledged park,
    // completion at the save gate) must survive reduction.
    let (target, original) = (0..96)
        .map(|seed| {
            let target = save_ack(seed);
            let outcome = run(target.config, None, None);
            (target, outcome)
        })
        .find(|(_, outcome)| outcome.stranded.iter().any(|s| *s == (0, true, false)))
        .map(|(target, outcome)| (target, choices(&outcome.transcript)))
        .expect("a gated completion");
    let kept = original.clone();
    let shrunk = shrink(&target, &original, Limit::default());
    report(
        &format!("missing save ack seed {}", target.config.seed),
        &original,
        &shrunk,
    );
    assert_eq!(original, kept, "the unreduced original is preserved");
    let Failure::Stranded(stranded) = &shrunk.failure else {
        panic!("{:?}", shrunk.failure);
    };
    assert!(stranded.contains(&(0, true, false)));
    assert!(shrunk.reduced.minimal && shrunk.choices.len() < original.len());
    replays_exactly(&target, &shrunk);
    // Without the defect the reduced schedule finishes every lineage.
    let positive = replay(
        Config {
            mutant: None,
            ..target.config
        },
        &shrunk.choices,
        None,
        None,
    );
    assert!(positive.invalid.is_none() && positive.stranded.is_empty());

    // A different failure class is not a match: the order mutation on
    // the same schedule diverges instead of stranding.
    let other = Target {
        config: Config {
            mutant: None,
            ..target.config
        },
        mutation: Some(Mutation {
            epoch: 1,
            host_only: false,
        }),
    };
    let different = trial(&other, &shrunk.choices);
    assert!(matches!(different, Trial::Fail(Failure::Divergence { .. })));
    assert_eq!(verdict(&shrunk.failure, &different), Verdict::Pass);
    assert_eq!(verdict(&shrunk.failure, &Trial::Invalid(0)), Verdict::Pass);

    // A bounded trial count stops with an honest partial reduction.
    let bounded = shrink(
        &target,
        &original,
        Limit {
            deadline: None,
            max_tests: Some(8),
        },
    );
    report("missing save ack, 8 trials", &original, &bounded);
    assert_eq!(bounded.reduced.stopped, Some(Stop::MaxTests));
    assert!(!bounded.reduced.minimal && bounded.reduced.tests == 8);
    assert_eq!(
        trial(&target, &bounded.choices),
        Trial::Fail(shrunk.failure.clone())
    );
}
