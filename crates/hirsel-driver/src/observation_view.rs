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
            sha256: hirsel_run_config::hex(&Sha256::digest(bytes)),
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
                let recovered = journal::recover(bytes).map_err(|e| e.to_string())?;
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
                let outcome = recovered.outcome.as_ref().map(journal::render_outcome);
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
                let valid = bytes
                    .iter()
                    .rposition(|b| *b == b'\n')
                    .map_or(0, |at| at + 1);
                let text =
                    std::str::from_utf8(&bytes[..valid]).map_err(|_| "bad recording UTF-8")?;
                let (recording, complete) = launch::parse_recording_prefix(text)?;
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
                    Request::Ready { .. } | Request::Schedule { .. }
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
        let observed =
            crate::replay_observations(source, tape, max_steps).map_err(|e| e.to_string())?;
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
                Effect::CapabilityConsumed(event) => json!({"kind": "capability", "event": event}),
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
    Ok(text
        .split_once('\n')
        .ok_or("missing tape header")?
        .1
        .trim_end_matches('\n')
        .into())
}
fn request_text(request: &Request) -> Result<String, String> {
    let text = event_text(&Event {
        request: request.clone(),
        completion: Completion::Checked(false),
        fault: None,
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
            C::Blocked { .. } => None,
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
    const ORDINARY: &[u8] = include_bytes!("../tests/fixtures/observation-view/ordinary.tape");
    const DURABLE: &[u8] = include_bytes!("../tests/fixtures/observation-view/durable.journal");
    fn document(bytes: &[u8]) -> Value {
        serde_json::from_str(&View::parse("fixture", bytes).unwrap().normalized()).unwrap()
    }

    #[test]
    fn complete_fixtures_and_golden_views_are_stable() {
        for (name, bytes, normalized, perfetto) in [
            (
                "ordinary",
                ORDINARY,
                include_str!("../tests/fixtures/observation-view/ordinary.view.json"),
                include_str!("../tests/fixtures/observation-view/ordinary.perfetto.json"),
            ),
            (
                "durable",
                DURABLE,
                include_str!("../tests/fixtures/observation-view/durable.view.json"),
                include_str!("../tests/fixtures/observation-view/durable.perfetto.json"),
            ),
        ] {
            let view = View::parse("fixture", bytes).unwrap();
            assert_eq!(view.normalized(), normalized, "{name}");
            assert_eq!(view.logical_perfetto(), perfetto, "{name}");
            assert_eq!(view.document["complete"], true);
            for e in view.document["events"].as_array().unwrap() {
                assert!(e["source"].is_null());
                assert!(e["original_time_ns"].is_null());
            }
        }
        let ordinary = document(ORDINARY);
        assert!(ordinary["events"][0]["durable_anchor"].is_null());
        let durable = document(DURABLE);
        assert_eq!(durable["events"][0]["durable_anchor"]["task"], json!([]));
        assert_eq!(
            durable["events"][1]["durable_anchor"]["task"],
            json!([0, 1])
        );
        assert_eq!(durable["events"][1]["accepted_bytes"], 5);
    }

    #[test]
    fn ordinary_torn_tail_is_cut_but_interior_damage_is_refused() {
        let end = ORDINARY.windows(9).position(|w| w == b"complete ").unwrap();
        let prefix = &ORDINARY[..end];
        let view = document(prefix);
        assert_eq!(view["complete"], false);
        assert_eq!(view["torn_tail"], false);
        let torn = document(&ORDINARY[..end - 3]);
        assert_eq!(torn["torn_tail"], true);
        assert_eq!(torn["events"].as_array().unwrap().len(), 1);
        for replacement in ["broken", "complete 9", "stdout 1 -> nonsense"] {
            let bad = String::from_utf8(ORDINARY.to_vec())
                .unwrap()
                .replace("stdout 1 -> handle ok 2", replacement);
            assert!(View::parse("fixture", bad.as_bytes()).is_err());
        }
        assert!(View::parse("fixture", b"hirsel-io-tape 7\nrun_identity ").is_err());
        let mut suffix = ORDINARY.to_vec();
        suffix.extend_from_slice(b"garbage");
        assert!(View::parse("fixture", &suffix).is_err());
    }

    #[test]
    fn durable_torn_trailer_in_doubt_and_interior_checksum() {
        let cut = DURABLE.len() - 5;
        let view = document(&DURABLE[..cut]);
        assert_eq!(view["complete"], false);
        assert_eq!(view["torn_tail"], true);
        assert_eq!(view["events"].as_array().unwrap().len(), 2);
        let mut corrupt = DURABLE.to_vec();
        // CRC-32C covers the header frame's payload; later valid frames follow it.
        corrupt[journal::MAGIC.len() + 17] ^= 1;
        assert!(
            View::parse("fixture", &corrupt)
                .err()
                .unwrap()
                .contains("corrupt")
        );
        let recovered = journal::recover(DURABLE).unwrap();
        let header_len = 17
            + launch::render_recording(&launch::Recording {
                events: vec![],
                ..recovered.recording.clone()
            })
            .unwrap()
            .len();
        let first_prepared_len = 17 + "0 - 0 stdout 1".len();
        let prefix = &DURABLE[..journal::MAGIC.len() + header_len + first_prepared_len];
        assert_eq!(document(prefix)["in_doubt"][0]["request_id"], 0);
    }

    #[test]
    fn evidence_names_distinguish_identical_captures() {
        assert_ne!(
            EvidenceId::new("one", ORDINARY).unwrap(),
            EvidenceId::new("two", ORDINARY).unwrap()
        );
        assert_ne!(
            EvidenceId::new("one", ORDINARY).unwrap(),
            EvidenceId::new("one", DURABLE).unwrap()
        );
        assert!(EvidenceId::new("", ORDINARY).is_err());
    }

    #[test]
    fn timing_is_bound_sparse_checked_and_atomic() {
        let mut view = View::parse("fixture", ORDINARY).unwrap();
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
        sidecar["evidence"]["capture"] = json!("another");
        assert!(view.apply_timing(&sidecar.to_string()).is_err());
    }

    #[test]
    fn perfetto_thread_instant_vector_from_reference() {
        // Adapted from Perfetto c7b436451a48's export_json_unittest.cc,
        // InstantEventOnThread: thread-scoped I, explicit pid/tid and timestamp.
        let view = View::parse("fixture", ORDINARY).unwrap();
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
            let root =
                std::env::temp_dir().join(format!("observation-replay-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&root);
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
