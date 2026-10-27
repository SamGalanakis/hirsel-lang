//! Versioned diagnostic canonical events. The input is the semantic projection,
//! never native boundaries or outcomes. serde_json owns JSON escaping/numbers;
//! tape_wire owns closed terminal payloads and initialized host/value bytes.
use super::*;
use serde_json::{Value, json};

impl Log {
    /// Build and validate the entire projection before a caller opens a trace
    /// destination. Missing evidence cannot produce a successful partial file.
    pub(in crate::tasks::ordered) fn jsonl(&self) -> Result<Vec<u8>, &'static str> {
        let events = self
            .canonical()
            .map_err(|_| "incomplete canonical event evidence")?;
        let mut out = Vec::new();
        for event in events {
            serde_json::to_writer(&mut out, &row(event)).expect("JSON values serialize into Vec");
            out.push(b'\n');
        }
        Ok(out)
    }
}

/// A complete owned trace, detached from scheduler rows before destruction.
/// Its bytes can only be constructed after validation; destination creation
/// and writes happen after that validation and outside the scheduler role.
#[derive(Debug)]
pub(crate) struct Trace(Vec<u8>);

impl Trace {
    pub(crate) fn free(projection: &hirsel_free::projection::Projection) -> Result<Self, String> {
        crate::trace_events::free::canonical_events(projection).map(Self)
    }
    /// # Safety
    /// Admission's home thread, after run_root returned, before abandonment
    /// or destruction. The run remains live, including when workers linger.
    pub(crate) unsafe fn capture(
        run: *mut crate::context::RunShared,
        pending: u32,
    ) -> Result<Self, &'static str> {
        if pending != 0 {
            return Err("canonical trace has running workers");
        }
        let _role = unsafe { crate::tasks::role::IdleGuard::new(run) };
        let scheduler = unsafe { &mut *(*run).tasks.get() }
            .as_deref_mut()
            .ok_or("no ordered scheduler")?;
        if let Some(stop) = unsafe { (*run).control.stopped() } {
            let snapshots =
                std::mem::take(&mut *unsafe { (*run).ordered_home_results.lock().unwrap() });
            super::host_terminal::home_results(scheduler, snapshots)?;
            super::host_terminal::external(scheduler, &stop)?;
            super::terminal_waits::close(scheduler, &stop)?;
        }
        let Logical::Ordered(driver) = &scheduler.logical else {
            return Err("no ordered scheduler");
        };
        let log = driver
            .events
            .as_ref()
            .ok_or("canonical evidence was not enabled")?;
        Self::completed(log)
    }

    fn completed(log: &Log) -> Result<Self, &'static str> {
        let events = log
            .canonical()
            .map_err(|_| "incomplete canonical event evidence")?;
        if !events.last().is_some_and(|event| {
            event.item == CanonicalItem::Terminal
                || matches!(event.outcome, CanonicalOutcome::RootEnd { .. })
        }) {
            return Err("canonical trace has no terminal boundary");
        }
        log.jsonl().map(Self)
    }

    /// Use the launcher's existing physical grant/alias checks. The pinned
    /// destination creates/truncates nothing until this validated trace exists.
    pub(crate) fn publish(
        &self,
        destination: crate::host_paths::RecorderDestination,
        grants: &[(&str, &crate::host_paths::PinnedDirectory)],
        files: &[(&str, &std::fs::File)],
    ) -> Result<(), String> {
        self.write(|| {
            destination
                .open_with_files(grants, files)
                .map_err(|error| error.to_string())
        })
    }

    fn write<W: std::io::Write>(
        &self,
        open: impl FnOnce() -> Result<W, String>,
    ) -> Result<(), String> {
        let mut writer = open()?;
        writer
            .write_all(&self.0)
            .and_then(|()| writer.flush())
            .map_err(|error| format!("canonical trace: {error}"))
    }

    #[cfg(test)]
    pub(crate) fn bytes(&self) -> &[u8] {
        &self.0
    }
}

fn object(id: &ObjectId) -> Value {
    json!({"creator": id.creator, "ordinal": id.ordinal})
}
fn objects(ids: &[ObjectId]) -> Vec<Value> {
    ids.iter().map(object).collect()
}
fn handle(id: &HostHandleId) -> Value {
    json!({"operation": object(&id.operation), "field": id.field})
}
fn authority(authority: &HostAuthority) -> Value {
    match authority {
        HostAuthority::RootIo => json!({"kind":"root_io"}),
        HostAuthority::RootEnv => json!({"kind":"root_env"}),
        HostAuthority::RootArgs => json!({"kind":"root_args"}),
        HostAuthority::Created(id) => json!({"kind":"created", "handle":handle(id)}),
    }
}
fn resource(key: &host_keys::Resource) -> Value {
    use host_keys::Resource as R;
    match key {
        R::Namespace => json!({"kind":"namespace"}),
        R::Coupled => json!({"kind":"coupled"}),
        R::Root => json!({"kind":"root"}),
        R::Place => json!({"kind":"place"}),
        R::Created(id) => json!({"kind":"created", "handle":handle(id)}),
        R::Domain { owner, label } => {
            json!({"kind":"domain", "owner":resource(owner), "label":label})
        }
    }
}
fn creation(row: &host_keys::Creation) -> Value {
    use host_keys::ResourceKind as K;
    let kind = match &row.kind {
        K::File => json!({"kind":"file"}),
        K::Sink { file_backed } => json!({"kind":"sink", "file_backed":file_backed}),
        K::Other => json!({"kind":"other"}),
        K::Half { stream } => json!({"kind":"half", "stream":resource(stream)}),
    };
    json!({"handle":handle(&row.handle), "resource":resource(&row.resource), "resource_kind":kind})
}
fn host(value: &host_values::HostValue) -> Value {
    json!({"payload":hirsel_run_schema::hex(&value.payload), "handles":value.handles.iter().map(handle).collect::<Vec<_>>()})
}
fn bytes(value: &[u8]) -> Value {
    json!(hirsel_run_schema::hex(value))
}
fn optional_bytes(value: &Option<Vec<u8>>) -> Value {
    value.as_ref().map_or(Value::Null, |value| bytes(value))
}
fn failures(chain: &[(hirsel_run_schema::Failure, Vec<u32>)]) -> Vec<Value> {
    chain.iter().map(|(failure, path)| json!({"rule":failure.rule, "site":failure.site, "operation":failure.operation, "task":path})).collect()
}
fn item(item: &CanonicalItem) -> Value {
    match item {
        CanonicalItem::Root => json!({"kind":"root"}),
        CanonicalItem::Terminal => json!({"kind":"terminal"}),
        CanonicalItem::SharedRoot(root) => {
            json!({"kind":"shared_root", "creator":root.creator, "freeze":root.freeze})
        }
        CanonicalItem::Object(id) => json!({"kind":"object", "id":object(id)}),
        CanonicalItem::Cancellation(path) => json!({"kind":"cancellation", "task":path}),
        CanonicalItem::CompletionRank(id) => json!({"kind":"completion_rank", "id":object(id)}),
        CanonicalItem::ScopeCancellation(id) => {
            json!({"kind":"scope_cancellation", "id":object(id)})
        }
        CanonicalItem::HostKey(id) => json!({"kind":"host_key", "id":object(id)}),
        CanonicalItem::HostOperation(id) => json!({"kind":"host_operation", "id":object(id)}),
        CanonicalItem::HostResource(key) => json!({"kind":"host_resource", "key":resource(key)}),
        CanonicalItem::HostHandle(id) => json!({"kind":"host_handle", "id":handle(id)}),
    }
}
fn transition(transition: &Transition) -> Value {
    let kind = match transition {
        Transition::Keep => "keep",
        Transition::Yield => "yield",
        Transition::Park => "park",
        Transition::Hold => "hold",
        Transition::End => "end",
        Transition::Stop => "stop",
        Transition::Call {
            members,
            then_yield,
        } => return json!({"kind":"call", "members":members, "then_yield":then_yield}),
    };
    json!({"kind":kind})
}
fn outcome(outcome: &CanonicalOutcome) -> Value {
    use CanonicalOutcome as O;
    match outcome {
        O::HostRequest(request) => {
            json!({"kind":"host_request", "payload":bytes(&request.payload), "authorities":request.authorities.iter().map(authority).collect::<Vec<_>>()})
        }
        O::HostAccess { mode, result } => {
            json!({"kind":"host_access", "mode":match mode {crate::io_keys::Mode::Shared=>"shared", crate::io_keys::Mode::Exclusive=>"exclusive"}, "result":host(result)})
        }
        O::HostResourceCreated => json!({"kind":"host_resource_created"}),
        O::HostCreated(row) => json!({"kind":"host_created", "creation":creation(row)}),
        O::HostAlias(row) => json!({"kind":"host_alias", "creation":creation(row)}),
        O::HostResult(result) => json!({"kind":"host_result", "result":host(result)}),
        O::HostOutput(value) => json!({"kind":"host_output", "bytes":bytes(value)}),
        O::HostWaiting => json!({"kind":"host_waiting"}),
        O::HostAbandoned { completed } => {
            json!({"kind":"host_abandoned", "phase":if *completed {"result"} else {"blocked"}})
        }
        O::HostFinished => json!({"kind":"host_finished"}),
        O::Created(id) => json!({"kind":"created", "id":object(id)}),
        O::Alias(id) => json!({"kind":"alias", "id":object(id)}),
        O::Scalar(value) => json!({"kind":"scalar", "value":bytes(value)}),
        O::CpuControl {
            width,
            action,
            count,
            outcome,
        } => {
            json!({"kind":"cpu_control", "width":width, "action":action, "count":count, "outcome":outcome})
        }
        O::Argument { kind, index, value } => {
            json!({"kind":"argument", "operation":kind, "index":index, "value":optional_bytes(value)})
        }
        O::Value { status, payload } => {
            json!({"kind":"value", "status":status, "payload":bytes(payload)})
        }
        O::Spawned { scope, path } => json!({"kind":"spawned", "scope":object(scope), "path":path}),
        O::ScopeExiting { members } => json!({"kind":"scope_exiting", "members":objects(members)}),
        O::ScopeExited {
            status,
            expired,
            failures: chain,
        } => {
            json!({"kind":"scope_exited", "status":status, "expired":expired, "failures":failures(chain)})
        }
        O::BulkCanceling { members } => {
            json!({"kind":"bulk_canceling", "members":objects(members)})
        }
        O::Awaiting { members } => {
            json!({"kind":"awaiting", "members":members.iter().map(|(index,id)| json!({"index":index,"id":object(id)})).collect::<Vec<_>>()})
        }
        O::AwaitAbandoned { task, ordinal } => {
            json!({"kind":"await_abandoned", "task":task, "ordinal":ordinal})
        }
        O::Selected {
            index,
            member,
            rank,
        } => {
            json!({"kind":"selected", "index":index, "member":member.as_ref().map(object), "rank":rank})
        }
        O::Failures(chain) => json!({"kind":"failures", "failures":failures(chain)}),
        O::RootReleased { last } => json!({"kind":"root_released", "last":last}),
        O::Thaw { last } => json!({"kind":"thaw", "last":last}),
        O::Released { kind, leftovers } => {
            json!({"kind":"released", "resource_kind":kind, "leftovers":leftovers.iter().map(|value| bytes(value)).collect::<Vec<_>>()})
        }
        O::DeadlineArmed { deadline, armed } => {
            json!({"kind":"deadline_armed", "deadline":deadline, "armed":armed})
        }
        O::DeadlineExpired => json!({"kind":"deadline_expired"}),
        O::GateWaiting => json!({"kind":"gate_waiting"}),
        O::GateAcquired => json!({"kind":"gate_acquired"}),
        O::GateReleased => json!({"kind":"gate_released"}),
        O::Receiving => json!({"kind":"receiving"}),
        O::Received { status, payload } => {
            json!({"kind":"received", "status":status, "payload":optional_bytes(payload)})
        }
        O::Closed => json!({"kind":"closed"}),
        O::BulkDetaching { members } => {
            json!({"kind":"bulk_detaching", "members":objects(members)})
        }
        O::DetachSkipped => json!({"kind":"detach_skipped"}),
        O::Detached => json!({"kind":"detached"}),
        O::Joining => json!({"kind":"joining"}),
        O::JoinAbandoned { task, ordinal } => {
            json!({"kind":"join_abandoned", "task":task, "ordinal":ordinal})
        }
        O::Joined {
            status,
            rank,
            payload,
            failures: chain,
        } => {
            json!({"kind":"joined", "status":status, "rank":rank, "payload":optional_bytes(payload), "failures":failures(chain)})
        }
        O::TaskEnd { holding, result } => {
            json!({"kind":"task_end", "holding":holding, "result":optional_bytes(result)})
        }
        O::TaskFailed { failures: chain } => {
            json!({"kind":"task_failed", "failures":failures(chain)})
        }
        O::Stopped(ended) => {
            use crate::tape_wire::Wire;
            let mut payload = Vec::new();
            ended.encode(&mut payload);
            json!({"kind":"stopped", "payload":bytes(&payload), "codec":"hirsel.tape.fixint@1"})
        }
        O::Blocked(waits) => {
            json!({"kind":"blocked", "waits":waits.iter().map(|wait|json!({"task":wait.task, "site":wait.site, "kind":wait.kind, "targets":objects(&wait.targets)})).collect::<Vec<_>>()})
        }
        O::RootEnd {
            status,
            result,
            failures: chain,
        } => {
            json!({"kind":"root_end", "status":status, "result":optional_bytes(result), "failures":failures(chain)})
        }
        O::Unit => json!({"kind":"unit"}),
    }
}
fn row(event: &CanonicalEvent) -> Value {
    json!({
        "schema":"hirsel.ordered.events@2", "task":event.task, "ordinal":event.ordinal,
        "subevent":event.subevent, "op":event.op, "transition":transition(&event.transition),
        "item":item(&event.item), "version":event.version, "outcome":outcome(&event.outcome),
        "retirement":event.retirement, "credit":event.credit,
        "before":event.before.pieces().map(|(credit,piece)|json!({"credit":credit,"fuel":piece.fuel,"net":piece.net,"envelope":piece.envelope})).collect::<Vec<_>>()
    })
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod export_tests;
