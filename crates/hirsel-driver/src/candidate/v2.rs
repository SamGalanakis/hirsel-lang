//! Serial closed-input admission. Namespace structure and vectors are ported
//! from TS-Go 89d5d5b snapshotfs.go:56-117 and snapshotfs_test.go:24-257,
//! 1113-1144,1377-1432 by the G1 adapter. LSP baf2db31 workspaceEdit.md:153-157
//! supplies atomicity; Hirsel applies a simultaneous delta, with no mkdir or
//! host fallback. Lifecycle and exact checked-read differences reuse v1.
use super::*;
pub use hirsel_engine::loader_inputs::namespace::{
    CapturePlan, DiskCapture, Inspection, Key, LeaseBudget, NamespaceLease,
};
use hirsel_engine::loader_inputs::{
    InputKind,
    namespace::{Change, OverlayError},
};
use serde_json::{Value, json};

pub(super) fn json_bytes(value: &Value) -> usize {
    std::mem::size_of::<Value>()
        + match value {
            Value::String(s) => s.capacity(),
            Value::Array(a) => {
                a.capacity() * std::mem::size_of::<Value>()
                    + a.iter().map(json_bytes).sum::<usize>()
            }
            Value::Object(o) => o
                .iter()
                .map(|(k, v)| k.capacity() + 128 + json_bytes(v))
                .sum(),
            _ => 0,
        }
}
pub(super) fn ffi_bytes(packages: &[hirsel_engine::UnsafeFfiPackage]) -> usize {
    packages
        .iter()
        .map(|p| {
            512 + p.package.name.len()
                + p.package.version.len()
                + p.link
                    .iter()
                    .map(|link| {
                        128 + match link {
                            hirsel_engine::LinkInput::Library(name) => name.len(),
                            hirsel_engine::LinkInput::File(path) => path.as_os_str().len(),
                        }
                    })
                    .sum::<usize>()
                + p.wrappers
                    .keys()
                    .map(|path| format!("{path:?}").len() + 256)
                    .sum::<usize>()
        })
        .sum()
}
pub(super) fn environment(w: &Workspace) -> ContentHash {
    ContentHash::of_parts(
        "hirsel.candidate.environment.v2",
        [
            w.environment.0.as_slice(),
            &(w.config.jobs.get() as u64).to_le_bytes(),
            &w.config.hold_bytes.to_le_bytes(),
            format!("{:?}", w.config.retention).as_bytes(),
            b"Verify::On;Comptime;NoneStore",
        ],
    )
}
#[derive(Clone, Debug)]
pub(super) struct Selection {
    pub scope: CheckedScope,
    pub sources: BTreeSet<PathBuf>,
    pub manifests: BTreeSet<PathBuf>,
    pub facts: Value,
}
impl Selection {
    pub(super) fn of(program: &hirsel_engine::LoadedProgram) -> Self {
        Self {
            scope: scope(program),
            sources: program
                .files
                .iter()
                .filter_map(|(f, _)| program.engine.sources.file(*f))
                .map(|s| s.path.clone())
                .collect(),
            manifests: program.inputs.iter().cloned().collect(),
            facts: json!({
                "entry":format!("{:?}", program.entry),
                "sources":program.files.iter().map(|(f,c)| {
                    let s = program.engine.sources.file(*f).unwrap();
                    json!({"logical":s.logical_path(),"package":c.package,"modules":c.modules,
                        "image":program.engine.imaged(&c.package)})
                }).collect::<Vec<_>>(),
                "unsafe_ffi":program.unsafe_ffi.iter().map(|p|json!({
                    "package":p.package,"headers_checked":p.headers_checked,"threads":format!("{:?}",p.threads),
                    "link":p.link.iter().map(|l|match l {
                        hirsel_engine::LinkInput::Library(name)=>json!({"library":name}),
                        hirsel_engine::LinkInput::File(path)=>json!({"file":path}),
                    }).collect::<Vec<_>>(),
                    "wrappers":p.wrappers.iter().map(|(path,wrapper)|json!({"definition":path,"wrapper":wrapper})).collect::<Vec<_>>(),
                })).collect::<Vec<_>>(),
                "image_notes":program.image_notes,
                "images":program.engine.selected_image().map(|(p,k)|json!({"package":p,"key":protocol::hex(k)})),
            }),
        }
    }
    pub(super) fn retained_bytes(&self) -> usize {
        json_bytes(&self.facts)
            + self
                .sources
                .iter()
                .chain(&self.manifests)
                .map(|p| p.as_os_str().len() + 128)
                .sum::<usize>()
            + self
                .scope
                .packages
                .iter()
                .map(|p| p.name.len() + p.version.len() + 128)
                .sum::<usize>()
            + 512
    }
}
pub(super) struct RequestRecord {
    canonical: Value,
    waiter: Weak<Waiter>,
    _charge: Arc<Charge>,
}
pub(super) fn same_inputs(a: &Inputs, b: &Inputs) -> bool {
    match (&a.namespace, &b.namespace) {
        (Some(a), Some(b)) => a.same_namespace(b),
        (None, None) => a.frozen == b.frozen,
        _ => false,
    }
}
fn empty_scope() -> CheckedScope {
    CheckedScope {
        packages: vec![],
        policy: policy(),
    }
}
fn err(e: impl std::fmt::Debug) -> String {
    format!("{e:?}")
}
fn io_error(e: std::io::Error) -> String {
    if e.get_ref().is_some_and(|e| e.is::<InputUnavailable>()) {
        "InputUnavailable".into()
    } else {
        format!("UnsupportedEdit: {e}")
    }
}
fn overlay_error(e: OverlayError) -> String {
    match e {
        OverlayError::Precondition => "PreconditionFailed".into(),
        OverlayError::Unavailable => "InputUnavailable".into(),
        OverlayError::Limit => "ResourceLimit".into(),
        OverlayError::StaleBytes => "StaleBytes".into(),
        OverlayError::DuplicateTarget => "DuplicateTarget".into(),
        other => format!("UnsupportedEdit: {other:?}"),
    }
}
fn token(value: &Value) -> Result<BaseToken, String> {
    fields(value, &["workspace", "generation", "inputs"], &[])?;
    Ok(BaseToken {
        workspace: protocol::number(value, "workspace")?,
        generation: protocol::number(value, "generation")?,
        inputs: hash(value, "inputs")?,
    })
}
pub(super) fn fields(v: &Value, required: &[&str], optional: &[&str]) -> Result<(), String> {
    let obj = v.as_object().ok_or("InvalidRequest: object")?;
    if required.iter().any(|k| !obj.contains_key(*k))
        || obj
            .keys()
            .any(|k| !required.contains(&k.as_str()) && !optional.contains(&k.as_str()))
    {
        return Err("InvalidRequest: fields".into());
    }
    Ok(())
}
fn hash(v: &Value, key: &str) -> Result<ContentHash, String> {
    let text = protocol::string(v, key)?;
    if text.len() != 64
        || !text
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err("InvalidRequest: digest".into());
    }
    protocol::parse_hash(text)
}
fn key(v: &Value) -> Result<Key, String> {
    fields(v, &["root", "path"], &[])?;
    Ok(Key {
        root: protocol::string(v, "root")?.into(),
        path: protocol::string(v, "path")?.into(),
    })
}
#[derive(Clone)]
struct Edit {
    wire: Value,
    key: Key,
    action: String,
    manifest: bool,
    present: Option<ContentHash>,
    bytes: Option<ContentHash>,
    absent: Option<ContentHash>,
    text: Option<String>,
    patches: Vec<ByteEdit>,
}
fn decode_edit(v: &Value) -> Result<Edit, String> {
    let kind = protocol::string(v, "kind")?;
    let manifest = kind == "manifest";
    let action = if manifest {
        protocol::string(v, "action")?
    } else {
        match kind {
            "create_source" => "create",
            "delete_source" => "delete",
            "replace_source" => "replace",
            "edit_source" => "edit",
            _ => return Err("InvalidRequest: kind".into()),
        }
    };
    let mut required = vec!["kind", "path", "expected"];
    if manifest {
        required.push("action");
    }
    match action {
        "create" | "replace" => required.push("text"),
        "edit" if !manifest => required.push("byte_edits"),
        "delete" => {}
        _ => return Err("InvalidRequest: action".into()),
    }
    fields(v, &required, &[])?;
    let expected = &v["expected"];
    fields(
        expected,
        if action == "create" {
            &["absent"]
        } else {
            &["present", "bytes"]
        },
        &[],
    )?;
    let mut edit = Edit {
        wire: v.clone(),
        key: key(&v["path"])?,
        action: action.into(),
        manifest,
        present: None,
        bytes: None,
        absent: None,
        text: None,
        patches: vec![],
    };
    if action == "create" {
        edit.absent = Some(hash(expected, "absent")?);
    } else {
        edit.present = Some(hash(expected, "present")?);
        edit.bytes = Some(hash(expected, "bytes")?);
    }
    if matches!(action, "create" | "replace") {
        edit.text = Some(protocol::string(v, "text")?.into());
    }
    if action == "edit" {
        for patch in v["byte_edits"]
            .as_array()
            .ok_or("InvalidRequest: byte_edits")?
        {
            fields(patch, &["start", "end", "replacement"], &[])?;
            edit.patches.push(ByteEdit {
                start: protocol::number(patch, "start")?
                    .try_into()
                    .map_err(|_| "InvalidRequest: start")?,
                end: protocol::number(patch, "end")?
                    .try_into()
                    .map_err(|_| "InvalidRequest: end")?,
                replacement: protocol::string(patch, "replacement")?.into(),
            });
        }
        edit.patches.sort_by_key(|p| (p.start, p.end));
        edit.wire["byte_edits"] = json!(
            edit.patches
                .iter()
                .map(|p| json!({"start":p.start,"end":p.end,"replacement":p.replacement}))
                .collect::<Vec<_>>()
        );
    }
    Ok(edit)
}
fn decode_request(
    r: &Value,
    limits: &Limits,
) -> Result<(CandidateId, BaseToken, Vec<Edit>), String> {
    fields(
        r,
        &["version", "op", "id", "base", "token", "edits"],
        &["priority", "identity", "env"],
    )?;
    if r["version"] != 2 || r["op"] != "submit" {
        return Err("InvalidRequest: envelope".into());
    }
    protocol::number(r, "base")?;
    if r.get("priority")
        .is_some_and(|p| p != "background" && p != "selected")
    {
        return Err("InvalidRequest: priority".into());
    }
    let id = CandidateId {
        client: 1,
        nonce: protocol::number(r, "id")?,
    };
    let supplied = token(&r["token"])?;
    let raw = r["edits"].as_array().ok_or("InvalidRequest: edits")?;
    if raw.len() > limits.edit_files {
        return Err("ResourceLimit".into());
    }
    let mut ordered = raw.iter().collect::<Vec<_>>();
    ordered.sort_by(|a, b| {
        (
            a["path"]["root"].as_str(),
            a["path"]["path"].as_str(),
            a.to_string(),
        )
            .cmp(&(
                b["path"]["root"].as_str(),
                b["path"]["path"].as_str(),
                b.to_string(),
            ))
    });
    let edits = ordered
        .into_iter()
        .map(decode_edit)
        .collect::<Result<Vec<_>, _>>()?;
    let edit_bytes = edits
        .iter()
        .try_fold(0usize, |n, e| {
            n.checked_add(e.text.as_ref().map_or(0, String::len))
                .and_then(|n| {
                    e.patches
                        .iter()
                        .try_fold(n, |n, p| n.checked_add(p.replacement.len()))
                })
        })
        .ok_or("ResourceLimit")?;
    if edit_bytes > limits.edit_bytes
        || edits.iter().map(|e| e.patches.len()).sum::<usize>() > limits.byte_edits
    {
        return Err("ResourceLimit".into());
    }
    Ok((id, supplied, edits))
}
pub(super) fn validate_submit(r: &Value, limits: &Limits) -> Result<(), String> {
    decode_request(r, limits).map(|_| ())
}
impl CandidateWorkspace {
    /// Explicit host-reading capture. All compiler defaults were fixed at
    /// workspace construction; the client plan cannot change that policy.
    pub fn capture_v2(&self, mut plan: CapturePlan) -> Result<BaseLease, String> {
        let w = &self.inner;
        if w.active
            .compare_exchange(0, 1, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Err("Busy".into());
        }
        struct CaptureActive<'a>(&'a AtomicUsize);
        impl Drop for CaptureActive<'_> {
            fn drop(&mut self) {
                self.0.store(0, Ordering::Release);
            }
        }
        let _active = CaptureActive(&w.active);
        let _update = w.head_update.lock().unwrap();
        if plan.limits.bytes > w.limits.input_bytes
            || plan.limits.nodes > w.limits.input_operations
            || plan.limits.edits > w.limits.edit_files
            || plan.limits.attempts > w.limits.capture_attempts
        {
            return Err("ResourceLimit".into());
        }
        plan.environment = environment(w).0.to_vec();
        let namespace = hirsel_engine::with_compiler_stack(|| {
            NamespaceLease::capture_program(
                &plan,
                &DiskCapture,
                &w.namespace_budget,
                &w.compiler,
                w.config.clone(),
            )
        })
        .map_err(err)?;
        let program = hirsel_engine::with_compiler_stack(|| {
            namespace.load(&w.compiler, w.config.clone(), false)
        })
        .map_err(err)?;
        let selection = Arc::new(Selection::of(&program));
        let charge = w
            .budget
            .reserve(namespace.usage().charged_bytes + selection.retained_bytes())
            .ok_or("ResourceLimit")?;
        let mut state = w.state.lock().unwrap();
        let generation = state.generation.checked_add(1).ok_or("ResourceLimit")?;
        let inputs = Arc::new(Inputs {
            checked: Mutex::new(None),
            token: BaseToken {
                workspace: w.id,
                generation,
                inputs: namespace.inputs_digest(),
            },
            entry: namespace.entry().into(),
            namespace: Some(namespace),
            frozen: CaptureInputs::new(InputLimits {
                bytes: 0,
                operations: 0,
            })
            .freeze(),
            sources: Arc::new(BTreeMap::new()),
            scope: selection.scope.clone(),
            selection: Some(selection),
            _selection_charge: None,
            _charge: charge,
            _parent: None,
        });
        state.generation = generation;
        state.head = Some(inputs.clone());
        Ok(BaseLease {
            inputs,
            owner: Arc::downgrade(w),
        })
    }
    pub fn inspect_v2(
        &self,
        base: &BaseLease,
        root: &str,
        path: &str,
    ) -> Result<Inspection, String> {
        if !base.owner.ptr_eq(&Arc::downgrade(&self.inner)) {
            return Err("UnknownBase".into());
        }
        base.inputs
            .namespace
            .as_ref()
            .ok_or("UnknownBase")?
            .inspect(root, path)
            .map_err(io_error)
    }
    /// Strict simultaneous admission: syntax/limits, base, roles/duplicates,
    /// preconditions/ranges, allocation. Loading happens only on the serial worker.
    pub fn submit_v2(&self, base: &BaseLease, r: &Value) -> Result<CandidateLease, String> {
        let w = &self.inner;
        let (id, supplied, edits) = decode_request(r, &w.limits)?;
        if !base.owner.ptr_eq(&Arc::downgrade(w)) || supplied != base.inputs.token {
            return Err("UnknownBase".into());
        }
        let namespace = base.inputs.namespace.as_ref().ok_or("UnknownBase")?;
        // The same stage always reports in canonical target order. A path
        // that cannot resolve is ordered by its original raw spelling.
        let mut lookups = edits
            .into_iter()
            .map(|edit| {
                let fact = namespace
                    .inspect(&edit.key.root, &edit.key.path)
                    .map_err(io_error);
                let key = match &fact {
                    Ok(Inspection::Present { key, .. } | Inspection::Absent { key, .. }) => {
                        key.clone()
                    }
                    _ => edit.key.clone(),
                };
                ((key.root, key.path), edit, fact)
            })
            .collect::<Vec<_>>();
        lookups.sort_by(|a, b| a.0.cmp(&b.0));
        let mut targets = BTreeSet::new();
        let mut resolved = vec![];
        for (_, mut edit, fact) in lookups {
            let fact = fact?;
            let canonical = match &fact {
                Inspection::Present {
                    key,
                    kind,
                    editable,
                    ..
                } => {
                    if *kind != InputKind::File || !editable {
                        return Err("UnsupportedEdit: regular editable target required".into());
                    }
                    if edit.action == "delete" && *key != edit.key {
                        return Err("UnsupportedEdit: canonical delete required".into());
                    }
                    key.clone()
                }
                Inspection::Absent { key, .. } => {
                    if !namespace.can_create(key).map_err(io_error)? {
                        return Err("UnsupportedEdit: read-only parent".into());
                    }
                    key.clone()
                }
                Inspection::MissingParent => return Err("UnsupportedEdit: MissingParent".into()),
                Inspection::DanglingSymlink => {
                    return Err("UnsupportedEdit: DanglingSymlink".into());
                }
            };
            // Existing selected nonstandard names retain their roles. Eligible
            // unselected files use suffixes only at this editing boundary.
            let physical = namespace.path_for_key(&canonical).map_err(io_error)?;
            let selected = base.inputs.selection.as_ref().unwrap();
            let known = if edit.manifest {
                &selected.manifests
            } else {
                &selected.sources
            };
            let extension = if edit.manifest { "json" } else { "hir" };
            if physical.extension().and_then(|s| s.to_str()) != Some(extension)
                && !known
                    .iter()
                    .any(|p| namespace.canonicalize(p).is_ok_and(|p| p == physical))
            {
                return Err("UnsupportedEdit: role".into());
            }
            if !targets.insert((canonical.root.clone(), canonical.path.clone())) {
                return Err("DuplicateTarget".into());
            }
            edit.key = canonical;
            edit.wire["path"] = json!({"root":edit.key.root,"path":edit.key.path});
            resolved.push((edit, fact));
        }
        resolved
            .sort_by(|(a, _), (b, _)| (&a.key.root, &a.key.path).cmp(&(&b.key.root, &b.key.path)));
        let canonical = json!({"token":r["token"],"edits":resolved.iter().map(|(e,_)|e.wire.clone()).collect::<Vec<_>>()});
        let mut state = w.state.lock().unwrap();
        if let Some(old) = state.requests_v2.get(&id) {
            if old.canonical != canonical {
                return Err("RequestIdConflict".into());
            }
            return old
                .waiter
                .upgrade()
                .map(|waiter| CandidateLease { waiter })
                .ok_or("ExpiredRequestId".into());
        }
        if state.requests_v2.len() >= w.limits.requests
            || w.leases.load(Ordering::Acquire) >= w.limits.leases
        {
            return Err("ResourceLimit".into());
        }
        let mut changes = vec![];
        for (edit, fact) in resolved {
            match (&*edit.action, &fact) {
                ("create", Inspection::Absent { absent, .. }) if edit.absent == Some(*absent) => {}
                ("create", _) => return Err("PreconditionFailed".into()),
                (
                    _,
                    Inspection::Present {
                        present,
                        bytes: Some(bytes),
                        ..
                    },
                ) => {
                    if edit.present != Some(*present) {
                        return Err("PreconditionFailed".into());
                    }
                    if edit.bytes != Some(*bytes) {
                        return Err("StaleBytes".into());
                    }
                }
                _ => return Err("PreconditionFailed".into()),
            }
            let bytes = if edit.action == "edit" {
                let p = namespace.path_for_key(&edit.key).map_err(io_error)?;
                let old = namespace.read(&p).map_err(io_error)?;
                let old = std::str::from_utf8(&old).map_err(|_| "InvalidEdit: Utf8Boundary")?;
                edits::apply_bytes(old, &edit.patches, w.limits.input_bytes)
                    .map_err(|e| format!("InvalidEdit: {e:?}"))?
                    .into_bytes()
            } else {
                edit.text.unwrap_or_default().into_bytes()
            };
            if bytes.len() > w.limits.input_bytes || bytes.len() > u32::MAX as usize {
                return Err("ResourceLimit".into());
            }
            changes.push(match &*edit.action {
                "create" => Change::Create {
                    key: edit.key,
                    absent: edit.absent.unwrap(),
                    bytes: bytes.into(),
                },
                "delete" => Change::Delete {
                    key: edit.key,
                    present: edit.present.unwrap(),
                    expected_bytes: edit.bytes.unwrap(),
                },
                _ => Change::Replace {
                    key: edit.key,
                    present: edit.present.unwrap(),
                    expected_bytes: edit.bytes.unwrap(),
                    bytes: bytes.into(),
                },
            });
        }
        let (child, _delta) = namespace.overlay(&changes).map_err(overlay_error)?;
        let digest = child.inputs_digest();
        state.works.retain(|_, v| v.strong_count() > 0);
        let work = state
            .works
            .get(&digest)
            .and_then(Weak::upgrade)
            .filter(|w| {
                w.inputs
                    .namespace
                    .as_ref()
                    .is_some_and(|n| n.same_namespace(&child))
                    && !matches!(
                        *w.state.lock().unwrap(),
                        WorkState::Stopping | WorkState::Failed(_)
                    )
            });
        let work = match work {
            Some(work) => work,
            None => {
                let charge = w
                    .budget
                    .reserve(child.usage().charged_bytes + 512)
                    .ok_or("ResourceLimit")?;
                let inputs = Arc::new(Inputs {
                    checked: Mutex::new(None),
                    token: BaseToken {
                        workspace: w.id,
                        generation: supplied.generation,
                        inputs: digest,
                    },
                    entry: child.entry().into(),
                    namespace: Some(child),
                    selection: None,
                    _selection_charge: None,
                    frozen: CaptureInputs::new(InputLimits {
                        bytes: 0,
                        operations: 0,
                    })
                    .freeze(),
                    sources: Arc::new(BTreeMap::new()),
                    scope: empty_scope(),
                    _charge: charge,
                    _parent: None,
                });
                let work = Arc::new(Work {
                    inputs,
                    state: Mutex::new(WorkState::Queued),
                    waiters: Mutex::new(vec![]),
                });
                state.works.insert(digest, Arc::downgrade(&work));
                work
            }
        };
        let request_charge = w
            .budget
            .reserve(json_bytes(&canonical) + 512)
            .ok_or("ResourceLimit")?;
        let waiter_charge = w.budget.reserve(1024).ok_or("ResourceLimit")?;
        let waiter = Arc::new(Waiter {
            id,
            parent: supplied,
            parent_inputs: base.inputs.clone(),
            work,
            state: Mutex::new(WaiterState::Pending),
            owner: Arc::downgrade(w),
            _charge: waiter_charge,
        });
        waiter
            .work
            .waiters
            .lock()
            .unwrap()
            .push(Arc::downgrade(&waiter));
        w.leases.fetch_add(1, Ordering::AcqRel);
        state.requests_v2.insert(
            id,
            RequestRecord {
                canonical,
                waiter: Arc::downgrade(&waiter),
                _charge: request_charge,
            },
        );
        Ok(CandidateLease { waiter })
    }
}
