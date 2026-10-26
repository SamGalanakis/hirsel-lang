//! NeedSave admits a retry only after both physical images are saved. The
//! notification never commits the retained operation or its original stretch.
use crate::budget::stretch::{Budgets, Settled, Stretch};
use crate::ordered::{Backend, Next, Operation, Progress, Receipt, Retirement};
use crate::tasks::ordered::{Command, Outcome, ahead, driver};
use crate::tasks::{Logical, Scheduler, Task, TaskKey, handle};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

struct Producer {
    saved: Arc<AtomicBool>,
    committed: usize,
    recorded: Vec<(u64, Stretch)>,
}
impl Backend<Command> for Producer {
    type Result = Outcome;
    fn can_retire(&mut self, _: u64, _: &Operation<Command>) -> bool {
        self.saved.load(Ordering::Acquire)
    }
    fn commit(&mut self, _: u64, _: &Operation<Command>, early: bool) -> Receipt<Outcome> {
        assert!(!early);
        self.committed += 1;
        Receipt {
            result: Outcome::image(0, vec![]),
            next: Next::Keep,
            credit: 0,
        }
    }
    fn record(&mut self, _: u64, _: &Operation<Command>, _: &Outcome, _: u64) {
        unreachable!()
    }
    fn record_retirement(
        &mut self,
        _: u64,
        _: &Operation<Command>,
        _: &Outcome,
        record: Retirement<'_>,
    ) {
        self.recorded.push((record.index, record.before.clone()));
    }
    fn publish(&mut self, _: u64, _: &Operation<Command>, _: &Outcome) {}
    fn defer_poll(&self) -> bool {
        true
    }
    fn poll(&mut self) -> Vec<u64> {
        panic!("a save notification cannot poll")
    }
}

#[test]
fn need_save_preserves_one_retirement_when_producer_or_waiter_saves_first() {
    for producer_first in [false, true] {
        let waiter = handle(0, 1);
        let producer = TaskKey::new(1, 2);
        let saved = Arc::new(AtomicBool::new(false));
        let mut backend = Producer {
            saved: saved.clone(),
            committed: 0,
            recorded: vec![],
        };
        let mut s = Scheduler::default();
        s.tasks = vec![
            Some(Task::new(1, std::ptr::null_mut(), None, vec![], None)),
            None,
        ];
        let mut driver = driver::Driver::new(
            waiter,
            crate::ordered::Policy::admit(&Default::default(), None),
            Settled {
                level: 0,
                limit: 100,
                fuel: 100,
            },
            crate::ordered::objects::Registry::default(),
        );
        driver.reserved.insert(
            1,
            ahead::Reserved {
                task: Box::new(Task::new(2, std::ptr::null_mut(), Some(0), vec![0], None)),
            },
        );
        assert_eq!(driver.select(), [waiter]);
        driver.claimed(waiter, 0);
        let mut before = Stretch::new(Budgets {
            fuel: true,
            memory: true,
        });
        before.fuel_points(7);
        before.alloc(12).unwrap();
        let operation = Command::AtomicLoad { word: 0, bytes: 8 }.operation(Default::default());
        assert_eq!(
            driver
                .frontier
                .as_mut()
                .unwrap()
                .apply_head(before.clone(), operation, &mut backend),
            Ok(Progress::NeedSave(waiter))
        );
        driver.awaiting_save = true;
        driver.suspending(waiter, true);
        s.logical = Logical::Ordered(driver);
        for is_producer in [producer_first, !producer_first] {
            let wakes = if is_producer {
                saved.store(true, Ordering::Release);
                ahead::after_save(&mut s, producer, false, false).unwrap()
            } else {
                driver::after_save(&mut s, 0, false, true, true)
            };
            let Logical::Ordered(d) = &mut s.logical else {
                unreachable!()
            };
            let f = d.frontier.as_ref().unwrap();
            assert_eq!(f.retired(), 0);
            assert_eq!((f.settled.fuel, f.settled.level), (100, 0));
            assert_eq!(backend.committed, 0);
            assert!(backend.recorded.is_empty());
            if is_producer == producer_first {
                assert!(wakes.is_empty(), "only one image saved");
                assert!(d.select().is_empty());
            } else {
                assert_eq!(
                    wakes.iter().map(|(key, _)| key.word()).collect::<Vec<_>>(),
                    [waiter]
                );
                assert!(d.select().is_empty(), "no duplicate notification");
            }
        }
        let Logical::Ordered(d) = &mut s.logical else {
            unreachable!()
        };
        assert!(!d.awaiting_save);
        d.claimed(waiter, 0);
        let f = d.frontier.as_mut().unwrap();
        assert_eq!(f.absorb(&mut backend), Ok(Progress::Absorbed(waiter)));
        assert_eq!(f.retired(), 0, "Keep absorbs without ending the turn");
        assert_eq!((f.settled.fuel, f.settled.level), (93, 12));
        assert_eq!(backend.committed, 1);
        assert_eq!(backend.recorded, [(0, before)]);
        assert_eq!(f.absorb(&mut backend), Ok(Progress::NeedSegment(waiter)));
        assert_eq!(backend.committed, 1);
        assert_eq!(
            f.continue_head(
                Stretch::new(Budgets {
                    fuel: true,
                    memory: true
                }),
                &mut backend,
                || Next::Yield
            ),
            Ok(Progress::Poll)
        );
        f.complete_poll(vec![]);
        assert_eq!(f.retired(), 1);
        assert_eq!((f.settled.fuel, f.settled.level), (93, 12));
        assert_eq!(backend.committed, 1);
    }
}
