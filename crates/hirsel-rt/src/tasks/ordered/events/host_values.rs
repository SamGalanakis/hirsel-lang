//! Semantic result projection using the established tape payload codec.
//! This is only the result: requests and resource keys must also be certified
//! before a host operation can enter the complete canonical event stream.
use crate::io::{AdapterError, Completion};
use crate::ordered::footprint::ObjectId;
use crate::tape_wire::Wire;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(in crate::tasks::ordered) struct HostValue {
    /// Tape payload with every initialized handle slot replaced by zero.
    pub payload: Vec<u8>,
    /// Logical identities in the same order as the initialized handle slots.
    pub handles: Vec<ObjectId>,
}

pub(super) fn project(
    result: &Result<Completion, AdapterError>,
    mut identity: impl FnMut(u64) -> Option<ObjectId>,
) -> Result<HostValue, &'static str> {
    let mut result = result.clone();
    crate::io_observation::capture(&mut result).map_err(|_| "invalid host mapping")?;
    let mut handles = Vec::new();
    if let Err(AdapterError::InvalidHandle(word)) = &mut result {
        handles.push(identity(*word).ok_or("unmapped host refusal")?);
        *word = 0;
    } else {
        for word in crate::io_observation::fields(&mut result)?
            .into_iter()
            .flatten()
        {
            handles.push(identity(*word).ok_or("unmapped host result")?);
            *word = 0;
        }
    }
    let mut payload = Vec::new();
    result.encode(&mut payload);
    Ok(HostValue { payload, handles })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::io::{Adapter, Request};
    use crate::tasks::ordered::{engine, events, host};

    fn id(ordinal: u64) -> ObjectId {
        ObjectId {
            creator: vec![2, 1],
            ordinal,
        }
    }

    #[test]
    fn host_result_projection_replaces_all_handles_and_preserves_initialized_metadata() {
        let address = crate::net::SocketAddr::of("[::1]:4488".parse().unwrap());
        for make in [
            |word| Ok(Completion::Handle(Ok(word))),
            |word| Ok(Completion::ClockGrant(Ok(word))),
            |word| Ok(Completion::OptionalFile(Some(word))),
            |word| Ok(Completion::NetHandle(Ok(word))),
            |word| Ok(Completion::Spawned(Ok(word))),
            |word| Err(AdapterError::InvalidHandle(word)),
        ] {
            let a = project(&make(0x1234_5678_9abc_def0), |_| Some(id(7))).unwrap();
            let b = project(&make(0xfedc_ba98_7654_3210), |_| Some(id(7))).unwrap();
            assert_eq!(a, b);
            assert_eq!(a.handles, [id(7)]);
            assert_eq!(
                crate::tape_wire::decode::<Result<Completion, AdapterError>>(&a.payload).unwrap(),
                make(0)
            );
            assert!(project(&make(99), |_| None).is_err());
        }
        for result in [
            Completion::Listened(Ok((99, address))),
            Completion::Accepted(Ok((99, address))),
            Completion::Span {
                handle: 99,
                trace: [17; 16],
                span: [23; 8],
            },
        ] {
            let projected = project(&Ok(result.clone()), |_| Some(id(8))).unwrap();
            let mut expected = Ok(result);
            *crate::io_observation::fields(&mut expected).unwrap()[0]
                .take()
                .unwrap() = 0;
            assert_eq!(
                crate::tape_wire::decode::<Result<Completion, AdapterError>>(&projected.payload)
                    .unwrap(),
                expected
            );
            assert_eq!(projected.handles, [id(8)]);
        }
        let halves = Ok(Completion::Halves(Ok((99, 100))));
        assert!(project(&halves, |word| (word == 99).then(|| id(9))).is_err());
        let projected = project(&halves, |word| Some(id(word - 90))).unwrap();
        assert_eq!(projected.handles, [id(9), id(10)]);
        assert_eq!(
            crate::tape_wire::decode::<Result<Completion, AdapterError>>(&projected.payload)
                .unwrap(),
            Ok(Completion::Halves(Ok((0, 0))))
        );
        let alias = project(&halves, |_| Some(id(9))).unwrap();
        assert_eq!(alias.handles, [id(9), id(9)]);
        assert_eq!(halves, Ok(Completion::Halves(Ok((99, 100)))));
    }

    #[test]
    fn host_result_projection_owns_payloads_and_rejects_unclassified_carriers() {
        for result in [
            Ok(Completion::Bytes(Ok(vec![0, 255, 7]))),
            Ok(Completion::View(crate::io::MapView::of(vec![3, 0, 255]))),
            Ok(Completion::OptionalFile(None)),
            Ok(Completion::Handle(Err(crate::io::IoError::NotFound))),
            Ok(Completion::Wait(Err(crate::io::WaitError::Canceled(7)))),
            Err(AdapterError::InvalidCompletion),
        ] {
            let value = project(&result, |_| panic!("scalar has no handles")).unwrap();
            assert!(value.handles.is_empty());
            assert_eq!(
                crate::tape_wire::decode::<Result<Completion, AdapterError>>(&value.payload)
                    .unwrap(),
                result
            );
        }
        for result in [
            Ok(Completion::Refused(99)),
            Ok(Completion::Blocked { committed: false }),
            Ok(Completion::Ready),
            Ok(Completion::View(crate::io::MapView {
                length: 1,
                bytes: None,
                block: 0,
            })),
            Ok(Completion::View(crate::io::MapView {
                length: 2,
                bytes: Some(vec![1]),
                block: 0,
            })),
        ] {
            assert!(project(&result, |_| Some(id(9))).is_err());
        }
    }

    struct Results {
        base: u64,
        home: std::thread::ThreadId,
    }
    impl Adapter for Results {
        fn preflight(&self, _: &Request) -> Result<Option<Completion>, AdapterError> {
            Ok(None)
        }
        fn call(&mut self, request: Request) -> Result<Completion, AdapterError> {
            assert_eq!(std::thread::current().id(), self.home);
            let Request::TestCheck { test, .. } = request else {
                panic!("test request")
            };
            match test {
                0 => Ok(Completion::Handle(Ok(self.base))),
                1 => Ok(Completion::Handle(Ok(self.base + 1))),
                2 => Ok(Completion::Halves(Ok((self.base, self.base + 2)))),
                3 => Err(AdapterError::InvalidHandle(self.base)),
                4 => Ok(Completion::Bytes(Ok(vec![0, 255, 7]))),
                5 => Ok(Completion::Halves(Ok((self.base, self.base + 3)))),
                _ => unreachable!(),
            }
        }
    }

    #[test]
    fn native_host_results_ignore_physical_words_and_require_complete_identities() {
        use std::sync::{Arc, Mutex};
        let mut reference = None;
        for width in [1, 2, 4, 8] {
            for reorder in [
                hirsel_run_config::Reorder::Off,
                hirsel_run_config::Reorder::Auto,
            ] {
                let base = 0xa123_4567_8900_0000
                    + width * 256
                    + u64::from(reorder == hirsel_run_config::Reorder::Auto) * 16;
                let output = Arc::new(Mutex::new(Vec::new()));
                let seen = output.clone();
                let mut run = crate::host::BodyRun::new(width as u32, true);
                run.adapter = Some(Box::new(Results {
                    base,
                    home: std::thread::current().id(),
                }));
                unsafe { run.use_physical_dispatch() };
                let outcome = run.run(
                    move |ctx| unsafe {
                        let mut policy = crate::ordered::Policy::admit(&Default::default(), None);
                        policy.reorder = reorder;
                        engine::enable(ctx, policy);
                        events::enable(ctx);
                        let objects = &mut engine::driver(ctx).objects;
                        for (word, ordinal) in [(base, 7), (base + 1, 7), (base + 2, 8)] {
                            objects.bind_erased(
                                crate::ordered::objects::Kind::Capability,
                                word,
                                id(ordinal),
                                Default::default(),
                            );
                        }
                    },
                    move |ctx| unsafe {
                        for test in 0..6 {
                            let (_, parked) = crate::host_waits::issue(
                                ctx,
                                Request::TestCheck {
                                    test,
                                    condition: true,
                                },
                            )
                            .unwrap();
                            assert!(!parked);
                            host::finish(ctx, crate::ordered::Next::Keep).unwrap();
                        }
                        let driver = engine::driver(ctx);
                        assert!(driver.host_ops.is_empty());
                        let log = driver.events.as_ref().unwrap();
                        let observed: Vec<_> = log
                            .host_completions
                            .iter()
                            .map(|row| (row.ordinal, row.observation.clone()))
                            .collect();
                        assert_eq!(
                            observed.iter().map(|row| row.0).collect::<Vec<_>>(),
                            [0, 1, 2, 3, 4, 5]
                        );
                        assert_eq!(
                            observed[0].1, observed[1].1,
                            "aliases have one logical result"
                        );
                        assert_eq!(observed[2].1.as_ref().unwrap().handles, [id(7), id(8)]);
                        assert_eq!(observed[3].1.as_ref().unwrap().handles, [id(7)]);
                        assert!(observed[4].1.as_ref().unwrap().handles.is_empty());
                        assert!(
                            observed[5].1.is_none(),
                            "an unmapped second field rejects the whole result"
                        );
                        assert!(
                            log.canonical().is_err(),
                            "results alone cannot certify host requests or keys"
                        );
                        *seen.lock().unwrap() = observed;
                        crate::STATUS_OK
                    },
                );
                assert_eq!(outcome.ended, Ok(crate::STATUS_OK));
                assert_eq!(outcome.pending, 0);
                let observed = output.lock().unwrap().clone();
                if let Some(reference) = &reference {
                    assert_eq!(&observed, reference);
                } else {
                    reference = Some(observed);
                }
            }
        }
    }
}
