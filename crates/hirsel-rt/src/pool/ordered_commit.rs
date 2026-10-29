//! Bounded synchronous callback loans follow pool::call_home_in/run_call.
//! Unlike home I/O, commit cannot withdraw halfway through restoring Frontier.
//! The issuer waits without accessing its loan, lending its existing permit
//! directly so a one-permit process cannot deadlock behind a role waiter.
use super::*;
use std::sync::mpsc::{self, Sender};

struct Action(*mut (dyn FnMut() + 'static));
// SAFETY: a synchronous sender retains the closure and all its loans until
// completion. Only the recipient executes it, once, with the scheduler role
// delegated by the waiting issuer. It can neither suspend nor escape a loan.
unsafe impl Send for Action {}
impl Action {
    unsafe fn run(self) {
        unsafe { (*self.0)() }
    }
}

enum Reply {
    Action(Action, Arc<Done>),
    Done,
}
struct Job {
    action: Action,
    reply: Sender<Reply>,
    issuer: Arc<Exec>,
}
struct Loan {
    reply: Sender<Reply>,
    issuer: Arc<Exec>,
    commit: Arc<Exec>,
    permits: Arc<Permits>,
}
thread_local! { static LOAN: Cell<*const Loan> = const { Cell::new(std::ptr::null()) }; }

pub(crate) struct Service {
    sender: Mutex<Option<Sender<Job>>>,
    thread: Mutex<Option<std::thread::JoinHandle<()>>>,
    exec: Arc<Exec>,
    permits: Arc<Permits>,
    #[cfg(test)]
    calls: AtomicU64,
}

fn transfer(permits: &Permits, from: &Exec, to: &Exec) {
    let _state = permits.state.lock().unwrap();
    assert!(
        from.held.swap(false, Ordering::AcqRel),
        "commit issuer holds a permit"
    );
    assert!(
        !to.held.swap(true, Ordering::AcqRel),
        "one commit permit loan"
    );
    assert!(
        !to.lost
            .swap(from.lost.swap(false, Ordering::AcqRel), Ordering::AcqRel)
    );
}

pub(crate) unsafe fn install(ctx: *mut TaskCtx) {
    let execs = unsafe { init_execs(ctx) };
    let exec = Exec::new(Kind::Commit, std::ptr::null_mut(), false);
    execs
        .holders
        .workers
        .lock()
        .unwrap()
        .push(Arc::downgrade(&exec));
    let permits = execs.permits.clone();
    let (sender, receiver) = mpsc::channel::<Job>();
    let dedicated = exec.clone();
    let limits = permits.clone();
    let thread = std::thread::Builder::new()
        .name("hirsel-ordered-commit".into())
        .spawn(move || {
            EXEC.with(|slot| slot.set(Arc::as_ptr(&dedicated)));
            while let Ok(job) = receiver.recv() {
                let loan = Loan {
                    reply: job.reply.clone(),
                    issuer: job.issuer,
                    commit: dedicated.clone(),
                    permits: limits.clone(),
                };
                LOAN.with(|slot| {
                    assert!(slot.replace(&loan).is_null());
                });
                unsafe { job.action.run() };
                LOAN.with(|slot| slot.set(std::ptr::null()));
                transfer(&limits, &dedicated, &loan.issuer);
                job.reply.send(Reply::Done).expect("live commit issuer");
            }
            EXEC.with(|slot| slot.set(std::ptr::null()));
        })
        .expect("Ordered commit thread");
    assert!(
        unsafe {
            (*ctx).run_ref().ordered_commit.set(Service {
                sender: Mutex::new(Some(sender)),
                thread: Mutex::new(Some(thread)),
                exec,
                permits,
                #[cfg(test)]
                calls: AtomicU64::new(0),
            })
        }
        .is_ok()
    );
}

impl Service {
    #[cfg(test)]
    pub(crate) fn calls(&self) -> u64 {
        self.calls.load(Ordering::Relaxed)
    }
    /// # Safety
    /// Issuer owns the role, with no role-owned borrow captured by the action.
    /// A foreign transition without a permit may release the role before the
    /// loan starts to acquire one. The bounded action retains no loans after
    /// return, invokes no user code and cannot suspend or raise a stop.
    pub(crate) unsafe fn call<T>(&self, ctx: *mut TaskCtx, action: impl FnOnce() -> T) -> T {
        assert!(!is_current());
        let execs = unsafe { (*ctx).run_ref().execs.as_deref().unwrap() };
        let current = current(execs);
        // Home/Pool foreign transitions legitimately execute without a
        // permit. Follow foreign_leave's Segment protocol: a last-permit
        // holder can be waiting for our role, so release it before waiting.
        // No Frontier or callback loan has started at this point.
        let took = if unsafe { (*current).held.load(Ordering::Acquire) } {
            false
        } else {
            let took = {
                let _segment = unsafe { crate::tasks::role::Segment::new(ctx) };
                acquire_permit(ctx)
            };
            // Raw idle/continuation role owners can have role_depth zero.
            unsafe { crate::tasks::role::enter(ctx) };
            took
        };
        // Execs retains this allocation until every issuer leaves. Create
        // the owned reference only after the stop-aware permit wait, which
        // can abandon this stack without unwinding its Rust locals.
        let issuer = unsafe {
            Arc::increment_strong_count(current);
            Arc::from_raw(current)
        };
        let (mut action, mut result) = (Some(action), None);
        let mut callback = || {
            result = Some(action.take().unwrap()());
        };
        let callback: *mut (dyn FnMut() + 'static) =
            unsafe { std::mem::transmute::<&mut dyn FnMut(), _>(&mut callback) };
        let (reply, receiver) = mpsc::channel();
        transfer(&self.permits, &issuer, &self.exec);
        self.sender
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .send(Job {
                action: Action(callback),
                reply,
                issuer: issuer.clone(),
            })
            .expect("live commit service");
        loop {
            match receiver.recv().expect("commit callback completion") {
                Reply::Done => break,
                Reply::Action(callback, done) => {
                    // A bounded head effect retains the issuer's physical
                    // thread/foreign context. Suppress automatic scheduler
                    // stop observation until commit restores Frontier.
                    let running = crate::stop::running();
                    crate::stop::set_running(std::ptr::null_mut());
                    unsafe { callback.run() };
                    crate::stop::set_running(running);
                    // As run_call: the borrowed callback has fully returned
                    // before its owner can destroy the closure or result.
                    done.ended.store(1, Ordering::Relaxed);
                    done.bell.notify();
                }
            }
        }
        #[cfg(test)]
        self.calls.fetch_add(1, Ordering::Relaxed);
        let result = result.expect("commit result");
        if took {
            self.permits.release(&issuer);
        }
        result
    }
}

/// Return an affine bounded head callback to its waiting issuer. Its caller
/// on the commit thread waits without touching the borrowed closure/result.
pub(crate) fn on_issuer<T>(action: impl FnOnce() -> T) -> T {
    let loan = LOAN.with(Cell::get);
    if loan.is_null() {
        return action();
    }
    let loan = unsafe { &*loan };
    let (mut action, mut result) = (Some(action), None);
    let done = Arc::new(Done::default());
    let mut callback = || {
        result = Some(action.take().unwrap()());
        transfer(&loan.permits, &loan.issuer, &loan.commit);
    };
    let callback: *mut (dyn FnMut() + 'static) =
        unsafe { std::mem::transmute::<&mut dyn FnMut(), _>(&mut callback) };
    transfer(&loan.permits, &loan.commit, &loan.issuer);
    loan.reply
        .send(Reply::Action(Action(callback), done.clone()))
        .expect("waiting commit issuer");
    done.bell.wait();
    result.expect("affine callback result")
}

pub(crate) fn is_current() -> bool {
    LOAN.with(|slot| !slot.get().is_null())
}

impl Drop for Service {
    fn drop(&mut self) {
        // No active callback can remain: every issuer retains the run while
        // synchronously waiting. The idle thread owns no run or home state.
        self.sender.get_mut().unwrap().take();
        self.thread
            .get_mut()
            .unwrap()
            .take()
            .unwrap()
            .join()
            .expect("commit thread exit");
        assert!(!self.exec.held.load(Ordering::Acquire));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dedicated_commit_without_an_issuer_permit_releases_the_role_before_waiting() {
        const CHILD: &str = "HIRSEL_ORDERED_COMMIT_ROLE_PERMIT";
        if std::env::var_os(CHILD).is_none() {
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "pool::ordered_commit::tests::dedicated_commit_without_an_issuer_permit_releases_the_role_before_waiting", "--nocapture"])
                .env(CHILD, "1").output().unwrap();
            assert!(
                output.status.success(),
                "{}{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            return;
        }
        set_compute_grant(1);
        let mut run = crate::host::BodyRun::new(1, true);
        unsafe { run.use_physical_dispatch() };
        let outcome = run.run(
            |ctx| unsafe {
                let mut policy = crate::ordered::Policy::admit(&Default::default(), None);
                policy.commit = hirsel_run_config::Commit::Dedicated;
                crate::tasks::ordered::engine::enable(ctx, policy);
            },
            |ctx| unsafe {
                let _role = crate::tasks::role::Guard::new(ctx);
                let service = (*ctx).run_ref().ordered_commit.get().unwrap();
                let permits = service.permits.clone();
                let shared = RunPtr((*ctx).run_ptr());
                let (ready, waiting) = mpsc::channel();
                release_permit(ctx);
                let contender = std::thread::spawn(move || {
                    let shared = shared;
                    let exec = Exec::new(Kind::Compute(0), std::ptr::null_mut(), false);
                    assert!(permits.acquire(&exec, None, None).unwrap());
                    ready.send(()).unwrap();
                    // This holder needs the role before it can give back the
                    // process's last permit. The issuer must let it proceed.
                    let role = crate::tasks::role::IdleGuard::new(shared.0);
                    drop(role);
                    permits.release(&exec);
                });
                waiting.recv_timeout(Duration::from_secs(5)).unwrap();
                let control = (*ctx).run_ref().control.clone();
                let (done, timeout) = mpsc::channel();
                let watchdog = std::thread::spawn(move || {
                    if timeout.recv_timeout(Duration::from_secs(5)).is_err() {
                        control.request(crate::stop::Stop::Requested);
                    }
                });
                assert_eq!(
                    service.call(ctx, || {
                        assert_eq!(compute_permits(), (1, 1, 0));
                        42
                    }),
                    42
                );
                done.send(()).unwrap();
                watchdog.join().unwrap();
                contender.join().unwrap();
                assert_eq!(compute_permits(), (1, 0, 0));
                // This fixture returns to ordinary code rather than C.
                let _segment = crate::tasks::role::Segment::new(ctx);
                acquire_permit(ctx);
                crate::STATUS_OK
            },
        );
        assert_eq!(outcome.ended, Ok(crate::STATUS_OK));
        assert_eq!(outcome.pending, 0);
        assert_eq!(compute_permits(), (1, 0, 0));
    }
}
