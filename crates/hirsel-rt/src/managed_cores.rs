//! Live §7 policy. Resolution stays host-only; workers apply requests at a
//! scheduling boundary. Production remains at one until E1/F1's differential gate.
use std::fs::File;
use std::io::{self, Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::net::UnixStream;
use std::sync::{
    Arc, Mutex, OnceLock,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, Instant};

use crate::host::RunControl;
use crate::host_paths::ControlSocket;
use hirsel_run_config::Placement;
use hirsel_run_config::managed_cores::{
    ControlRequest, CoreResolver, HostGrant, HostLimits, LinuxLimits, MetricsSnapshot,
    MetricsWriter, Width,
};

struct State {
    resolver: CoreResolver,
    placement: Placement,
    grant: HostGrant,
    limits: HostLimits,
}

/// The manager's atomic placement transaction and injected-clock recheck seam.
/// `enable_workers` is an executor gate, independent of the run's identity.
pub struct ManagedCores {
    state: Mutex<State>,
    control: RunControl,
    enable_workers: bool,
    uid: libc::uid_t,
}
impl ManagedCores {
    pub fn admit(
        placement: Placement,
        grant: HostGrant,
        limits: HostLimits,
        now: Duration,
        control: RunControl,
        enable_workers: bool,
    ) -> Result<Self, String> {
        let resolver = CoreResolver::admit(placement.clone(), &grant, &limits, now)
            .map_err(|e| e.to_string())?;
        let width = Self::width(resolver.resolution().target, enable_workers)?;
        control.resize(width)?;
        Ok(Self {
            state: Mutex::new(State {
                resolver,
                placement,
                grant,
                limits,
            }),
            control,
            enable_workers,
            uid: unsafe { libc::geteuid() },
        })
    }
    fn width(target: u64, enabled: bool) -> Result<u32, String> {
        if !enabled {
            return Ok(1);
        }
        u32::try_from(target)
            .ok()
            .filter(|n| *n <= crate::pool::MAX_WIDTH)
            .ok_or_else(|| format!("worker target exceeds {}", crate::pool::MAX_WIDTH))
    }
    /// Rechecked at claim and settlement, at most once per second. Read errors leave the
    /// previous (never wider) configuration in place and are returned to the host.
    pub fn refresh(
        &self,
        now: Duration,
        read: impl FnOnce() -> Result<(HostGrant, HostLimits), String>,
    ) -> Result<bool, String> {
        let mut state = self.state.lock().unwrap();
        let mut fresh = None;
        let placement = state.placement.clone();
        let changed = state
            .resolver
            .refresh(now, || {
                let (grant, limits) =
                    read().map_err(|detail| hirsel_run_schema::RunRefusal::Invalid {
                        field: "placement".into(),
                        detail,
                    })?;
                let next =
                    hirsel_run_config::managed_cores::resolve(&placement, &grant, &limits, true)?;
                Self::width(next.target, self.enable_workers).map_err(|detail| {
                    hirsel_run_schema::RunRefusal::Invalid {
                        field: "placement.cores".into(),
                        detail,
                    }
                })?;
                fresh = Some((grant.clone(), limits.clone()));
                Ok((grant, limits))
            })
            .map_err(|e| e.to_string())?;
        if let Some((grant, limits)) = fresh {
            state.grant = grant;
            state.limits = limits;
        }
        self.control.resize(Self::width(
            state.resolver.resolution().target,
            self.enable_workers,
        )?)?;
        Ok(changed)
    }
    /// Validate the entire patch before changing anything. Destination changes
    /// require a new admission, and cannot smuggle paths through a live request.
    pub fn request(&self, bytes: &[u8]) -> Result<MetricsSnapshot, String> {
        let (cores, cpus) = match ControlRequest::from_json(bytes).map_err(|e| e.to_string())? {
            ControlRequest::Metrics => return Ok(self.snapshot()),
            ControlRequest::Placement { cores, cpus } => (cores, cpus),
        };
        let mut state = self.state.lock().unwrap();
        let mut placement = state.placement.clone();
        if let Some(cores) = cores {
            placement.cores = cores;
        }
        if let Some(cpus) = cpus {
            placement.cpus = cpus;
        }
        placement.validate().map_err(|e| e.to_string())?;
        let next = hirsel_run_config::managed_cores::resolve(
            &placement,
            &state.grant,
            &state.limits,
            false,
        )
        .map_err(|e| e.to_string())?;
        let width = Self::width(next.target, self.enable_workers)?;
        let (grant, limits) = (state.grant.clone(), state.limits.clone());
        state
            .resolver
            .replace(placement.clone(), &grant, &limits)
            .map_err(|e| e.to_string())?;
        state.placement = placement;
        self.control.resize(width)?;
        drop(state);
        Ok(self.snapshot())
    }
    pub(crate) fn admission_policy(&self) -> (Placement, u64) {
        let state = self.state.lock().unwrap();
        (state.placement.clone(), state.resolver.resolution().target)
    }
    pub fn snapshot(&self) -> MetricsSnapshot {
        let state = self.state.lock().unwrap();
        let resolution = state.resolver.resolution();
        let status = self.control.status();
        let (limit, _, _) = crate::pool::compute_permits();
        let lost = crate::pool::lost_compute_permits();
        let (live, phase) = self.control.worker_snapshot();
        let effective = u64::from(live.min(limit.saturating_sub(lost)));
        let target = resolution.target;
        let mut snapshot = MetricsSnapshot::new(
            Width {
                requested: resolution.requested.clone(),
                effective,
                pending: if effective == target { 0 } else { target },
            },
            resolution.cpus_conflict,
        );
        snapshot.worker_phase = Some(phase);
        snapshot.throttled_usec = state.limits.throttled_usec;
        snapshot.reclamation_pending = Some(status.reclamation_pending.into());
        snapshot.lost_permits = Some(lost.into());
        snapshot.home_busy = Some(status.home_busy);
        snapshot.stop_requested = Some(status.stop_requested);
        snapshot
    }
}

/// The run owns its admission snapshot through the final worker exit. Claims
/// and settlements refresh it synchronously; diagnostics never read capacity.
/// Go 1.25 runtime/proc.go:6733-6767 refreshes through its existing sysmon.
pub(crate) struct Service {
    reader: LinuxLimits,
    manager: Arc<ManagedCores>,
    started: Instant,
    failed: AtomicBool,
    pending: Mutex<Option<PendingDiagnostics>>,
    diagnostics: OnceLock<Option<Diagnostics>>,
}
struct PendingDiagnostics {
    writer: Option<File>,
    socket: Option<ControlSocket>,
    metrics: Option<Arc<crate::pool::WorkerMetrics>>,
}
impl Service {
    pub(crate) fn new(
        reader: LinuxLimits,
        manager: Arc<ManagedCores>,
        writer: Option<File>,
        socket: Option<ControlSocket>,
        metrics: Option<Arc<crate::pool::WorkerMetrics>>,
    ) -> Self {
        Self {
            reader,
            manager,
            started: Instant::now(),
            failed: AtomicBool::new(false),
            pending: Mutex::new(Some(PendingDiagnostics {
                writer,
                socket,
                metrics,
            })),
            diagnostics: OnceLock::new(),
        }
    }
    pub(crate) fn refresh(&self) -> bool {
        self.diagnostics.get_or_init(|| {
            let pending = self.pending.lock().unwrap().take().unwrap();
            if pending.writer.is_none() && pending.socket.is_none() {
                return None;
            }
            match Diagnostics::start(
                self.manager.clone(),
                pending.writer,
                pending.socket,
                pending.metrics,
            ) {
                Ok(diagnostics) => Some(diagnostics),
                Err(_) => {
                    self.failed.store(true, Ordering::Release);
                    self.manager.control.stop();
                    None
                }
            }
        });
        self.check(self.started.elapsed(), || {
            self.reader.read().map_err(|e| e.to_string())
        })
    }
    fn check(&self, now: Duration, read: impl FnOnce() -> Result<HostLimits, String>) -> bool {
        if self.failed.load(Ordering::Acquire) {
            return false;
        }
        let mut grant = self.manager.state.lock().unwrap().grant.clone();
        grant.max_cores = crate::pool::compute_permits().0.into();
        if self
            .manager
            .refresh(now, || read().map(|limits| (grant, limits)))
            .is_err()
        {
            // Preserve the service's read-error stop, including the previous width.
            self.failed.store(true, Ordering::Release);
            self.manager.control.stop();
            return false;
        }
        true
    }
    pub(crate) fn permits(&self, width: u32) -> bool {
        if !self.refresh() {
            return false;
        }
        let state = self.manager.state.lock().unwrap();
        state
            .limits
            .admission_affinity
            .intersection(&state.limits.effective_cpuset)
            .len()
            >= u64::from(width)
            && state
                .limits
                .quota
                .is_none_or(|quota| quota.floor() >= u64::from(width))
    }
}

/// Created on first use only when a sidecar was requested. Owns Send
/// diagnostics, never a run pointer, adapter, sink or capacity reader.
/// Owned by admission. Its service thread owns only Send diagnostics, never a
/// run pointer, adapter or sink. A shutdown descriptor wakes poll immediately.
struct Diagnostics {
    wake: UnixStream,
    thread: Option<std::thread::JoinHandle<()>>,
}
impl Diagnostics {
    pub(crate) fn start(
        manager: Arc<ManagedCores>,
        writer: Option<File>,
        socket: Option<ControlSocket>,
        metrics: Option<Arc<crate::pool::WorkerMetrics>>,
    ) -> io::Result<Self> {
        let (wake, stop) = UnixStream::pair()?;
        let thread = std::thread::Builder::new()
            .name("hirsel-diagnostics".into())
            .spawn(move || {
                let started = Instant::now();
                let initial = manager.snapshot().throttled_usec;
                let mut writer =
                    writer.map(|file| MetricsWriter::new(file, Duration::ZERO, initial));
                let mut tick = Instant::now() + Duration::from_secs(1);
                loop {
                    let mut descriptors = [
                        libc::pollfd {
                            fd: stop.as_raw_fd(),
                            events: libc::POLLIN,
                            revents: 0,
                        },
                        libc::pollfd {
                            fd: socket.as_ref().map_or(-1, |s| s.listener.as_raw_fd()),
                            events: libc::POLLIN,
                            revents: 0,
                        },
                    ];
                    let timeout = tick
                        .saturating_duration_since(Instant::now())
                        .as_millis()
                        .min(1000) as i32;
                    let result = unsafe { libc::poll(descriptors.as_mut_ptr(), 2, timeout) };
                    if result < 0 {
                        if io::Error::last_os_error().kind() == io::ErrorKind::Interrupted {
                            continue;
                        }
                        break;
                    }
                    if descriptors[0].revents != 0 {
                        break;
                    }
                    if descriptors[1].revents & libc::POLLIN != 0
                        && let Some(socket) = &socket
                        && let Ok((mut peer, _)) = socket.listener.accept()
                    {
                        let _ = serve_peer(&manager, &mut peer);
                    }
                    if Instant::now() >= tick {
                        let now = started.elapsed();
                        let mut snapshot = manager.snapshot();
                        if let Some(metrics) = &metrics {
                            for (name, value) in metrics.values() {
                                match name {
                                    "migrations_per_s" => snapshot.migrations_per_s = Some(value),
                                    "switches_per_op" => snapshot.switches_per_op = Some(value),
                                    _ => {}
                                }
                            }
                        }
                        let total = snapshot.throttled_usec;
                        if let Some(stream) = &mut writer
                            && stream.emit(now, total, &mut snapshot).is_err()
                        {
                            writer = None;
                        }
                        tick = Instant::now() + Duration::from_secs(1);
                    }
                }
            })?;
        Ok(Self {
            wake,
            thread: Some(thread),
        })
    }
}
impl Drop for Diagnostics {
    fn drop(&mut self) {
        let _ = self.wake.write_all(&[1]);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Linux peer credentials, same getsockopt(SO_PEERCRED) structure as Rust std's
/// os/unix/net/ucred.rs:49-78 peer_cred. Never trust request-supplied uid.
fn authorized(peer: &UnixStream, uid: libc::uid_t) -> io::Result<bool> {
    let mut credentials: libc::ucred = unsafe { std::mem::zeroed() };
    let mut size = std::mem::size_of_val(&credentials) as libc::socklen_t;
    if unsafe {
        libc::getsockopt(
            peer.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            (&raw mut credentials).cast(),
            &raw mut size,
        )
    } < 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok(size as usize == std::mem::size_of_val(&credentials) && credentials.uid == uid)
}
fn serve_peer(manager: &ManagedCores, peer: &mut UnixStream) -> io::Result<()> {
    if !authorized(peer, manager.uid)? {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "control uid differs from launcher",
        ));
    }
    peer.set_read_timeout(Some(Duration::from_millis(100)))?;
    peer.set_write_timeout(Some(Duration::from_millis(100)))?;
    let mut bytes = Vec::new();
    // One bounded JSON line per connection. Take the newline as the transaction
    // boundary; partial, oversized or pipelined requests never partly apply.
    let mut byte = [0u8];
    let deadline = Instant::now() + Duration::from_millis(100);
    while bytes.len() <= 16 * 1024 {
        if Instant::now() >= deadline {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "control request deadline",
            ));
        }
        peer.set_read_timeout(Some(
            deadline
                .saturating_duration_since(Instant::now())
                .max(Duration::from_millis(1)),
        ))?;
        if peer.read(&mut byte)? == 0 {
            break;
        }
        if byte[0] == b'\n' {
            break;
        }
        bytes.push(byte[0]);
    }
    let result = if bytes.len() > 16 * 1024 {
        Err("request too large".into())
    } else {
        manager.request(&bytes)
    };
    let mut reply = match result {
        Ok(snapshot) => serde_json::to_vec(&snapshot)?,
        Err(error) => serde_json::to_vec(&serde_json::json!({"error": error}))?,
    };
    reply.push(b'\n');
    peer.write_all(&reply)
}

#[cfg(test)]
mod width_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use hirsel_run_config::{Cores, managed_cores::CpuSet};
    #[test]
    fn socket_requests_use_peer_credentials_and_reply_without_waiting_for_eof() {
        let run = crate::host::BodyRun::new(1, false);
        let cpus = CpuSet::parse("0-7").unwrap();
        let manager = ManagedCores::admit(
            Placement::default(),
            HostGrant {
                max_cores: 8,
                cpus: cpus.clone(),
            },
            HostLimits {
                admission_affinity: cpus.clone(),
                effective_cpuset: cpus,
                quota: None,
                throttled_usec: 0,
            },
            Duration::ZERO,
            run.control(),
            false,
        )
        .unwrap();
        let (mut client, mut server) = UnixStream::pair().unwrap();
        assert!(authorized(&server, manager.uid).unwrap());
        assert!(!authorized(&server, manager.uid.wrapping_add(1)).unwrap());
        let peer = std::thread::spawn(move || {
            client
                .write_all(b"{\"placement\":{\"cores\":6}}\n")
                .unwrap();
            let mut reply = String::new();
            client.read_to_string(&mut reply).unwrap();
            let reply: serde_json::Value = serde_json::from_str(&reply).unwrap();
            assert_eq!(reply["width"]["requested"], 6);
        });
        serve_peer(&manager, &mut server).unwrap();
        drop(server);
        peer.join().unwrap();
        assert_eq!(manager.snapshot().width.requested, Cores::Fixed(6));
    }
}
