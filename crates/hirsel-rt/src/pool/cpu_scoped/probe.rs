//! Diagnostic on-CPU intervals for CPU-batch acceptance. No semantic input.
use crate::TaskCtx;
use std::io::Write;
use std::sync::{
    Mutex,
    atomic::{AtomicU64, Ordering},
};

pub(crate) struct Probe {
    output: std::fs::File,
    lifecycle: Mutex<std::fs::File>,
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
        Ok(Self {
            output: std::fs::File::create(path)?,
            lifecycle: Mutex::new(std::fs::File::create(lifecycle)?),
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

/// Called only while the run and executor are retained by the native worker.
pub(crate) unsafe fn worker(
    run: *mut crate::context::RunShared,
    exec: &super::super::Exec,
    kind: &str,
) {
    if matches!(exec.kind, super::super::Kind::Scoped(_))
        && let Some(probe) = unsafe { (*run).cpu_probe.as_ref() }
    {
        probe.event(kind, 0, exec as *const _ as usize, 0);
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
    fn end(self) -> Option<(Self, u64)> {
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
            (self.cpu >= 0
                && end.cpu == self.cpu
                && end.switches == self.switches
                && end.wall > self.wall
                && end.cpu_time > self.cpu_time)
                .then_some((end, end.cpu_time.saturating_sub(self.cpu_time)))
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
        if let Some((end, progress)) = start.end() {
            let mut samples = probe.samples[start.worker as usize].lock().unwrap();
            if samples.len() < 4096 {
                samples.push((start, end, progress));
            }
        }
        (*ctx).cpu_sample = Some(Sample::now(start.domain, start.worker));
    }
}
/// Restart on the current physical owner, excluding time saved on idle.
pub(crate) unsafe fn resume(ctx: *mut TaskCtx) {
    #[cfg(target_os = "linux")]
    unsafe {
        if (*ctx).run_ref().cpu_probe.is_none() {
            return;
        }
        let super::super::Kind::Scoped(worker) = (*super::super::own_exec()).kind else {
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
                .perturb(domain, u32::from(worker));
            (*ctx).cpu_sample = Some(Sample::now(domain, u32::from(worker)));
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
