//! CAP-3 signal projection controls, using grants' existing byte-stream fixtures.
use super::grants::{Draw, record, signal_grant, valid};
use hirsel_run_config::{RunConfig, grants_within};
use hirsel_run_schema::{Access, IoGrants, Mount, SignalKind, TelemetryGrant};
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug)]
struct Cell {
    io: bool,
    env: bool,
    // No mount, read, read_write at the same virtual and host path.
    mount: u8,
    // 0 absent, 1 host, 2 script; mask's independent bits are I=1, T=2, H=4.
    provider: usize,
    mask: u8,
}

impl Cell {
    fn record(self) -> RunConfig {
        let mut r = record(&mut Draw(&[]));
        r.env = self.env.then(BTreeMap::new);
        r.io = self.io.then(|| IoGrants {
            dirs: if self.mount == 0 {
                vec![]
            } else {
                vec![Mount {
                    path: "data".into(),
                    host: "/h/data".into(),
                    access: if self.mount == 1 {
                        Access::Read
                    } else {
                        Access::ReadWrite
                    },
                }]
            },
            signals: (self.provider != 0)
                .then(|| signal_grant(self.provider, self.mask, &mut Draw(&[]))),
            ..IoGrants::default()
        });
        r
    }

    fn within(self, b: Self) -> bool {
        (!self.io || b.io)
            && (!self.env || b.env)
            && self.mount <= b.mount
            && (self.provider == 0 || (self.provider == b.provider && self.mask & !b.mask == 0))
    }
}

#[test]
fn every_signal_subset_and_provider_pair_matches_the_finite_model() {
    // Rust alloc btree/set/tests.rs:284-304, f6c1107f8be1 (MIT/Apache-2.0):
    // empty/equal, smaller/larger, missing endpoints and interior holes. Exhaust
    // those boundaries over three kinds; absence represents empty authority,
    // because a present grant with no kinds is invalid in Hirsel.
    let mut cells = vec![];
    for env in [false, true] {
        cells.push(Cell {
            io: false,
            env,
            mount: 0,
            provider: 0,
            mask: 0,
        });
        for mount in 0..3 {
            cells.push(Cell {
                io: true,
                env,
                mount,
                provider: 0,
                mask: 0,
            });
            for provider in 1..=2 {
                for mask in 1..=7 {
                    cells.push(Cell {
                        io: true,
                        env,
                        mount,
                        provider,
                        mask,
                    });
                }
            }
        }
    }
    assert_eq!(cells.len(), 92);
    let runs: Vec<_> = cells.iter().map(|c| valid(&c.record())).collect();
    let mut relation = vec![vec![false; cells.len()]; cells.len()];
    for (a, left) in cells.iter().enumerate() {
        for (b, right) in cells.iter().enumerate() {
            let actual = grants_within(&runs[a], &runs[b]);
            assert_eq!(
                actual,
                left.within(*right),
                "left={left:?}, right={right:?}"
            );
            relation[a][b] = actual;
        }
        assert!(relation[a][a], "reflexivity: {left:?}");
    }
    for (a, ab) in relation.iter().enumerate() {
        for (b, bc) in relation.iter().enumerate() {
            for c in 0..cells.len() {
                assert!(
                    !(ab[b] && bc[c]) || ab[c],
                    "transitivity: {:?} -> {:?} -> {:?}",
                    cells[a],
                    cells[b],
                    cells[c]
                );
            }
        }
    }
}

#[test]
fn changing_script_order_and_content_preserves_both_subset_directions() {
    use SignalKind::{Hangup, Interrupt, Terminate};
    let scripts = [
        vec![],
        vec![Interrupt, Terminate, Hangup],
        vec![Hangup, Terminate, Interrupt],
        vec![Terminate, Terminate],
    ];
    for left in 1..=7 {
        for right in 1..=7 {
            let cell = |mask| Cell {
                io: true,
                env: true,
                mount: 1,
                provider: 2,
                mask,
            };
            let expected = left & !right == 0;
            let reverse = right & !left == 0;
            for a in &scripts {
                for b in &scripts {
                    let scripted = |mask, script: &Vec<SignalKind>| {
                        let mut r = cell(mask).record();
                        r.io.as_mut().unwrap().signals.as_mut().unwrap().script =
                            Some(script.clone());
                        valid(&r)
                    };
                    let (a, b) = (scripted(left, a), scripted(right, b));
                    assert_eq!(
                        grants_within(&a, &b),
                        expected,
                        "left={left}, right={right}"
                    );
                    assert_eq!(grants_within(&b, &a), reverse, "left={left}, right={right}");
                }
            }
        }
    }
}

#[test]
fn telemetry_currently_compares_the_complete_validated_record() {
    // Inventory of current behavior, not a new CAP-3 weakening rule. In particular,
    // do not apply signal-script independence to telemetry's seed or queue.
    let null = |seed, queue| TelemetryGrant {
        provider: "telemetry.null@1".into(),
        seed: Some(seed),
        queue,
    };
    let grants = [
        None,
        Some(TelemetryGrant {
            provider: "telemetry.capture@1".into(),
            seed: None,
            queue: None,
        }),
        Some(null(0, None)),
        Some(null(1, None)),
        Some(null(0, Some(0))),
        Some(null(0, Some(1))),
        Some(null(0, Some(2))),
    ];
    let runs: Vec<_> = grants
        .iter()
        .map(|telemetry| {
            let mut r = Cell {
                io: true,
                env: false,
                mount: 0,
                provider: 0,
                mask: 0,
            }
            .record();
            r.io.as_mut().unwrap().telemetry = telemetry.clone();
            valid(&r)
        })
        .collect();
    for (a, left) in grants.iter().enumerate() {
        for (b, right) in grants.iter().enumerate() {
            assert_eq!(
                grants_within(&runs[a], &runs[b]),
                left.is_none() || a == b,
                "left={left:?}, right={right:?}"
            );
        }
    }
}
