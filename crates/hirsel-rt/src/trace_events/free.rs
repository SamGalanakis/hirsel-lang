//! Lossless JSONL for the Free protocol's normalized semantic projection.
use hirsel_free::projection::{Observed, Projection};
use serde_json::{Value, json};

/// Serialize every component of a strictly completed production Free replay or
/// recording. The caller validates the tape and quiescence before publication.
pub fn canonical_events(projection: &Projection) -> Result<Vec<u8>, String> {
    use crate::tape_wire::Wire;
    let outcome = crate::free_replay::Codec::finished_outcome(projection)?
        .ok_or("Free canonical trace has no terminal boundary")?;
    let mut terminal = Vec::new();
    outcome.encode(&mut terminal);
    let mut output = Vec::new();
    let mut row = |mut value: Value| {
        value["schema"] = json!("hirsel.free.events@1");
        serde_json::to_writer(&mut output, &value).expect("JSON values serialize into Vec");
        output.push(b'\n');
    };
    for (object, history) in &projection.objects {
        let object = match object {
            hirsel_free::record::ObjectId::Made { path, op } => {
                json!({"kind":"made","creator":path,"ordinal":op})
            }
            hirsel_free::record::ObjectId::Gate(package) => {
                json!({"kind":"gate","package":package})
            }
        };
        for (version, touch) in &history.versions {
            row(json!({"kind":"object", "object":object, "version":version,
                "task":touch.task,"ordinal":touch.op,"subevent":touch.sub,"outcome":touch.outcome}));
        }
        for touch in &history.receives {
            row(json!({"kind":"receive", "object":object, "task":touch.task,
                "ordinal":touch.op,"subevent":touch.sub,"outcome":touch.outcome}));
        }
    }
    for (key, epochs) in &projection.keys {
        for (epoch, members) in epochs {
            row(json!({"kind":"host_key","key":key,"epoch":epoch,"members":members}));
        }
    }
    for (sink, bytes) in &projection.outputs {
        row(json!({"kind":"output","sink":sink,"bytes":hirsel_run_schema::hex(bytes)}));
    }
    for ((task, op), observed) in &projection.observed {
        for (sub, observation) in observed.iter().enumerate() {
            row(
                json!({"kind":"observed","task":task,"ordinal":op,"subevent":sub,
                "outcome":observed_value(observation)}),
            );
        }
    }
    for (task, progress) in &projection.cut {
        row(json!({"kind":"cut","task":task,"progress":progress.pack()}));
    }
    row(
        json!({"kind":"terminal","outcome":hirsel_run_schema::hex(&terminal),
        "evidence":projection.outcome}),
    );
    Ok(output)
}

fn observed_value(observed: &Observed) -> Value {
    match observed {
        Observed::CpuControl {
            width,
            action,
            count,
            outcome,
        } => json!({
            "kind":"cpu_control","width":width,"action":action,"count":count,"outcome":outcome}),
        Observed::Cancel(cause) => json!({"kind":"canceled","cause":cause}),
        Observed::Expired(value) => json!({"kind":"expired","value":value}),
        Observed::AwaitAny(value) => json!({"kind":"await_any","value":value}),
        Observed::Thaw(value) => json!({"kind":"thaw","value":value}),
        Observed::Load(bits) => json!({"kind":"load","bits":format!("{bits:032x}")}),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tape_wire::Wire;
    use hirsel_free::phase::{Phase, Progress};

    #[test]
    fn canonical_free_trace_keeps_cpu_requests_full_load_bits_and_terminal_outcome() {
        let mut terminal = Vec::new();
        hirsel_run_schema::RunOutcome::Returned(hirsel_run_schema::Returned::Ok)
            .encode(&mut terminal);
        let projection = Projection {
            outcome: format!("native-outcome-v1/{}", hirsel_run_schema::hex(&terminal)),
            cut: [(vec![], Progress::new(2, Phase::Done))].into(),
            observed: [(
                (vec![], 1),
                vec![
                    Observed::CpuControl {
                        width: 8,
                        action: 5,
                        count: 15,
                        outcome: 0,
                    },
                    Observed::Load(u128::MAX),
                ],
            )]
            .into(),
            outputs: [("stdout".into(), b"\0\n\"".to_vec())].into(),
            ..Default::default()
        };
        let bytes = canonical_events(&projection).unwrap();
        let rows: Vec<Value> = std::str::from_utf8(&bytes)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(rows.len(), 5);
        assert_eq!(rows[0]["bytes"], "000a22");
        assert_eq!(
            rows[1]["outcome"],
            json!({"kind":"cpu_control","width":8,"action":5,"count":15,"outcome":0})
        );
        assert_eq!(
            rows[2]["outcome"]["bits"],
            "ffffffffffffffffffffffffffffffff"
        );
        assert_eq!(rows[3]["progress"], Progress::new(2, Phase::Done).pack());
        assert_eq!(rows[4]["outcome"], hirsel_run_schema::hex(&terminal));
        assert_eq!(rows[4]["evidence"], projection.outcome);
        assert!(canonical_events(&Projection::default()).is_err());
        assert!(
            canonical_events(&Projection {
                outcome: "truncated".into(),
                ..projection
            })
            .is_err()
        );
    }
}
