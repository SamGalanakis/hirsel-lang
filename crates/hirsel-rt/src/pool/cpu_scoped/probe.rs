//! Diagnostic CPU intervals and operation-boundary events. No semantic input.
use crate::TaskCtx;
use std::io::Write;
use std::sync::{
    Mutex,
    atomic::{AtomicU64, Ordering},
};

pub(crate) struct Probe {
    output: std::fs::File,
    lifecycle: Mutex<std::fs::File>,
    boundaries: Mutex<std::fs::File>,
    controls: Mutex<std::fs::File>,
    threads: Mutex<std::fs::File>,
    jobs: std::sync::Arc<Mutex<std::fs::File>>,
    rejected_output: std::fs::File,
    rejected: [Mutex<Vec<(Sample, Sample)>>; 16],
    jitter: u64,
    checkpoints: AtomicU64,
    samples: [Mutex<Vec<(Sample, Sample, u64)>>; 16],
    next: AtomicU64,
}
#[derive(Clone, Copy)]
pub(crate) struct Sample {
    wall: u64,
    cpu_time: u64,
    switches: i64,
    cpu: i32,
    domain: u64,
    worker: u32,
}
impl Probe {
    pub(crate) fn open(path: &std::path::Path) -> std::io::Result<Self> {
        let mut lifecycle = path.as_os_str().to_os_string();
        lifecycle.push(".lifecycle");
        let mut boundaries = path.as_os_str().to_os_string();
        boundaries.push(".boundaries");
        let mut controls = path.as_os_str().to_os_string();
        controls.push(".control");
        let mut jobs = path.as_os_str().to_os_string();
        jobs.push(".jobs");
        let mut rejected = path.as_os_str().to_os_string();
        rejected.push(".rejected");
        let mut threads = path.as_os_str().to_os_string();
        threads.push(".threads");
        Ok(Self {
            output: std::fs::File::create(path)?,
            lifecycle: Mutex::new(std::fs::File::create(lifecycle)?),
            boundaries: Mutex::new(std::fs::File::create(boundaries)?),
            controls: Mutex::new(std::fs::File::create(controls)?),
            threads: Mutex::new(std::fs::File::create(threads)?),
            jobs: std::sync::Arc::new(Mutex::new(std::fs::File::create(jobs)?)),
            rejected_output: std::fs::File::create(rejected)?,
            rejected: std::array::from_fn(|_| Mutex::new(Vec::new())),
            jitter: std::env::var("HIRSEL_RT_CPU_JITTER")
                .ok()
                .and_then(|seed| seed.parse().ok())
                .unwrap_or(0),
            checkpoints: AtomicU64::new(0),
            samples: std::array::from_fn(|_| Mutex::new(Vec::new())),
            next: AtomicU64::new(1),
        })
    }
    pub(crate) fn domain(&self) -> u64 {
        self.next.fetch_add(1, Ordering::Relaxed)
    }

    pub(crate) fn event(&self, kind: &str, domain: u64, owner: usize, value: u64) {
        writeln!(
            self.lifecycle.lock().unwrap(),
            "{kind} {domain} {owner} {value}"
        )
        .expect("CPU lifecycle probe write");
    }

    pub(crate) fn control(&self, task: u64, width: u32, action: u32, count: u64, outcome: u32) {
        writeln!(
            self.controls.lock().unwrap(),
            "control {task} {width} {action} {count} {outcome}"
        )
        .expect("CPU control probe write");
    }

    pub(crate) fn job(
        &self,
        task: u64,
        batch: u64,
        width: u32,
        ordinal: u64,
        path: &[u32],
    ) -> std::sync::Arc<Job> {
        std::sync::Arc::new(Job {
            output: self.jobs.clone(),
            task,
            batch,
            width,
            ordinal,
            path: path
                .iter()
                .map(u32::to_string)
                .collect::<Vec<_>>()
                .join("."),
            entered: std::sync::atomic::AtomicBool::new(false),
        })
    }

    fn perturb(&self, domain: u64, worker: u32) {
        if self.jitter == 0 {
            return;
        }
        let ordinal = self.checkpoints.fetch_add(1, Ordering::Relaxed);
        let turns = (ordinal.wrapping_mul(6364136223846793005) ^ self.jitter) % 4 + 1;
        self.event("jitter", domain, worker as usize, turns);
        for _ in 0..turns {
            std::thread::yield_now();
        }
    }
}

/// A diagnostic handle follows the runtime row, never a saved task context.
/// Its generation-qualified identity is reconstructed from recorded controls
/// and the existing TaskFinish, completion and retirement boundaries in replay.
pub(crate) struct Job {
    output: std::sync::Arc<Mutex<std::fs::File>>,
    task: u64,
    batch: u64,
    width: u32,
    ordinal: u64,
    path: String,
    entered: std::sync::atomic::AtomicBool,
}
impl Job {
    pub(crate) fn enter(&self, width: u32, ordinal: u64) {
        assert_eq!((self.width, self.ordinal), (width, ordinal));
        assert!(
            !self.entered.swap(true, Ordering::Relaxed),
            "one mapper entry per generation"
        );
        self.event("generation");
        self.event("entry");
    }
    pub(crate) fn event(&self, kind: &str) {
        if !self.entered.load(Ordering::Relaxed) {
            return;
        }
        writeln!(
            self.output.lock().unwrap(),
            "{kind} {} {} {} {} {}",
            self.task,
            self.batch,
            self.width,
            self.ordinal,
            self.path
        )
        .expect("CPU job probe write");
    }
}

/// Called only while the run and executor are retained by the native worker.
pub(crate) unsafe fn worker(
    run: *mut crate::context::RunShared,
    exec: &super::super::Exec,
    kind: &str,
) {
    if let Some(worker) = exec.scoped_index()
        && let Some(probe) = unsafe { (*run).cpu_probe.as_ref() }
    {
        probe.event(kind, 0, exec as *const _ as usize, 0);
        if kind == "exit" {
            // Read on the retiring worker, after its measured work. An
            // external polling thread would preempt one of the exact CPUs.
            let mut snapshot = format!("worker {worker}\n");
            for name in ["comm", "sched", "schedstat", "status"] {
                if let Ok(text) = std::fs::read_to_string(format!("/proc/thread-self/{name}")) {
                    snapshot.push_str(&format!("{name}: {text}\n"));
                }
            }
            write!(probe.threads.lock().unwrap(), "{snapshot}").expect("CPU thread probe write");
        }
    }
}
#[cfg(target_os = "linux")]
impl Sample {
    fn now(domain: u64, worker: u32) -> Self {
        let mut usage = std::mem::MaybeUninit::<libc::rusage>::uninit();
        let mut wall = libc::timespec {
            tv_sec: 0,
            tv_nsec: 0,
        };
        let mut cpu_time = wall;
        unsafe {
            assert_eq!(libc::getrusage(libc::RUSAGE_THREAD, usage.as_mut_ptr()), 0);
            let usage = usage.assume_init();
            let cpu = libc::sched_getcpu();
            assert_eq!(
                libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &mut cpu_time),
                0
            );
            assert_eq!(libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut wall), 0);
            Self {
                wall: wall.tv_sec as u64 * 1_000_000_000 + wall.tv_nsec as u64,
                cpu_time: cpu_time.tv_sec as u64 * 1_000_000_000 + cpu_time.tv_nsec as u64,
                switches: usage.ru_nvcsw + usage.ru_nivcsw,
                cpu,
                domain,
                worker,
            }
        }
    }
    fn end(self) -> (Self, bool) {
        // The ending usage read is after the interval's end. The starting read
        // precedes its beginning, so unchanged switches prove no deschedule
        // throughout the reported wall interval. Migration requires a switch.
        let mut wall = libc::timespec {
            tv_sec: 0,
            tv_nsec: 0,
        };
        let mut cpu_time = wall;
        let mut usage = std::mem::MaybeUninit::<libc::rusage>::uninit();
        unsafe {
            assert_eq!(libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut wall), 0);
            assert_eq!(
                libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &mut cpu_time),
                0
            );
            let cpu = libc::sched_getcpu();
            assert_eq!(libc::getrusage(libc::RUSAGE_THREAD, usage.as_mut_ptr()), 0);
            let usage = usage.assume_init();
            let end = Self {
                wall: wall.tv_sec as u64 * 1_000_000_000 + wall.tv_nsec as u64,
                cpu_time: cpu_time.tv_sec as u64 * 1_000_000_000 + cpu_time.tv_nsec as u64,
                switches: usage.ru_nvcsw + usage.ru_nivcsw,
                cpu,
                ..self
            };
            let valid = self.cpu >= 0
                && end.cpu == self.cpu
                && end.switches == self.switches
                && end.wall > self.wall
                && end.cpu_time > self.cpu_time;
            (end, valid)
        }
    }
}
/// No role, heap, probe lock or Rust owner survives this checkpoint.
pub(crate) unsafe fn checkpoint(ctx: *mut TaskCtx) {
    #[cfg(target_os = "linux")]
    unsafe {
        let Some(start) = (*ctx).cpu_sample.take() else {
            return;
        };
        let Some(probe) = (*ctx).run_ref().cpu_probe.as_ref() else {
            return;
        };
        probe.perturb(start.domain, start.worker);
        let (end, valid) = start.end();
        if valid {
            let progress = end.cpu_time.saturating_sub(start.cpu_time);
            let mut samples = probe.samples[start.worker as usize].lock().unwrap();
            if samples.len() < 4096 {
                samples.push((start, end, progress));
            }
        } else {
            let mut rejected = probe.rejected[start.worker as usize].lock().unwrap();
            // At most 4096 rejected rows across the 16 workers. Accepted
            // samples retain their independent, unchanged 4096-per-worker cap.
            if rejected.len() < 256 {
                rejected.push((start, end));
            }
        }
        (*ctx).cpu_sample = Some(Sample::now(start.domain, start.worker));
    }
}
/// Record a CPU alternate's operation handoff even when its on-CPU interval
/// was rejected after a host context switch. File order is physical order only.
pub(crate) unsafe fn boundary(ctx: *mut TaskCtx, reason: u32) {
    #[cfg(target_os = "linux")]
    unsafe {
        if let Some(sample) = (*ctx).cpu_sample
            && let Some(probe) = (*ctx).run_ref().cpu_probe.as_ref()
        {
            writeln!(
                probe.boundaries.lock().unwrap(),
                "boundary {} {} {reason}",
                sample.domain,
                sample.worker
            )
            .expect("CPU boundary probe write");
        }
    }
}
/// Restart on the current physical owner, excluding time saved on idle.
pub(crate) unsafe fn resume(ctx: *mut TaskCtx) {
    #[cfg(target_os = "linux")]
    unsafe {
        if (*ctx).run_ref().cpu_probe.is_none() {
            return;
        }
        let Some(worker) = (*super::super::own_exec()).scoped_index() else {
            (*ctx).cpu_sample = None;
            return;
        };
        let domain = if let Some(start) = (*ctx).cpu_sample {
            Some(start.domain)
        } else {
            let execs = super::super::execs(ctx);
            let slot = execs.scoped.lock().unwrap();
            slot.as_ref()
                .filter(|w| w.caller_task != (*ctx).task_id)
                .map(|w| w.domain)
        };
        if let Some(domain) = domain {
            (*ctx)
                .run_ref()
                .cpu_probe
                .as_ref()
                .unwrap()
                .perturb(domain, worker);
            (*ctx).cpu_sample = Some(Sample::now(domain, worker));
        }
    }
}

pub(crate) unsafe fn begin(ctx: *mut TaskCtx, domain: u64, worker: u32) {
    #[cfg(target_os = "linux")]
    unsafe {
        if let Some(probe) = (*ctx).run_ref().cpu_probe.as_ref() {
            probe.perturb(domain, worker);
        }
        (*ctx).cpu_sample = Some(Sample::now(domain, worker));
    }
}

impl Drop for Probe {
    fn drop(&mut self) {
        for rejected in &mut self.rejected {
            for (start, end) in rejected.get_mut().unwrap() {
                writeln!(
                    self.rejected_output,
                    "rejected {} {} {} {} {} {} switches={} cpu_ns={}",
                    start.domain,
                    start.worker,
                    start.cpu,
                    end.cpu,
                    start.wall,
                    end.wall,
                    end.switches - start.switches,
                    end.cpu_time.saturating_sub(start.cpu_time),
                )
                .expect("CPU rejected interval probe write");
            }
        }
        for samples in &mut self.samples {
            for (start, end, progress) in samples.get_mut().unwrap() {
                writeln!(
                    self.output,
                    "cpu {} {} {} {} {} {}",
                    start.domain, start.worker, start.cpu, start.wall, end.wall, progress
                )
                .expect("CPU probe write");
            }
        }
    }
}
