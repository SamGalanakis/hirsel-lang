//! Explicit v2 connection route. V1 decoding/serialization stays in parent.
use super::*;
use crate::candidate::v2::{CapturePlan, Inspection, Key, fields};
use hirsel_engine::loader_inputs::namespace::{Limits as NamespaceLimits, Root};
impl Connection {
    pub(super) fn dispatch_v2(&mut self, r: &Value) -> Result<Value, String> {
        if r.get("__invalid_json").is_some() {
            return Err("InvalidRequest: duplicate fields".into());
        }
        let op = string(r, "op")?;
        let required: &[&str] = match op {
            "capture" => &["version", "op", "entry", "roots"],
            "inspect" => &["version", "op", "base", "paths"],
            "submit" => return self.submit_wire_v2(r),
            "head" | "counters" => &["version", "op"],
            "result" | "cancel" | "release" => &["version", "op", "lease"],
            "promote" => &["version", "op", "id", "lease", "base"],
            "observe_disk" => &["version", "op", "target", "handle"],
            _ => return Err("UnsupportedOperation".into()),
        };
        fields(
            r,
            required,
            if op == "capture" {
                &["identity", "env", "std_manifest", "excluded"]
            } else {
                &["identity", "env"]
            },
        )?;
        match op {
            "capture" => {
                let q = self.queue.0.lock().unwrap();
                if q.active || !q.jobs.is_empty() {
                    return Err("Busy".into());
                }
                drop(q);
                let id = self.allocate()?;
                let l = &self.workspace.inner.limits;
                let roots = r["roots"].as_array().ok_or("InvalidRequest: roots")?;
                if roots.len() > l.input_operations {
                    return Err("ResourceLimit".into());
                }
                let roots = roots
                    .iter()
                    .map(|v| {
                        fields(v, &["id", "path", "editable"], &[])?;
                        let spelling = string(v, "path")?;
                        let path = if spelling.is_empty() {
                            self.root.clone()
                        } else {
                            self.root.join(spelling)
                        };
                        let editable = v["editable"].as_bool().ok_or("InvalidRequest: editable")?;
                        if editable && !path.starts_with(&self.root) {
                            return Err("OutsideWorkspace".into());
                        }
                        Ok(Root {
                            id: string(v, "id")?.into(),
                            path,
                            editable,
                        })
                    })
                    .collect::<Result<Vec<_>, String>>()?;
                let entry = self.root.join(string(r, "entry")?);
                if !entry.starts_with(&self.root) {
                    return Err("OutsideWorkspace".into());
                }
                let std_manifest = match r.get("std_manifest") {
                    None | Some(Value::Null) => None,
                    Some(Value::String(p)) => Some(self.root.join(p)),
                    _ => return Err("InvalidRequest: std_manifest".into()),
                };
                let mut excluded = BTreeSet::new();
                if let Some(value) = r.get("excluded") {
                    for v in value.as_array().ok_or("InvalidRequest: excluded")? {
                        excluded.insert(
                            self.root
                                .join(v.as_str().ok_or("InvalidRequest: excluded")?),
                        );
                    }
                }
                let base = self.workspace.capture_v2(CapturePlan {
                    roots,
                    entry,
                    std_manifest,
                    excluded,
                    failed_probes: vec![],
                    environment: vec![],
                    limits: NamespaceLimits {
                        bytes: l.input_bytes,
                        nodes: l.input_operations,
                        aliases: l.input_operations,
                        depth: 64,
                        link_hops: 40,
                        edits: l.edit_files,
                        attempts: l.capture_attempts,
                    },
                })?;
                let reply = base_wire_v2(id, &base);
                self.bases.insert(id, base);
                Ok(reply)
            }
            "inspect" => {
                let base = self.bases.get(&number(r, "base")?).ok_or("UnknownBase")?;
                let paths = r["paths"].as_array().ok_or("InvalidRequest: paths")?;
                if paths.len() > self.workspace.inner.limits.edit_files {
                    return Err("ResourceLimit".into());
                }
                let facts = paths
                    .iter()
                    .map(|v| {
                        fields(v, &["root", "path"], &[])?;
                        let fact = self.workspace.inspect_v2(
                            base,
                            string(v, "root")?,
                            string(v, "path")?,
                        )?;
                        Ok(inspection_wire(fact))
                    })
                    .collect::<Result<Vec<_>, String>>()?;
                Ok(json!({"token":token_json(base.token()),"nodes":facts}))
            }
            "head" => {
                let id = self.allocate()?;
                let base = self.workspace.head_lease().ok_or("UnknownBase")?;
                if base.inputs.namespace.is_none() {
                    return Err("UnknownBase".into());
                }
                let reply = base_wire_v2(id, &base);
                self.bases.insert(id, base);
                Ok(reply)
            }
            "result" => {
                let lease = self
                    .candidates
                    .get(&number(r, "lease")?)
                    .ok_or("UnknownLease")?;
                match self.workspace.poll(lease).map_err(|e| format!("{e:?}"))? {
                    CandidateResult::Pending => Ok(json!({"pending":true})),
                    CandidateResult::Ready(receipt) => {
                        let mut value = receipt_json(&receipt);
                        value["scope_complete"] = json!(receipt._selection.is_some());
                        value["selection"] = receipt
                            ._selection
                            .as_ref()
                            .map_or(Value::Null, |s| s.facts.clone());
                        value["entry_path"] = json!(lease.waiter.work.inputs.entry);
                        Ok(value)
                    }
                }
            }
            "observe_disk" => {
                let target = string(r, "target")?;
                let handle = number(r, "handle")?;
                let inputs = match target {
                    "base" => self.bases.get(&handle).ok_or("UnknownBase")?.inputs.clone(),
                    "candidate" => self
                        .candidates
                        .get(&handle)
                        .ok_or("UnknownLease")?
                        .waiter
                        .work
                        .inputs
                        .clone(),
                    _ => return Err("InvalidRequest: target".into()),
                };
                let namespace = inputs.namespace.as_ref().ok_or("UnknownBase")?;
                let generation = self.workspace.head().map_or(0, |h| h.generation);
                let result = namespace.observe_disk();
                Ok(match result {
                    Ok(differences) => {
                        json!({"inputs":hex(inputs.token.inputs),"observation_id":self.requests,
                        "head_generation_at_observation":generation,"status":if differences.is_empty(){"MatchesObserved"}else{"Changed"},"differences":differences})
                    }
                    Err(e) => {
                        json!({"inputs":hex(inputs.token.inputs),"observation_id":self.requests,
                        "head_generation_at_observation":generation,"status":"Incomplete","error":e.to_string(),"differences":[]})
                    }
                })
            }
            // Lifecycle fields and replies deliberately reuse the existing code.
            _ => self.dispatch(r),
        }
    }
    fn submit_wire_v2(&mut self, r: &Value) -> Result<Value, String> {
        crate::candidate::v2::validate_submit(r, &self.workspace.inner.limits)?;
        let id = self.allocate()?;
        let base = self.bases.get(&number(r, "base")?).ok_or("UnknownBase")?;
        let mut q = self.queue.0.lock().unwrap();
        if q.jobs.len() >= HANDLES {
            return Err("ResourceLimit".into());
        }
        let lease = self.workspace.submit_v2(base, r)?;
        q.jobs.push_back(Job {
            lease: lease.clone(),
            selected: r["priority"] == "selected",
            queued: std::time::Instant::now(),
        });
        self.candidates.insert(id, lease);
        self.queue.1.notify_one();
        Ok(json!({"lease":id}))
    }
}
fn base_wire_v2(id: u64, base: &BaseLease) -> Value {
    let n = base.inputs.namespace.as_ref().unwrap();
    let u = n.usage();
    json!({"base":id,"token":token_json(base.token()),"namespace":hex(n.digest()),
        "scope_complete":true,"packages":base.inputs.scope.packages,
        "usage":{"nodes":u.nodes,"directories":u.directories,"entries":u.directory_entries,
            "aliases":u.aliases,"file_bytes":u.file_bytes,
            "charged_bytes":u.charged_bytes}})
}
fn key_wire(k: Key) -> Value {
    json!({"root":k.root,"path":k.path})
}
fn inspection_wire(f: Inspection) -> Value {
    match f {
        Inspection::Present {
            key,
            kind,
            editable,
            present,
            bytes,
        } => json!({"kind":"Present","path":key_wire(key),
            "node_kind":format!("{kind:?}"),"editable":editable,"expected":{"present":hex(present),"bytes":bytes.map(hex)}}),
        Inspection::Absent {
            key,
            parent,
            absent,
        } => {
            json!({"kind":"Absent","path":key_wire(key),"parent":key_wire(parent),"expected":{"absent":hex(absent)}})
        }
        Inspection::MissingParent => json!({"kind":"MissingParent"}),
        Inspection::DanglingSymlink => json!({"kind":"DanglingSymlink"}),
    }
}
