//! Offline evidence views. No host provider, clock or exporter is invoked here.
use hirsel_interp::observation::{Effect, Observable};
use hirsel_rt::io::{Completion, Event, Request};
use hirsel_rt::{journal, launch};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

/// An archival identity chosen by tooling, bound to the exact captured bytes. A
/// digest alone cannot distinguish independent captures with identical contents.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EvidenceId {
    capture: String,
    sha256: String,
}
impl EvidenceId {
    pub fn new(capture: &str, bytes: &[u8]) -> Result<Self, String> {
        if capture.is_empty() || capture.len() > 256 || capture.chars().any(char::is_control) {
            return Err(
                "capture identity must contain 1..256 bytes without control characters".into(),
            );
        }
        Ok(Self {
            capture: capture.into(),
            sha256: hirsel_run_schema::hex(&Sha256::digest(bytes)),
        })
    }
    fn value(&self) -> Value {
        json!({"capture": self.capture, "sha256": self.sha256})
    }
}

/// A validated prefix; evidence bytes and the archival name remain inseparable.
pub struct View {
    evidence: EvidenceId,
    document: Value,
}
impl View {
    /// Accept complete ordinary/durable recordings and inspect confirmed prefixes.
    /// Only a final unterminated ordinary line is cut; malformed terminated lines
    /// (including a bad trailer) remain corruption. Header loss is never accepted.
    pub fn parse(capture: &str, bytes: &[u8]) -> Result<Self, String> {
        let evidence = EvidenceId::new(capture, bytes)?;
        let (recording, complete, valid, anchors, pending, outcome, format) =
            if journal::is_journal(bytes) {
                let recovered = journal::recover(bytes).map_err(|e| {
                    format!(
                        "corrupt durable recording: {}",
                        payload_summary(e.to_string().as_bytes())
                    )
                })?;
                let anchors: Vec<_> = recovered
                    .event_prepared
                    .iter()
                    .map(|id| {
                        id.map(|id| {
                            let p = &recovered.prepared[id as usize];
                            json!({"request_id": id, "task": p.task, "issue": p.issue})
                        })
                        .unwrap_or(Value::Null)
                    })
                    .collect();
                let pending: Vec<_> = recovered
                    .in_doubt
                    .iter()
                    .map(|p| {
                        Ok(json!({"request_id": p.id, "task": p.task, "issue": p.issue,
                        "request": request_text(&p.request)?}))
                    })
                    .collect::<Result<_, String>>()?;
                let outcome = recovered.outcome.as_ref().map(|outcome| {
                    use hirsel_run_schema::RunOutcome;
                    let canonical = journal::render_outcome(outcome);
                    match outcome {
                        RunOutcome::Returned(_) | RunOutcome::BudgetExceeded(_) => canonical,
                        _ => {
                            let (kind, details) =
                                canonical.split_once(' ').unwrap_or((&canonical, ""));
                            format!("{kind} details={}", payload_summary(details.as_bytes()))
                        }
                    }
                });
                (
                    recovered.recording.clone(),
                    recovered.complete(),
                    recovered.valid,
                    anchors,
                    pending,
                    outcome,
                    "durable",
                )
            } else {
                let valid = hirsel_rt::tape_binary::confirmed_prefix(bytes)?;
                let (recording, complete) = hirsel_rt::tape_binary::parse_prefix(&bytes[..valid])
                    .map_err(|e| {
                    format!(
                        "corrupt ordinary recording: {}",
                        payload_summary(e.as_bytes())
                    )
                })?;
                if complete && valid != bytes.len() {
                    return Err("bytes after complete recording".into());
                }
                let anchors = vec![Value::Null; recording.events.len()];
                (
                    recording,
                    complete,
                    valid,
                    anchors,
                    vec![],
                    None,
                    "ordinary",
                )
            };
        let events = recording
            .events
            .iter()
            .zip(anchors)
            .enumerate()
            .map(|(ordinal, (event, anchor))| {
                let text = event_text(event)?;
                let (request, completion) =
                    text.split_once(" -> ").ok_or("event has no completion")?;
                let origin = if matches!(
                    event.request,
                    Request::Ready { .. } | Request::Schedule { .. } | Request::CpuControl { .. }
                ) {
                    "core"
                } else {
                    "application"
                };
                Ok(
                    json!({"ordinal": ordinal, "origin": origin, "request": request,
                "completion": completion, "durable_anchor": anchor, "source": null,
                "original_time_ns": null, "accepted_bytes": accepted_bytes(event)}),
                )
            })
            .collect::<Result<Vec<_>, String>>()?;
        let document = json!({"schema": "hirsel-observation-view", "version": 1,
            "evidence": evidence.value(), "format": format, "complete": complete,
            "torn_tail": valid < bytes.len(), "valid_bytes": valid,
            "outcome": outcome, "in_doubt": pending, "events": events,
            "replay_observer": null, "time_basis": "logical-order"});
        Ok(Self { evidence, document })
    }

    /// Bind replay to this capture's bytes and the selected program, then attach
    /// separately ordered semantic observations. Replaying never admits live grants.
    pub fn replay_enrich(
        &mut self,
        source: &std::path::Path,
        tape: &std::path::Path,
        analysis: &str,
        max_steps: Option<u64>,
    ) -> Result<Observable, String> {
        let check = || -> Result<(), String> {
            let bytes = std::fs::read(tape).map_err(|e| e.to_string())?;
            if EvidenceId::new(&self.evidence.capture, &bytes)? != self.evidence {
                return Err("replay evidence digest mismatch".into());
            }
            Ok(())
        };
        check()?;
        let observed = crate::replay_observations(source, tape, max_steps).map_err(|e| {
            format!(
                "replay analysis failed: {}",
                payload_summary(e.to_string().as_bytes())
            )
        })?;
        check()?;
        self.enrich(analysis, &observed)?;
        Ok(observed)
    }

    /// Validate diagnostic sidecar v1. Samples are host-monotonic nanoseconds,
    /// keyed to exact evidence and event ordinals; sparse samples remain unknown.
    pub fn apply_timing(&mut self, text: &str) -> Result<(), String> {
        let sidecar: Value = serde_json::from_str(text).map_err(|e| e.to_string())?;
        if sidecar["schema"] != "hirsel-observation-timing"
            || sidecar["version"] != 1
            || sidecar["evidence"] != self.evidence.value()
            || sidecar["clock"] != "host-monotonic-ns"
        {
            return Err("timing sidecar schema, clock or evidence mismatch".into());
        }
        let samples = sidecar["samples"]
            .as_array()
            .ok_or("missing timing samples")?;
        let count = self.document["events"].as_array().expect("events").len();
        let mut checked = Vec::with_capacity(samples.len());
        let mut previous = None;
        for sample in samples {
            let ordinal = sample["ordinal"].as_u64().ok_or("bad timing ordinal")?;
            let ns = sample["ns"].as_u64().ok_or("bad timing nanoseconds")?;
            if ordinal >= count as u64 || previous.is_some_and(|(o, t)| ordinal <= o || ns < t) {
                return Err(
                    "timing samples must have increasing ordinals and monotonic timestamps".into(),
                );
            }
            checked.push((ordinal as usize, ns));
            previous = Some((ordinal, ns));
        }
        for event in self.document["events"].as_array_mut().expect("events") {
            event["original_time_ns"] = Value::Null;
        }
        for (ordinal, ns) in checked {
            self.document["events"][ordinal]["original_time_ns"] = json!(ns);
        }
        self.document["time_basis"] = json!("host-monotonic-ns");
        self.document["timing"] =
            json!({"clock": "host-monotonic-ns", "includes_runtime_overhead": true});
        Ok(())
    }

    fn enrich(&mut self, analysis: &str, observed: &Observable) -> Result<(), String> {
        if analysis.is_empty() || analysis.len() > 256 || analysis.chars().any(char::is_control) {
            return Err("invalid analysis identity".into());
        }
        let effects: Vec<_> = observed.effects.iter().enumerate().map(|(ordinal, e)| {
            let effect = match &e.effect {
                Effect::CapabilityConsumed(event) => json!({"kind": "capability", "event": {
                    "capability": event.capability, "operation": event.operation,
                    "input": payload_summary(&event.input), "output": payload_summary(&event.output)}}),
                Effect::HandleDropped { kind, id } => json!({"kind": "handle-drop", "handle_kind": kind, "id": id}),
                Effect::ResourceCreated { definition } => json!({"kind": "resource-construction", "definition": definition.to_string()}),
                Effect::Finalizer { definition } => json!({"kind": "finalizer-entry", "definition": definition.to_string()}),
                Effect::Forgotten { definition } => json!({"kind": "forget", "definition": definition.to_string()}),
            };
            json!({"ordinal": ordinal, "origin": "replay-observer", "tape_ordinal": null,
                "source": {"definition": e.anchor.source.definition.to_string(),
                    "line": e.anchor.source.line, "column": e.anchor.source.column}, "effect": effect})
        }).collect();
        self.document["replay_observer"] = json!({"analysis": analysis,
            "evidence": self.evidence.value(), "semantic_schema_version": observed.schema_version,
            "effects": effects});
        Ok(())
    }

    pub fn normalized(&self) -> String {
        format!(
            "{}\n",
            serde_json::to_string_pretty(&self.document).expect("JSON value")
        )
    }

    /// Select original timing when evidence supplies it; sparse unknown events
    /// retain a separate logical track. Never turn absent times into durations.
    pub fn perfetto(&self) -> String {
        if self.document.get("timing").is_none() {
            return self.logical_perfetto();
        }
        let mut events = vec![
            json!({"ph": "M", "pid": 1, "tid": 1, "name": "thread_name",
                "args": {"name": "Original host-monotonic time (includes runtime overhead)"}}),
            json!({"ph": "M", "pid": 2, "tid": 1, "name": "thread_name",
                "args": {"name": "Unknown time: logical order (ordinals; not elapsed time)"}}),
        ];
        for event in self.document["events"].as_array().expect("events") {
            let ns = event["original_time_ns"].as_u64();
            // Perfetto's Chrome JSON import uses microseconds. Preserve the exact
            // ns in args and split before conversion to avoid rounding large u64s.
            let ts = ns.map_or_else(
                || event["ordinal"].clone(),
                |ns| json!((ns / 1000) as f64 + (ns % 1000) as f64 / 1000.0),
            );
            events.push(
                json!({"name": event["request"].as_str().unwrap().split(' ').next(),
                "cat": if ns.is_some() { "hirsel.host-monotonic" } else { "hirsel.logical-order" },
                "ph": "I", "s": "t", "pid": if ns.is_some() { 1 } else { 2 }, "tid": 1,
                "ts": ts, "args": event}),
            );
        }
        format!("{}\n", serde_json::to_string_pretty(&json!({"traceEvents": events,
            "hirsel": {"version": 1, "evidence": self.evidence.value(),
                "time_basis": "host-monotonic-ns", "complete": self.document["complete"],
                "includes_runtime_overhead": true,
                "warning": "Original monotonic instants; unknown samples use a separate logical track; no durations inferred"}})).expect("JSON value"))
    }

    /// Perfetto's Chrome trace-event import. Instants represent ordinal positions,
    /// never measured duration; no begin/end nesting is fabricated from effects.
    pub fn logical_perfetto(&self) -> String {
        let mut events = vec![json!({"ph": "M", "pid": 1, "tid": 1,
            "name": "thread_name", "args": {"name": "Logical order (ordinals; not elapsed time)"}})];
        for event in self.document["events"].as_array().expect("events") {
            events.push(
                json!({"name": event["request"].as_str().unwrap().split(' ').next(),
                "cat": "hirsel.logical-order", "ph": "I", "s": "t", "pid": 1, "tid": 1,
                "ts": event["ordinal"], "args": event}),
            );
        }
        format!(
            "{}\n",
            serde_json::to_string_pretty(&json!({"traceEvents": events,
            "hirsel": {"version": 1, "evidence": self.evidence.value(),
                "time_basis": "logical-order", "complete": self.document["complete"],
                "warning": "Synthetic ordinal coordinates; no original latency is available"}}))
            .expect("JSON value")
        )
    }
}

fn event_text(event: &Event) -> Result<String, String> {
    let text = launch::render_tape(std::slice::from_ref(event))?;
    let line = text
        .split_once('\n')
        .ok_or("missing tape header")?
        .1
        .trim_end_matches('\n');
    let (request, completion) = line.split_once(" -> ").ok_or("missing completion")?;
    let (completion, fault) = completion.split_once(" ! ").unwrap_or((completion, ""));
    let mut result = format!(
        "{} -> {}",
        redact_request(&event.request, request),
        redact_completion(&event.completion, completion)
    );
    if !fault.is_empty() {
        result.push_str(" ! ");
        result.push_str(fault);
    }
    Ok(result)
}

// Keep redaction at the diagnostic boundary: replay's canonical codec must retain
// the original bytes. Reuse the view's existing SHA-256 implementation.
fn payload_summary(bytes: &[u8]) -> String {
    format!(
        "[redacted len={} sha256={}]",
        bytes.len(),
        &hirsel_run_schema::hex(&Sha256::digest(bytes))[..12]
    )
}

fn redact_request(request: &Request, canonical: &str) -> String {
    use Request as R;
    let mut words: Vec<String> = canonical.split(' ').map(String::from).collect();
    let last = words.len() - 1;
    match request {
        R::Print { bytes, .. }
        | R::PrintStderr { bytes, .. }
        | R::FileWrite { bytes, .. }
        | R::FileAppend { bytes, .. }
        | R::Write { bytes, .. }
        | R::WriteAll { bytes, .. }
        | R::NetWrite { bytes, .. }
        | R::NetWriteAll { bytes, .. }
        | R::ChildWrite { bytes, .. } => words[last] = payload_summary(bytes),
        R::PutNew { bytes, .. } | R::UdpSend { bytes, .. } => {
            words[last - 1] = payload_summary(bytes);
        }
        R::TestLog { text, .. } | R::TestSnapshot { text, .. } => {
            words[last] = payload_summary(text.as_bytes());
        }
        // These composite inputs may embed credentials, child output, runner
        // arguments or granted Env values. Summarize the entire canonical argument
        // text rather than duplicate their nested codecs in a diagnostic tool.
        R::ProcessRun { .. }
        | R::ProcessPipeline { .. }
        | R::ProcessSpawn { .. }
        | R::ProcessSpawnPipeline { .. }
        | R::TestProcess { .. }
        | R::TestNet { .. }
        | R::TestDir { .. }
        | R::RunnerCall(_)
        | R::CallReturn(_) => {
            let (operation, arguments) = canonical.split_once(' ').expect("request arguments");
            return format!(
                "{operation} arguments={}",
                payload_summary(arguments.as_bytes())
            );
        }
        _ => {}
    }
    words.join(" ")
}

fn redact_completion(completion: &Completion, canonical: &str) -> String {
    use Completion as C;
    use hirsel_run_schema::{CallOutcome, PipelineOutcome, ProcessOutcome};
    let mut words: Vec<String> = canonical.split(' ').map(String::from).collect();
    match completion {
        C::Random(Ok(bytes))
        | C::Input(Ok(bytes))
        | C::Bytes(Ok(bytes))
        | C::NetBytes(Ok(bytes))
        | C::ChildBytes(Ok(bytes)) => words[2] = payload_summary(bytes),
        C::Env(Ok(Some(value))) => words[2] = payload_summary(value.as_bytes()),
        C::Datagram(Ok((bytes, _, _))) => words[2] = payload_summary(bytes),
        C::Certificates(Ok(Some(bytes))) => words[2] = format!("der:{}", payload_summary(bytes)),
        C::View(view) => {
            words[1] = payload_summary(view.bytes.as_ref().expect("rendered mapped bytes"));
        }
        C::Process(ProcessOutcome::Output { stdout, stderr, .. }) => {
            words[3] = payload_summary(stdout);
            words[4] = payload_summary(stderr);
        }
        C::Pipeline(PipelineOutcome::Output { stdout, stderr, .. }) => {
            words[2] = payload_summary(stdout);
            words[3] = payload_summary(stderr);
        }
        C::GroupExit(Ok(output)) => {
            let first = words.len() - output.stderr.len();
            for (word, bytes) in words[first..].iter_mut().zip(&output.stderr) {
                *word = payload_summary(bytes);
            }
        }
        C::Runner(c) => match &c.outcome {
            CallOutcome::Ok(bytes) | CallOutcome::Failed(bytes) | CallOutcome::Faulted(bytes) => {
                words[2] = payload_summary(bytes);
            }
            _ => {}
        },
        _ => {}
    }
    words.join(" ")
}

fn request_text(request: &Request) -> Result<String, String> {
    let text = event_text(&Event {
        request: request.clone(),
        completion: Completion::Checked(false),
        fault: None,
        resources: None,
    })?;
    Ok(text.split_once(" -> ").ok_or("missing request")?.0.into())
}

fn accepted_bytes(event: &Event) -> Option<u64> {
    use Completion as C;
    use Request as R;
    let bytes = match &event.request {
        R::Print { bytes, .. }
        | R::PrintStderr { bytes, .. }
        | R::FileWrite { bytes, .. }
        | R::FileAppend { bytes, .. }
        | R::Write { bytes, .. }
        | R::WriteAll { bytes, .. }
        | R::NetWrite { bytes, .. }
        | R::NetWriteAll { bytes, .. } => Some(bytes),
        _ => None,
    };
    if let Some(bytes) = bytes {
        if let Some(fault) = &event.fault {
            return Some(fault.partial as u64);
        }
        return match &event.completion {
            C::Unit(Ok(())) | C::NetUnit(Ok(())) => Some(bytes.len() as u64),
            C::Count(Ok(n)) | C::Size(Ok(n)) => Some(*n),
            C::Blocked { .. } | C::Held => None,
            _ => Some(0),
        };
    }
    match &event.completion {
        C::Input(Ok(b)) | C::Bytes(Ok(b)) | C::NetBytes(Ok(b)) | C::ChildBytes(Ok(b)) => {
            Some(b.len() as u64)
        }
        C::Datagram(Ok((b, _, _))) => Some(b.len() as u64),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture_header() -> launch::Recording {
        launch::Recording {
            arguments: vec![],
            run_identity: [0; 32],
            hash_seed: 2,
            random_seed: 3,
            test_seed: 5,
            binding: None,
            tree: hirsel_rt::io::Tree::new(Default::default()).unwrap(),
            events: vec![],
        }
    }
    fn fixture_events() -> Vec<Event> {
        vec![
            Event {
                request: Request::Stdout { io: 1 },
                completion: Completion::Handle(Ok(2)),
                fault: None,
                resources: None,
            },
            Event {
                request: Request::Print {
                    stdout: 2,
                    bytes: b"hello".to_vec(),
                },
                completion: Completion::Unit(Ok(())),
                fault: None,
                resources: None,
            },
        ]
    }
    // Fixtures are regenerated by their owning tests through the production
    // writers. No checked-in tape bytes survive a durable format migration.
    fn ordinary() -> &'static [u8] {
        static BYTES: std::sync::LazyLock<Vec<u8>> = std::sync::LazyLock::new(|| {
            let mut bytes = vec![];
            let mut writer = launch::RecordingWriter::new(&mut bytes, &fixture_header()).unwrap();
            for event in fixture_events() {
                writer.append(&event).unwrap();
            }
            writer.finish().unwrap();
            drop(writer);
            bytes
        });
        &BYTES
    }
    fn durable() -> &'static [u8] {
        static BYTES: std::sync::LazyLock<Vec<u8>> = std::sync::LazyLock::new(|| {
            use hirsel_rt::journal::{Journal, Prepared, SimJournalFile};
            let file = std::rc::Rc::new(std::cell::RefCell::new(SimJournalFile::default()));
            let mut writer =
                journal::DurableJournal::create(file.clone(), &fixture_header()).unwrap();
            for (id, event) in fixture_events().iter().enumerate() {
                writer
                    .prepare(&Prepared {
                        id: id as u64,
                        task: if id == 0 { vec![] } else { vec![0, 1] },
                        issue: 0,
                        request: event.request.clone(),
                    })
                    .unwrap();
                writer.barrier().unwrap();
                writer.event(Some(id as u64), event).unwrap();
                writer.barrier().unwrap();
            }
            writer
                .end(&hirsel_run_schema::RunOutcome::Returned(
                    hirsel_run_schema::Returned::Ok,
                ))
                .unwrap();
            file.borrow().bytes().to_vec()
        });
        &BYTES
    }
    fn document(bytes: &[u8]) -> Value {
        serde_json::from_str(&View::parse("fixture", bytes).unwrap().normalized()).unwrap()
    }

    #[test]
    fn cpu_refusal_receipt_is_core_input_without_a_capability_or_committed_bytes() {
        let mut bytes = vec![];
        let mut writer = launch::RecordingWriter::new(&mut bytes, &fixture_header()).unwrap();
        writer
            .append(&Event {
                request: Request::CpuControl {
                    width: 4,
                    action: 0,
                    count: 0,
                    outcome: 9,
                },
                completion: Completion::Ready,
                fault: None,
                resources: None,
            })
            .unwrap();
        writer.finish().unwrap();
        drop(writer);
        let view = document(&bytes);
        let event = &view["events"][0];
        assert_eq!(view["complete"], true);
        assert_eq!(event["origin"], "core");
        assert_eq!(event["request"], "cpu_control 4 0 0 9");
        assert!(event["durable_anchor"].is_null());
        assert!(event["accepted_bytes"].is_null());
    }

    #[test]
    fn complete_fixtures_and_golden_views_are_stable() {
        for (name, bytes, normalized, perfetto) in [
            (
                "ordinary",
                ordinary(),
                include_str!("../tests/fixtures/observation-view/ordinary.view.json"),
                include_str!("../tests/fixtures/observation-view/ordinary.perfetto.json"),
            ),
            (
                "durable",
                durable(),
                include_str!("../tests/fixtures/observation-view/durable.view.json"),
                include_str!("../tests/fixtures/observation-view/durable.perfetto.json"),
            ),
        ] {
            let view = View::parse("fixture", bytes).unwrap();
            let mut expected: Value = serde_json::from_str(normalized).unwrap();
            expected["evidence"] = view.evidence.value();
            expected["valid_bytes"] = json!(bytes.len());
            assert_eq!(
                serde_json::from_str::<Value>(&view.normalized()).unwrap(),
                expected,
                "{name}"
            );
            let mut expected: Value = serde_json::from_str(perfetto).unwrap();
            expected["hirsel"]["evidence"] = view.evidence.value();
            assert_eq!(
                serde_json::from_str::<Value>(&view.logical_perfetto()).unwrap(),
                expected,
                "{name}"
            );
            assert_eq!(view.document["complete"], true);
            for e in view.document["events"].as_array().unwrap() {
                assert!(e["source"].is_null());
                assert!(e["original_time_ns"].is_null());
            }
        }
        let ordinary = document(ordinary());
        assert!(ordinary["events"][0]["durable_anchor"].is_null());
        let durable = document(durable());
        assert_eq!(durable["events"][0]["durable_anchor"]["task"], json!([]));
        assert_eq!(
            durable["events"][1]["durable_anchor"]["task"],
            json!([0, 1])
        );
        assert_eq!(durable["events"][1]["accepted_bytes"], 5);
    }

    #[test]
    fn payloads_are_redacted_in_completed_pending_and_terminal_records() {
        use hirsel_rt::journal::Journal;
        let secret = b"recorded-credential-DO-NOT-PRINT";
        let value = String::from_utf8(secret.to_vec()).unwrap();
        let encoded = hirsel_run_schema::hex(secret);
        let summary = payload_summary(secret);
        let command = hirsel_run_schema::Command {
            program: "child".into(),
            args: vec![value.clone()],
            directory: None,
            env: Some(vec![("TOKEN".into(), value.clone())]),
            own_group: false,
        };
        let events = vec![
            (
                Request::RandomBytes {
                    random: 1,
                    count: secret.len() as u64,
                },
                Completion::Random(Ok(secret.to_vec())),
            ),
            (
                Request::EnvGet {
                    env: 1,
                    name: "TOKEN".into(),
                },
                Completion::Env(Ok(Some(value.clone()))),
            ),
            (
                Request::FileRead {
                    directory: 1,
                    file: 2,
                    max: 100,
                },
                Completion::Bytes(Ok(secret.to_vec())),
            ),
            (
                Request::MapView {
                    directory: 1,
                    mapped: 2,
                },
                Completion::View(hirsel_rt::io::MapView::of(secret.to_vec())),
            ),
            (
                Request::NetWrite {
                    net: 1,
                    stream: 2,
                    bytes: secret.to_vec(),
                },
                Completion::Count(Ok(secret.len() as u64)),
            ),
            (
                Request::ProcessRun {
                    process: 1,
                    command,
                    stdin: secret.to_vec(),
                    max_output: 100,
                    deadline: None,
                },
                Completion::Process(hirsel_run_schema::ProcessOutcome::Output {
                    status: hirsel_run_schema::ExitStatus::Exited(0),
                    stdout: secret.to_vec(),
                    stderr: secret.to_vec(),
                }),
            ),
            (
                Request::ProcessSpawnPipeline {
                    process: 1,
                    stages: vec![],
                    max_stderr: 100,
                },
                Completion::GroupExit(Ok(hirsel_run_schema::GroupOutput {
                    statuses: vec![hirsel_run_schema::ExitStatus::Exited(0)],
                    stderr: vec![secret.to_vec()],
                })),
            ),
            (
                Request::CallReturn(hirsel_run_schema::CalleeOutcome::Ok(secret.to_vec())),
                Completion::Ready,
            ),
        ]
        .into_iter()
        .map(|(request, completion)| Event {
            request,
            completion,
            fault: None,
            resources: None,
        })
        .collect::<Vec<_>>();
        let mut recording = launch::parse_recording_bytes(ordinary()).unwrap();
        recording.events = events.clone();
        let bytes = launch::render_recording(&recording).unwrap();
        let mut view = View::parse("fixture", bytes.as_bytes()).unwrap();
        let observed = Observable {
            blocked: None,
            signaled: None,
            schema_version: 1,
            returned_type: None,
            returned: None,
            exit_code: Some(0),
            output: vec![],
            failures: vec![],
            final_state: None,
            effects: vec![hirsel_interp::observation::ObservedEffect {
                anchor: hirsel_interp::observation::SemanticAnchor {
                    source: hirsel_interp::observation::SourceSite {
                        file: "fixture".into(),
                        definition: hirsel_syntax::DefPath::core("main"),
                        line: 1,
                        column: 1,
                    },
                    role: hirsel_interp::observation::SemanticRole::CapabilityEffect,
                },
                effect: Effect::CapabilityConsumed(hirsel_tir::CapabilityEvent {
                    capability: hirsel_syntax::DefPath::core("Secret"),
                    operation: "read".into(),
                    input: secret.to_vec(),
                    output: secret.to_vec(),
                }),
            }],
        };
        view.enrich("analysis", &observed).unwrap();
        assert_eq!(
            view.document["replay_observer"]["effects"][0]["effect"]["event"]["output"],
            summary
        );
        for output in [view.normalized(), view.logical_perfetto()] {
            assert!(!output.contains(&value));
            assert!(!output.contains(&encoded));
            assert!(output.contains(&summary));
        }
        let malformed = bytes.replace("random_bytes 1", &format!("random_bytes {encoded}"));
        let error = View::parse("fixture", malformed.as_bytes()).err().unwrap();
        assert!(!error.contains(&value));
        assert!(!error.contains(&encoded));
        recording.events.clear();
        let mut journal =
            journal::DurableJournal::create(journal::SimJournalFile::default(), &recording)
                .unwrap();
        for (id, event) in events.iter().enumerate() {
            journal
                .prepare(&journal::Prepared {
                    id: id as u64,
                    task: vec![],
                    issue: id as u64,
                    request: event.request.clone(),
                })
                .unwrap();
            journal.event(Some(id as u64), event).unwrap();
        }
        journal
            .prepare(&journal::Prepared {
                id: events.len() as u64,
                task: vec![],
                issue: events.len() as u64,
                request: Request::Print {
                    stdout: 1,
                    bytes: secret.to_vec(),
                },
            })
            .unwrap();
        journal.barrier().unwrap();
        let pending = View::parse("fixture", journal.file().bytes()).unwrap();
        assert!(
            pending.document["in_doubt"][0]["request"]
                .as_str()
                .unwrap()
                .contains(&summary)
        );
        assert!(!pending.normalized().contains(&encoded));
        journal
            .end(&hirsel_run_schema::RunOutcome::Refused(value.clone()))
            .unwrap();
        let terminal = View::parse("fixture", journal.file().bytes())
            .unwrap()
            .normalized();
        assert!(!terminal.contains(&value));
        assert!(!terminal.contains(&encoded));
        assert!(terminal.contains("refused details=[redacted"));
    }

    #[test]
    fn ordinary_torn_tail_is_cut_but_interior_damage_is_refused() {
        let end = ordinary().len() - hirsel_run_schema::tape::FRAME - 8;
        let prefix = &ordinary()[..end];
        let view = document(prefix);
        assert_eq!(view["complete"], false);
        assert_eq!(view["torn_tail"], false);
        let torn = document(&ordinary()[..end - 3]);
        assert_eq!(torn["torn_tail"], true);
        assert_eq!(torn["events"].as_array().unwrap().len(), 1);
        for replacement in ["broken", "complete 9", "stdout 1 -> nonsense"] {
            let bad = hirsel_rt::tape_binary::text(ordinary())
                .unwrap()
                .replace("stdout 1 -> handle ok 2", replacement);
            assert!(View::parse("fixture", bad.as_bytes()).is_err());
        }
        assert!(View::parse("fixture", b"hirsel-io-tape 7\nrun_identity ").is_err());
        let mut suffix = ordinary().to_vec();
        suffix.extend_from_slice(b"garbage");
        assert!(View::parse("fixture", &suffix).is_err());
    }

    #[test]
    fn durable_torn_trailer_in_doubt_and_interior_checksum() {
        let cut = durable().len() - 5;
        let view = document(&durable()[..cut]);
        assert_eq!(view["complete"], false);
        assert_eq!(view["torn_tail"], true);
        assert_eq!(view["events"].as_array().unwrap().len(), 2);
        let mut corrupt = durable().to_vec();
        // CRC-32C covers the header frame's payload; later valid frames follow it.
        corrupt[journal::MAGIC.len() + 17] ^= 1;
        assert!(
            View::parse("fixture", &corrupt)
                .err()
                .unwrap()
                .contains("corrupt")
        );
        let header = hirsel_run_schema::tape::read(durable(), journal::MAGIC.len(), 0).unwrap();
        let prepared = hirsel_run_schema::tape::read(durable(), header.range.end, 1).unwrap();
        let prefix = &durable()[..prepared.range.end];
        assert_eq!(document(prefix)["in_doubt"][0]["request_id"], 0);
    }

    #[test]
    fn timing_is_bound_sparse_checked_and_atomic() {
        let mut view = View::parse("fixture", ordinary()).unwrap();
        let mut sidecar = json!({"schema": "hirsel-observation-timing", "version": 1,
            "evidence": view.evidence.value(), "clock": "host-monotonic-ns",
            "samples": [{"ordinal": 1, "ns": 200}]});
        view.apply_timing(&sidecar.to_string()).unwrap();
        assert!(view.document["events"][0]["original_time_ns"].is_null());
        assert_eq!(view.document["events"][1]["original_time_ns"], 200);
        let before = view.normalized();
        for samples in [
            json!([{"ordinal": 1, "ns": 100}, {"ordinal": 1, "ns": 200}]),
            json!([{"ordinal": 0, "ns": 200}, {"ordinal": 1, "ns": 100}]),
            json!([{"ordinal": 2, "ns": 200}]),
            json!([{"ordinal": 0, "ns": -1}]),
        ] {
            sidecar["samples"] = samples;
            assert!(view.apply_timing(&sidecar.to_string()).is_err());
            assert_eq!(view.normalized(), before);
        }
        sidecar["samples"] = json!([{"ordinal": 1, "ns": 200}]);
        for (key, value) in [("version", json!(2)), ("clock", json!("replay-duration"))] {
            let mut wrong = sidecar.clone();
            wrong[key] = value;
            assert!(view.apply_timing(&wrong.to_string()).is_err());
            assert_eq!(view.normalized(), before);
        }
        let mut wrong = sidecar.clone();
        wrong["evidence"]["sha256"] = json!("0".repeat(64));
        assert!(view.apply_timing(&wrong.to_string()).is_err());
        assert_eq!(view.normalized(), before);
        sidecar["evidence"]["capture"] = json!("another");
        assert!(view.apply_timing(&sidecar.to_string()).is_err());
    }

    #[test]
    fn timed_perfetto_uses_reference_nanosecond_to_microsecond_vector() {
        // Perfetto c7b436451a48 export_json_unittest.cc:920-951,
        // InstantEventOnThread: 10,000,000 ns -> 10,000 us, no duration.
        let mut view = View::parse("fixture", ordinary()).unwrap();
        assert_eq!(view.perfetto(), view.logical_perfetto());
        let sidecar = json!({"schema": "hirsel-observation-timing", "version": 1,
            "evidence": view.evidence.value(), "clock": "host-monotonic-ns",
            "samples": [{"ordinal": 1, "ns": 10_000_000}]});
        view.apply_timing(&sidecar.to_string()).unwrap();
        let trace: Value = serde_json::from_str(&view.perfetto()).unwrap();
        let logical = &trace["traceEvents"][2];
        let measured = &trace["traceEvents"][3];
        assert_eq!(logical["cat"], "hirsel.logical-order");
        assert_eq!(logical["pid"], 2);
        assert_eq!(measured["pid"], 1);
        assert_eq!(measured["ts"], 10_000.0);
        assert_eq!(measured["ph"], "I");
        assert_eq!(measured["s"], "t");
        assert_eq!(measured["args"]["original_time_ns"], 10_000_000);
        assert!(measured.get("dur").is_none());
        assert_eq!(trace["hirsel"]["time_basis"], "host-monotonic-ns");
    }

    #[test]
    fn perfetto_thread_instant_vector_from_reference() {
        // Adapted from Perfetto c7b436451a48's export_json_unittest.cc,
        // InstantEventOnThread: thread-scoped I, explicit pid/tid and timestamp.
        let view = View::parse("fixture", ordinary()).unwrap();
        let trace: Value = serde_json::from_str(&view.logical_perfetto()).unwrap();
        let e = &trace["traceEvents"][1];
        assert_eq!(e["ph"], "I");
        assert_eq!(e["s"], "t");
        assert_eq!(e["pid"], 1);
        assert_eq!(e["tid"], 1);
        assert_eq!(e["ts"], 0);
        assert_eq!(trace["hirsel"]["time_basis"], "logical-order");
        assert!(e.get("dur").is_none());
    }

    #[test]
    fn replay_enrichment_matches_live_observations_without_repeating_host_writes() {
        hirsel_engine::with_compiler_stack(|| {
            let root_scratch = tempfile::Builder::new()
                .prefix("observation-replay-")
                .tempdir()
                .unwrap();
            let root = root_scratch.path().to_path_buf();
            std::fs::create_dir_all(root.join("host")).unwrap();
            let source = root.join("main.hir");
            let tape = root.join("recording");
            std::fs::write(
                &source,
                r#"
resource type Marker {}
drop(x) {}
pub fn main(rt: Io) -> Result[(), FileError] {
  let marker = Marker()
  let d = io.dir(rt, path.of("work"))?
  file.write_bytes(d, path.of("marker"), b"original")?
  Ok(())
}
"#,
            )
            .unwrap();
            let grants = json!({"io": {"dirs": [{"path": "work",
                "host": root.join("host").to_str().unwrap(), "access": "read_write"}]}})
            .to_string();
            let mut live = None;
            crate::run_path_on_host_observed(
                &source,
                crate::HostLaunch::Live {
                    config: crate::LaunchConfig::Grants {
                        input: std::ffi::OsStr::new(&grants),
                        arguments: None,
                    },
                    record: Some(&tape),
                    revocations: None,
                },
                Some(1_000_000),
                hirsel_engine::Verify::On,
                Some(&mut live),
            )
            .unwrap();
            assert_eq!(
                std::fs::read(root.join("host/marker")).unwrap(),
                b"original"
            );
            // Removing the entire granted mount proves replay does not even admit it.
            std::fs::remove_dir_all(root.join("host")).unwrap();
            let mut view = View::parse("capture", &std::fs::read(&tape).unwrap()).unwrap();
            let replayed = view
                .replay_enrich(&source, &tape, "analysis-1", Some(1_000_000))
                .unwrap();
            assert_eq!(live.as_ref().unwrap(), &replayed);
            assert!(!root.join("host").exists());
            let first = view.normalized();
            view.replay_enrich(&source, &tape, "analysis-1", Some(1_000_000))
                .unwrap();
            assert_eq!(view.normalized(), first);
            assert!(
                !view.document["replay_observer"]["effects"]
                    .as_array()
                    .unwrap()
                    .is_empty()
            );
            assert!(view.document["events"][0]["source"].is_null());
            std::fs::write(&source, "pub fn main(rt: Io) {}\n").unwrap();
            assert!(
                view.replay_enrich(&source, &tape, "analysis-2", Some(1_000_000))
                    .is_err()
            );
            let mut changed = std::fs::read(&tape).unwrap();
            changed.push(b' ');
            std::fs::write(&tape, changed).unwrap();
            assert!(
                view.replay_enrich(&source, &tape, "analysis-2", Some(1_000_000))
                    .unwrap_err()
                    .contains("digest mismatch")
            );
            std::fs::remove_dir_all(root).unwrap();
        });
    }

    #[test]
    fn partial_output_counts_only_the_committed_prefix() {
        let event = Event {
            request: Request::Print {
                stdout: 2,
                bytes: b"hello".to_vec(),
            },
            completion: Completion::Unit(Err(hirsel_rt::io::IoError::Fault(1))),
            fault: Some(hirsel_rt::io::Fault {
                error: hirsel_rt::io::IoError::Fault(1),
                partial: 2,
            }),
            resources: None,
        };
        assert_eq!(accepted_bytes(&event), Some(2));
        assert_eq!(
            accepted_bytes(&Event {
                fault: None,
                ..event
            }),
            Some(0)
        );
    }
}
