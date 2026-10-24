//! Protocol-independent serial connection over owned candidate leases. Snapshot
//! handles follow rust-analyzer 3131751e ide/src/lib.rs:192-202,238-252; this
//! adapter keeps old snapshots alive instead of globally cancelling them.
use super::*;
mod v2;
use serde_json::{Value, json};
use std::sync::Condvar;

/// Preserve permissive v1 JSON; v2 must not overwrite duplicate request fields.
pub fn decode_frame(bytes: &[u8]) -> Result<Value, serde_json::Error> {
    let value: Value = serde_json::from_slice(bytes)?;
    if value["version"] == 2 {
        hirsel_engine::decode_unique_json(bytes)
    } else {
        Ok(value)
    }
}

/// Transport preserves a decoded v2 identity so duplicate keys receive an
/// InvalidRequest reply on the latched connection. No overwritten field can
/// reach candidate dispatch. Invalid UTF-8/JSON remains a framing refusal.
pub fn decode_wire_frame(bytes: &[u8]) -> Result<Value, serde_json::Error> {
    let mut value: Value = serde_json::from_slice(bytes)?;
    if value["version"] == 2 && hirsel_engine::decode_unique_json(bytes).is_err() {
        value["__invalid_json"] = json!(true);
    }
    Ok(value)
}

pub const VERSION: u64 = 1;
pub const FRAME_BYTES: usize = 8 << 20;
pub const CONNECTION_REQUESTS: usize = 1024;
const HANDLES: usize = 64;

/// One connection owns one bounded workspace. Disconnect drops every lease.
/// No argv, descriptors, process settings, runtime providers or disk writes.
struct Job {
    lease: CandidateLease,
    selected: bool,
    queued: std::time::Instant,
}
#[derive(Default)]
struct Queue {
    jobs: std::collections::VecDeque<Job>,
    active: bool,
    stopping: bool,
    queue_nanos: u64,
    compute_nanos: u64,
    service_jobs: u64,
    selected_admissions: u64,
}
type SharedQueue = Arc<(Mutex<Queue>, Condvar)>;
pub struct Connection {
    version: Option<u64>,
    workspace: CandidateWorkspace,
    root: PathBuf,
    bases: BTreeMap<u64, BaseLease>,
    candidates: BTreeMap<u64, CandidateLease>,
    next: u64,
    requests: usize,
    queue: SharedQueue,
    worker: Option<std::thread::JoinHandle<()>>,
}
impl Connection {
    pub fn new(workspace: CandidateWorkspace, root: PathBuf) -> std::io::Result<Self> {
        let queue = Arc::new((Mutex::new(Queue::default()), Condvar::new()));
        let worker_queue = queue.clone();
        let compiler = workspace.clone();
        let worker = std::thread::Builder::new()
            .name("candidate-serial".into())
            .stack_size(hirsel_intern::jobs::COMPILER_STACK_BYTES)
            .spawn(move || {
                let (lock, wake) = &*worker_queue;
                loop {
                    let mut q = lock.lock().unwrap();
                    while q.jobs.is_empty() && !q.stopping {
                        q = wake.wait(q).unwrap();
                    }
                    if q.stopping {
                        return;
                    }
                    // Selected affects only admission; S1 order is unchanged.
                    let index = q.jobs.iter().position(|job| job.selected).unwrap_or(0);
                    let job = q.jobs.remove(index).unwrap();
                    q.active = true;
                    q.queue_nanos = q.queue_nanos.saturating_add(nanos(job.queued));
                    q.selected_admissions += u64::from(job.selected);
                    let started = std::time::Instant::now();
                    drop(q);
                    let _ = compiler.result(&job.lease);
                    let mut q = lock.lock().unwrap();
                    q.active = false;
                    q.compute_nanos = q.compute_nanos.saturating_add(nanos(started));
                    q.service_jobs += 1;
                    wake.notify_all();
                }
            })?;
        Ok(Self {
            version: None,
            workspace,
            root,
            bases: BTreeMap::new(),
            candidates: BTreeMap::new(),
            next: 1,
            requests: 0,
            queue,
            worker: Some(worker),
        })
    }
    /// The connection's supported version, or v1 before negotiation.
    pub fn version(&self) -> u64 {
        self.version.unwrap_or(VERSION)
    }
    pub fn exhausted(&self) -> bool {
        self.requests >= CONNECTION_REQUESTS
    }
    pub fn handle(&mut self, request: &Value) -> Value {
        if self.exhausted() && matches!(request["op"].as_str(), Some("capture" | "head" | "submit"))
        {
            return json!({"version":self.version.unwrap_or(VERSION),"error":"ResourceLimit"});
        }
        self.requests = self.requests.saturating_add(1);
        let offered = request["version"].as_u64();
        if self.version.is_none() && matches!(offered, Some(1 | 2)) {
            self.version = offered;
        }
        let version = self.version.unwrap_or(VERSION);
        if offered != Some(version) || !matches!(version, 1 | 2) {
            return json!({"version":version,"error":"UnsupportedVersion"});
        }
        let result = if version == 2 {
            self.dispatch_v2(request)
        } else {
            self.dispatch(request)
        };
        match result {
            Ok(value) => json!({"version":version,"ok":value}),
            Err(error) => json!({"version":version,"error":error}),
        }
    }

    fn allocate(&mut self) -> Result<u64, String> {
        if self.bases.len() + self.candidates.len() >= HANDLES {
            return Err("ResourceLimit".into());
        }
        let id = self.next;
        self.next = self.next.checked_add(1).ok_or("ResourceLimit")?;
        Ok(id)
    }
    fn dispatch(&mut self, r: &Value) -> Result<Value, String> {
        match string(r, "op")? {
            "capture" => {
                let queue = self.queue.0.lock().unwrap();
                if queue.active || !queue.jobs.is_empty() {
                    return Err("Busy".into());
                }
                drop(queue);
                let id = self.allocate()?;
                let entry = self.root.join(string(r, "entry")?);
                // Root selection is restricted to this server's workspace;
                // declared dependencies are still resolved by the frozen loader.
                let canonical = entry
                    .canonicalize()
                    .map_err(|e| format!("InputUnavailable: {e}"))?;
                if !canonical.starts_with(&self.root) {
                    return Err("OutsideWorkspace".into());
                }
                let base = self
                    .workspace
                    .capture(&canonical)
                    .map_err(|e| format!("{e:?}"))?;
                let reply = base_json(id, &base);
                self.bases.insert(id, base);
                Ok(reply)
            }
            "head" => {
                let id = self.allocate()?;
                let base = self.workspace.head_lease().ok_or("UnknownBase")?;
                let reply = base_json(id, &base);
                self.bases.insert(id, base);
                Ok(reply)
            }
            "submit" => {
                let id = self.allocate()?;
                let base = self.bases.get(&number(r, "base")?).ok_or("UnknownBase")?;
                let mut files = vec![];
                let edits = r["edits"].as_array().ok_or("InvalidRequest: edits")?;
                if edits.len() > self.workspace.inner.limits.edit_files {
                    return Err("ResourceLimit".into());
                }
                for file in edits {
                    let mut changes = vec![];
                    let byte_edits = file["edits"].as_array().ok_or("InvalidRequest: edits")?;
                    if byte_edits.len() > self.workspace.inner.limits.byte_edits {
                        return Err("ResourceLimit".into());
                    }
                    for edit in byte_edits {
                        changes.push(ByteEdit {
                            start: number(edit, "start")?
                                .try_into()
                                .map_err(|_| "InvalidRequest: start")?,
                            end: number(edit, "end")?
                                .try_into()
                                .map_err(|_| "InvalidRequest: end")?,
                            replacement: string(edit, "replacement")?.into(),
                        });
                    }
                    files.push(FileEdit {
                        file: SourceKey(string(file, "file")?.into()),
                        expected: parse_hash(string(file, "expected")?)?,
                        edits: changes,
                    });
                }
                let lease = self
                    .workspace
                    .submit(
                        base,
                        CandidateRequest {
                            id: CandidateId {
                                client: 1,
                                nonce: number(r, "id")?,
                            },
                            base: base.token().clone(),
                            edits: files,
                            priority: if r["priority"].as_str() == Some("selected") {
                                Priority::Selected
                            } else {
                                Priority::Background
                            },
                        },
                    )
                    .map_err(|e| format!("{e:?}"))?;
                let mut q = self.queue.0.lock().unwrap();
                if q.jobs.len() >= HANDLES {
                    return Err("ResourceLimit".into());
                }
                q.jobs.push_back(Job {
                    lease: lease.clone(),
                    selected: r["priority"].as_str() == Some("selected"),
                    queued: std::time::Instant::now(),
                });
                self.candidates.insert(id, lease);
                self.queue.1.notify_one();
                Ok(json!({"lease":id}))
            }
            "result" => {
                let lease = self
                    .candidates
                    .get(&number(r, "lease")?)
                    .ok_or("UnknownLease")?;
                match self.workspace.poll(lease).map_err(|e| format!("{e:?}"))? {
                    CandidateResult::Pending => Ok(json!({"pending":true})),
                    CandidateResult::Ready(receipt) => Ok(receipt_json(&receipt)),
                }
            }
            "cancel" => {
                let lease = self
                    .candidates
                    .get(&number(r, "lease")?)
                    .ok_or("UnknownLease")?;
                Ok(
                    json!({"cancel":format!("{:?}",self.workspace.cancel(lease).map_err(|e| format!("{e:?}"))?)}),
                )
            }
            "promote" => {
                let lease = self
                    .candidates
                    .get(&number(r, "lease")?)
                    .ok_or("UnknownLease")?;
                let base = self.bases.get(&number(r, "base")?).ok_or("UnknownBase")?;
                let result = self
                    .workspace
                    .promote(
                        CandidateId {
                            client: 1,
                            nonce: number(r, "id")?,
                        },
                        lease,
                        base.token(),
                    )
                    .map_err(|e| format!("{e:?}"))?;
                Ok(match result {
                    Promotion::Promoted(token) => {
                        json!({"promotion":"Promoted","head":token_json(&token)})
                    }
                    Promotion::Conflict { actual } => {
                        json!({"promotion":"Conflict","actual":token_json(&actual)})
                    }
                    other => json!({"promotion":format!("{other:?}")}),
                })
            }
            "observe_disk" => {
                let lease = self
                    .candidates
                    .get(&number(r, "lease")?)
                    .ok_or("UnknownLease")?;
                Ok(
                    json!({"disk":format!("{:?}",self.workspace.observe_disk(lease).map_err(|e|format!("{e:?}"))?)}),
                )
            }
            "release" => {
                let id = number(r, "lease")?;
                // A base and a candidate never have the same handle.
                if let Some(lease) = self.candidates.remove(&id) {
                    if !self
                        .candidates
                        .values()
                        .any(|other| Arc::ptr_eq(&other.waiter, &lease.waiter))
                    {
                        let _ = self.workspace.cancel(&lease);
                    }
                } else if self.bases.remove(&id).is_none() {
                    return Err("UnknownLease".into());
                }
                Ok(json!({"released":id}))
            }
            "counters" => {
                let c = self.workspace.counters();
                let q = self.queue.0.lock().unwrap();
                Ok(
                    json!({"policy":"serial-frozen","forks":0,"active_candidates":self.workspace.inner.active.load(Ordering::Acquire),
                    "frozen_loads":c.frozen_loads,"store_adoptions":c.store_adoptions,"recomputations":c.recomputations,
                    "completed_results":c.completed_results,"dropped_engines":c.dropped_engines,"retained_sources":c.retained_sources,
                    "retained_bytes":c.retained_bytes,"peak_type_bytes":c.peak_type_bytes,"capture_nanos":c.capture_nanos,
                    "load_nanos":c.load_nanos,"check_nanos":c.check_nanos,"receipt_nanos":c.receipt_nanos,
                    "handles":self.bases.len()+self.candidates.len(),"requests":self.requests,
                    "queue_nanos":q.queue_nanos,"compute_nanos":q.compute_nanos,"service_jobs":q.service_jobs,"selected_admissions":q.selected_admissions,"queued":q.jobs.len(),
                    "transport_frame_allowance":FRAME_BYTES*2,"worker_stack_reservation":hirsel_intern::jobs::COMPILER_STACK_BYTES}),
                )
            }
            _ => Err("UnsupportedOperation".into()),
        }
    }
}
impl Drop for Connection {
    fn drop(&mut self) {
        // Cancellation is per waiter, and does not rewrite completed history.
        for lease in self.candidates.values() {
            let _ = self.workspace.cancel(lease);
        }
        let mut q = self.queue.0.lock().unwrap();
        q.stopping = true;
        q.jobs.clear();
        self.queue.1.notify_all();
        drop(q);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
#[cfg(test)]
impl Connection {
    fn wait_for_idle(&self) {
        let mut q = self.queue.0.lock().unwrap();
        while q.active || !q.jobs.is_empty() {
            q = self.queue.1.wait(q).unwrap();
        }
    }
}
pub fn failure(error: &str) -> Value {
    json!({"version":VERSION,"error":error})
}
pub(super) fn string<'a>(v: &'a Value, field: &str) -> Result<&'a str, String> {
    v[field]
        .as_str()
        .ok_or_else(|| format!("InvalidRequest: {field}"))
}
pub(super) fn number(v: &Value, field: &str) -> Result<u64, String> {
    v[field]
        .as_u64()
        .ok_or_else(|| format!("InvalidRequest: {field}"))
}
pub(super) fn hex(hash: ContentHash) -> String {
    hash.0.iter().map(|b| format!("{b:02x}")).collect()
}
pub(super) fn parse_hash(s: &str) -> Result<ContentHash, String> {
    if s.len() != 64 || !s.is_ascii() {
        return Err("InvalidRequest: digest".into());
    }
    let mut hash = [0; 32];
    for (i, byte) in hash.iter_mut().enumerate() {
        *byte =
            u8::from_str_radix(&s[2 * i..2 * i + 2], 16).map_err(|_| "InvalidRequest: digest")?;
    }
    Ok(ContentHash(hash))
}
fn token_json(t: &BaseToken) -> Value {
    json!({"workspace":t.workspace,"generation":t.generation,"inputs":hex(t.inputs)})
}
fn base_json(id: u64, b: &BaseLease) -> Value {
    json!({"base":id,"token":token_json(b.token()),"sources":b.sources().map(|(key,text,digest)|json!({"file":key.0,"expected":hex(digest),"bytes":text.len()})).collect::<Vec<_>>()})
}
fn identity_json(identity: &CheckedIdentity) -> Value {
    json!({"inputs":hex(identity.inputs),"compiler_environment":hex(identity.compiler_environment),
        "packages":identity.scope.packages,"policy":{"verify":"On","lowering":"Comptime"},
        "checked_products":hex(identity.products)})
}
/// Version-1 additive checked-read difference wire form.
pub fn dependency_json(delta: &DependencyDelta) -> Value {
    match delta {
        // Preserve the existing v1 Unknown string; Known is an additive object.
        DependencyDelta::Unknown { .. } => Value::String(format!("{delta:?}")),
        DependencyDelta::Known {
            parent,
            child,
            reads,
        } => json!({
            "kind":"Known","format":"hirsel.checked-read-delta.v1",
            "parent":identity_json(parent),"child":identity_json(child),
            "reads":reads.iter().map(|read| json!({
                "owner":read.owner.iter().map(|b|format!("{b:02x}")).collect::<String>(),
                "key":read.key.iter().map(|b|format!("{b:02x}")).collect::<String>(),
                "before":read.before.map(hex),"after":read.after.map(hex),
            })).collect::<Vec<_>>()
        }),
    }
}
fn receipt_json(r: &CheckReceipt) -> Value {
    let diagnostics: Vec<_> = r
        .output
        .diagnostics
        .iter()
        .map(|d| d.to_json(&r.output.sources))
        .collect();
    json!({"id":r.id.nonce,"base":token_json(&r.base),"candidate_inputs":hex(r.candidate_inputs),
        "compiler_environment":hex(r.compiler_environment),"packages":r.scope.packages,
        "policy":{"verify":"On","lowering":"Comptime"},"completion":format!("{:?}",r.completion),
        "verdict":format!("{:?}",r.verdict),"incomplete":r.incomplete.as_ref().map(|p|json!({"packages":p.uncompleted_packages,"phase":p.phase.map(|v|format!("{v:?}"))})),
        "diagnostics":diagnostics,"diagnostics_human":r.output.diagnostics.iter().map(|d|d.render_human(&r.output.sources)).collect::<Vec<_>>(),"loader_error":r.loader_error,"failure_class":r.failure_class.map(|v|format!("{v:?}")),
        "checked_products":r.checked_products.as_ref().map(|p|hex(p.digest)),"dependencies":dependency_json(&r.dependencies),
        "head":match &r.head { HeadFreshness::CurrentAt(g)=>json!({"current_at":g}), HeadFreshness::Stale(t)=>json!({"stale":token_json(t)}) },
        "disk":format!("{:?}",r.disk)})
}

#[cfg(test)]
mod tests {
    use super::*;
    static NEXT: AtomicU64 = AtomicU64::new(0);
    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!(
                "candidate-protocol-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir_all(&root).unwrap();
            std::fs::write(root.join("main.hir"), "pub fn main()->u64{1}\n").unwrap();
            std::fs::write(root.join("hirsel.json"),r#"{"schema":1,"package":{"name":"app","version":"1"},"sources":[{"path":"main.hir","module":[]}],"dependencies":[],"entry":{"module":[],"name":"main"}}"#).unwrap();
            Self(root)
        }
        fn connection(&self) -> Connection {
            Connection::new(
                CandidateWorkspace::new(
                    CompileConfig::default(),
                    Default::default(),
                    Limits::default(),
                )
                .unwrap(),
                self.0.clone(),
            )
            .unwrap()
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    fn call(c: &mut Connection, mut r: Value) -> Value {
        if r["op"].as_str() == Some("result") {
            c.wait_for_idle();
        }
        r["version"] = VERSION.into();
        let reply = c.handle(&r);
        assert!(reply.get("error").is_none(), "{reply}");
        reply["ok"].clone()
    }
    #[test]
    fn candidate_protocol_duplicate_waiters_promote_and_keep_old_receipt() {
        let f = Fixture::new();
        let mut c = f.connection();
        let entered = Arc::new(std::sync::Barrier::new(2));
        let resume = Arc::new(std::sync::Barrier::new(2));
        let (start, finish) = (entered.clone(), resume.clone());
        *c.workspace.inner.before_compute.lock().unwrap() = Some(Arc::new(move || {
            start.wait();
            finish.wait();
        }));
        let base = call(&mut c, json!({"op":"capture","entry":"hirsel.json"}));
        let source = &base["sources"][0];
        let edit = json!({"file":source["file"],"expected":source["expected"],"edits":[{"start":0,"end":source["bytes"],"replacement":"pub fn main()->u64{2}\n"}]});
        let a = call(
            &mut c,
            json!({"op":"submit","id":1,"base":base["base"],"edits":[edit.clone()]}),
        )["lease"]
            .clone();
        entered.wait();
        let b = call(
            &mut c,
            json!({"op":"submit","id":2,"base":base["base"],"edits":[edit]}),
        )["lease"]
            .clone();
        call(&mut c, json!({"op":"cancel","lease":a}));
        assert_eq!(
            c.handle(&json!({"version":1,"op":"result","lease":b}))["ok"]["pending"],
            true
        );
        resume.wait();
        let cancelled = call(&mut c, json!({"op":"result","lease":a}));
        assert_eq!(cancelled["completion"], "Incomplete(Cancelled)");
        assert_eq!(cancelled["checked_products"], Value::Null);
        let receipt = call(&mut c, json!({"op":"result","lease":b}));
        assert_eq!(receipt["completion"], "Complete");
        assert_eq!(receipt["verdict"], "Pass");
        assert_eq!(receipt["id"], 2);
        assert_eq!(receipt["base"], base["token"]);
        let counters = call(&mut c, json!({"op":"counters"}));
        assert_eq!(counters["frozen_loads"], 1);
        assert_eq!(counters["completed_results"], 1);
        assert_eq!(counters["forks"], 0);
        let promote = call(
            &mut c,
            json!({"op":"promote","id":1,"base":base["base"],"lease":b}),
        );
        assert_eq!(promote["promotion"], "Promoted");
        assert_eq!(
            call(
                &mut c,
                json!({"op":"promote","id":1,"base":base["base"],"lease":b})
            ),
            promote
        );
        let again = call(&mut c, json!({"op":"result","lease":b}));
        assert_eq!(receipt["checked_products"], again["checked_products"]);
        assert!(again["head"].get("stale").is_some());
        assert_eq!(
            std::fs::read_to_string(f.0.join("main.hir")).unwrap(),
            "pub fn main()->u64{1}\n"
        );
        call(&mut c, json!({"op":"release","lease":b}));
        assert_eq!(
            c.handle(&json!({"version":1,"op":"result","lease":b}))["error"],
            "UnknownLease"
        );
    }
    #[test]
    fn candidate_protocol_rejects_malformed_and_caps_handles_without_eviction() {
        let f = Fixture::new();
        let mut c = f.connection();
        assert_eq!(
            c.handle(&json!({"version":3,"op":"capture","entry":"hirsel.json"}))["error"],
            "UnsupportedVersion"
        );
        let base = call(&mut c, json!({"op":"capture","entry":"hirsel.json"}));
        let mut malformed = json!({"version":1,"op":"submit","id":1,"base":base["base"],"edits":[{"file":base["sources"][0]["file"],"expected":"zz","edits":[]}]});
        assert_eq!(c.handle(&malformed)["error"], "InvalidRequest: digest");
        malformed["edits"][0]["expected"] = base["sources"][0]["expected"].clone();
        malformed["edits"][0]["edits"] = json!([{"start":u64::MAX,"end":0,"replacement":""}]);
        assert_eq!(c.handle(&malformed)["error"], "InvalidRequest: start");
        for _ in 1..HANDLES {
            call(&mut c, json!({"op":"head"}));
        }
        assert_eq!(
            c.handle(&json!({"version":1,"op":"head"}))["error"],
            "ResourceLimit"
        );
        assert!(c.bases.contains_key(&base["base"].as_u64().unwrap()));
    }
    #[test]
    fn candidate_protocol_exhaustion_preserves_cleanup_and_completed_results() {
        let f = Fixture::new();
        let mut c = f.connection();
        let base = call(&mut c, json!({"op":"capture","entry":"hirsel.json"}));
        let lease = call(
            &mut c,
            json!({"op":"submit","id":1,"base":base["base"],"edits":[]}),
        )["lease"]
            .clone();
        let receipt = call(&mut c, json!({"op":"result","lease":lease}));
        c.requests = CONNECTION_REQUESTS;
        assert_eq!(
            c.handle(&json!({"version":1,"op":"submit","id":2,"base":base["base"],"edits":[]}))["error"],
            "ResourceLimit"
        );
        assert_eq!(
            call(&mut c, json!({"op":"result","lease":lease}))["checked_products"],
            receipt["checked_products"]
        );
        assert_eq!(
            call(&mut c, json!({"op":"cancel","lease":lease}))["cancel"],
            "AlreadyComplete"
        );
        call(&mut c, json!({"op":"release","lease":lease}));
        call(&mut c, json!({"op":"release","lease":base["base"]}));
        assert_eq!(call(&mut c, json!({"op":"counters"}))["handles"], 0);
    }
    #[test]
    fn candidate_protocol_disconnect_cancels_active_work_and_drains_worker() {
        let f = Fixture::new();
        let mut c = f.connection();
        let entered = Arc::new(std::sync::Barrier::new(2));
        let resume = Arc::new(std::sync::Barrier::new(2));
        let (start, finish) = (entered.clone(), resume.clone());
        *c.workspace.inner.before_compute.lock().unwrap() = Some(Arc::new(move || {
            start.wait();
            finish.wait();
        }));
        let base = call(&mut c, json!({"op":"capture","entry":"hirsel.json"}));
        let lease = call(
            &mut c,
            json!({"op":"submit","id":1,"base":base["base"],"edits":[]}),
        )["lease"]
            .as_u64()
            .unwrap();
        entered.wait();
        let waiter = c.candidates[&lease].clone();
        let workspace = c.workspace.clone();
        let queue = c.queue.clone();
        let dropper = std::thread::spawn(move || drop(c));
        let mut q = queue.0.lock().unwrap();
        while !q.stopping {
            q = queue.1.wait(q).unwrap();
        }
        drop(q);
        let CandidateResult::Ready(receipt) = workspace.poll(&waiter).unwrap() else {
            panic!("disconnect left a live waiter")
        };
        assert_eq!(
            receipt.completion,
            Completion::Incomplete(IncompleteReason::Cancelled)
        );
        assert!(receipt.checked_products.is_none());
        resume.wait();
        dropper.join().unwrap();
        assert_eq!(workspace.counters().completed_results, 0);
    }
}

#[cfg(test)]
mod matrix {
    use super::*;
    #[test]
    fn candidate_protocol_cold_jobs_salts() {
        if let Ok(job) = std::env::var("HIRSEL_PROTOCOL_CELL_JOBS") {
            hirsel_intern::salt_ids(
                std::env::var("HIRSEL_PROTOCOL_CELL_SALT")
                    .unwrap()
                    .parse()
                    .unwrap(),
            );
            cell(job.parse().unwrap());
            return;
        }
        for jobs in [1, 2, 8] {
            for salt in [0, 1, 7] {
                let result = std::process::Command::new(std::env::current_exe().unwrap())
                    .args([
                        "candidate::protocol::matrix::candidate_protocol_cold_jobs_salts",
                        "--exact",
                        "--nocapture",
                    ])
                    .env("HIRSEL_PROTOCOL_CELL_JOBS", jobs.to_string())
                    .env("HIRSEL_PROTOCOL_CELL_SALT", salt.to_string())
                    .output()
                    .unwrap();
                assert!(
                    result.status.success(),
                    "jobs{jobs}/salt{salt}: {}{}",
                    String::from_utf8_lossy(&result.stdout),
                    String::from_utf8_lossy(&result.stderr)
                );
            }
        }
    }
    fn cell(jobs: usize) {
        let root_scratch = tempfile::Builder::new()
            .prefix("candidate-protocol-matrix-")
            .tempdir()
            .unwrap();
        let root = root_scratch.path().to_path_buf();
        let manifest = r#"{"schema":1,"package":{"name":"app","version":"1"},"sources":[{"path":"main.hir","module":[]}],"dependencies":[],"entry":{"module":[],"name":"main"}}"#;
        let before = "pub fn main()->u64{0}\n";
        std::fs::write(root.join("hirsel.json"), manifest).unwrap();
        std::fs::write(root.join("main.hir"), before).unwrap();
        let config = CompileConfig {
            jobs: hirsel_intern::jobs::Jobs::new(jobs),
            ..CompileConfig::default()
        };
        let workspace =
            CandidateWorkspace::new(config.clone(), Default::default(), Limits::default()).unwrap();
        let mut c = Connection::new(workspace, root.clone()).unwrap();
        let base =
            c.handle(&json!({"version":1,"op":"capture","entry":"hirsel.json"}))["ok"].clone();
        let texts = [
            "pub fn main()->u64{1}\n",
            "pub fn main()->u64{1}\n",
            "pub opaque type Secret: data uses {} { x:u64, }\nfn get(s:Secret)->u64{s.x}\npub fn main()->u64{1}\n",
            "pub opaque type Secret: data uses {} { x:bool, }\nfn get(s:Secret)->u64{s.x}\npub fn main()->u64{1}\n",
            "fn missing()->u64{2}\npub fn main()->u64{missing()}\n",
            "fn source()->String{\"fn(x:u64)->u64 uses {} {x+3}\"}\nfn body()->Result[Body[fn(u64)->u64 uses {}],String]{Ok(codegen.body(source()))}\nfn generated(x:u64)->u64 uses {} = comptime body()\npub fn main()->u64{generated(1)}\n",
            "\n\nfn bad(x:u64)->u64{x}\npub fn main()->u64{bad(true)}\n",
            "pub fn main()->u64{8}\n",
        ];
        let mut leases = vec![];
        for index in (0..8).rev() {
            let reply=c.handle(&json!({"version":1,"op":"submit","base":base["base"],"id":index,"priority":if index==0 {"selected"} else {"background"},"edits":[{"file":base["sources"][0]["file"],"expected":base["sources"][0]["expected"],"edits":[{"start":0,"end":before.len(),"replacement":texts[index]}]}]}));
            assert!(reply.get("error").is_none(), "{reply}");
            leases.push((index, reply["ok"]["lease"].clone()));
        }
        c.wait_for_idle();
        for (index, lease) in leases {
            let actual = c.handle(&json!({"version":1,"op":"result","lease":lease}))["ok"].clone();
            let cold = root.join(format!("cold-{index}"));
            std::fs::create_dir_all(&cold).unwrap();
            std::fs::write(cold.join("hirsel.json"), manifest).unwrap();
            std::fs::write(cold.join("main.hir"), texts[index]).unwrap();
            let expected = hirsel_engine::with_compiler_stack(|| {
                let mut p = crate::COMPILER
                    .clone()
                    .with_config(config.clone())
                    .with_owned_engines()
                    .load_program_source(&cold.join("hirsel.json"), None)
                    .unwrap();
                let output = p.check(LoweringPolicy::Comptime).unwrap();
                assert_eq!(output.completion, CheckCompletion::Complete);
                let digest = p.engine.checked_products(&p.entry).unwrap().digest;
                let human: Vec<_> = output
                    .diagnostics
                    .iter()
                    .map(|d| {
                        d.render_human(&output.sources)
                            .replace(cold.to_str().unwrap(), root.to_str().unwrap())
                    })
                    .collect();
                let diagnostics: Vec<_> = output
                    .diagnostics
                    .iter()
                    .map(|d| {
                        serde_json::from_str::<Value>(
                            &d.to_json(&output.sources)
                                .to_string()
                                .replace(cold.to_str().unwrap(), root.to_str().unwrap()),
                        )
                        .unwrap()
                    })
                    .collect();
                (hex(digest), human, diagnostics)
            });
            assert_eq!(actual["completion"], "Complete", "{actual}");
            assert_eq!(actual["checked_products"], expected.0, "index{index}");
            assert_eq!(
                actual["diagnostics_human"],
                json!(expected.1),
                "index{index}"
            );
            assert_eq!(actual["diagnostics"], json!(expected.2), "index{index}");
        }
        assert_eq!(
            std::fs::read_to_string(root.join("main.hir")).unwrap(),
            before
        );
        assert_eq!(
            c.workspace.head().unwrap().generation,
            base["token"]["generation"].as_u64().unwrap()
        );
        assert_eq!(c.workspace.counters().frozen_loads, 7);
        assert_eq!(c.workspace.counters().completed_results, 7);
        drop(c);
        std::fs::remove_dir_all(root).unwrap();
    }
}
