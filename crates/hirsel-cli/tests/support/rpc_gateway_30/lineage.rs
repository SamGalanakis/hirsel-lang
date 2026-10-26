//! Pure RPC consumer of admitted observations, not a producer or tape decoder.
//!
//! The caller supplies the independently admitted program, exact declaration
//! catalogue and canonical RPC identity/timing projection. No payload, counter,
//! address, stdout, or ordinal-to-clock conversion can fill a missing mapping.
use hirsel_run_schema::Site;
use hirsel_run_schema::ownership_trace::{Action, Owner, Projection, TaskPath, Token, Trace};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

const EVENT_LIMIT: usize = 1 << 20;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub enum Role {
    Request,
    Upstream,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Kind {
    Create,
    Move,
    Drop,
}

/// Exact declarations, supplied by source admission, never discovered by a
/// Credit payload or inferred from a matching prefix of an aggregate path.
#[derive(Clone, Debug)]
pub struct CreditSite {
    pub role: Role,
    pub value_type: String,
    pub kind: Kind,
    pub site: Site,
    pub path: Vec<Projection>,
    pub from_path: Option<Vec<Projection>>,
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct Location {
    pub task: TaskPath,
    pub operation: u64,
    pub subordinal: u32,
}

#[derive(Clone, Debug)]
pub struct CreditJoin {
    pub role: Role,
    pub token: Token,
    pub owner: Owner,
    pub from: Option<Owner>,
}

/// An admitted canonical RPC projection. `None` means the decoder has not
/// established that identity or timing mapping; it is never synthesized here.
/// `move_credit` is diagnostic only and is not an admission-checker operation.
#[derive(Clone, Debug)]
pub struct Observation {
    pub location: Location,
    pub transition: Option<Value>,
    pub time_ms: Option<u64>,
    pub credit: Option<CreditJoin>,
}

#[derive(Clone, Debug)]
pub struct RpcObservations {
    pub program: [u8; 32],
    pub complete: bool,
    pub event_count: usize,
    pub events: Vec<Observation>,
}

fn kind(action: &Action) -> Kind {
    match action {
        Action::Create { .. } => Kind::Create,
        Action::Move { .. } => Kind::Move,
        Action::Drop { .. } => Kind::Drop,
    }
}

fn result(events: Vec<Value>, witnessed: Vec<Value>, missing: BTreeSet<String>) -> Value {
    let status = if missing.is_empty() {
        "witnessed"
    } else {
        "unproven"
    };
    json!({
        "complete": missing.is_empty(), "events": events, "witnessed": witnessed,
        "missing": missing, "coverage": {
            "admission": status, "request_credits": status, "upstream_credits": status
        }
    })
}

/// Both streams must bind to `program`, obtained independently by admission.
/// A consistent forged pair is not authentication of its producer. Completeness
/// here qualifies the join only; the seventeen-invariant checker still decides
/// admission. Partial supported transitions remain visible, never a partial pass.
pub fn consume(
    trace: &Trace,
    rpc: Option<&RpcObservations>,
    declarations: &[CreditSite],
    program: [u8; 32],
) -> Value {
    let mut missing = BTreeSet::new();
    if let Err(error) = trace.analyze(program, EVENT_LIMIT) {
        missing.insert(format!("ownership: {:?}", error.problem));
        return result(vec![], vec![], missing);
    }
    let Some(rpc) = rpc else {
        missing.insert("RPC identity/timing stream missing".into());
        return result(vec![], vec![], missing);
    };
    if rpc.program != program
        || !rpc.complete
        || rpc.event_count != rpc.events.len()
        || rpc.events.len() > EVENT_LIMIT
    {
        missing.insert("RPC program/completeness/count".into());
        return result(vec![], vec![], missing);
    }
    if declarations.is_empty() {
        missing.insert("declared credit sites missing".into());
    }
    let mut types = BTreeMap::new();
    let mut locations = BTreeMap::new();
    for (index, event) in trace.events.iter().enumerate() {
        if let Action::Create {
            token, value_type, ..
        } = &event.action
        {
            types.insert(token, value_type);
        }
        locations.insert(
            Location {
                task: event.task.clone(),
                operation: event.operation,
                subordinal: event.subordinal,
            },
            (index, event),
        );
    }
    let mut joined = BTreeSet::new();
    let mut assigned = BTreeMap::new();
    let mut held = BTreeMap::new();
    let mut last_credit = None;
    let mut last_rpc = BTreeMap::new();
    let mut events = vec![];
    let mut witnessed = vec![];
    for (index, observation) in rpc.events.iter().enumerate() {
        let label = format!("RPC observation {index}");
        let mut gaps = BTreeSet::new();
        let location = &observation.location;
        let ordinal = (location.operation, location.subordinal);
        if last_rpc
            .insert(location.task.clone(), ordinal)
            .is_some_and(|last| last >= ordinal)
        {
            gaps.insert(format!("{label}: duplicate/reordered location"));
        }
        let Some(transition) = &observation.transition else {
            missing.insert(format!("{label}: identity mapping missing"));
            continue;
        };
        let op = transition["op"].as_str().unwrap_or("");
        let expected = match op {
            "reserve" => Some((Role::Request, Kind::Create)),
            "acquire" => Some((Role::Upstream, Kind::Create)),
            "release_request" => Some((Role::Request, Kind::Drop)),
            "release_slot" => Some((Role::Upstream, Kind::Drop)),
            "move_credit" => observation.credit.as_ref().map(|c| (c.role, Kind::Move)),
            _ => None,
        };
        if !matches!(
            op,
            "reserve"
                | "acquire"
                | "release_request"
                | "release_slot"
                | "move_credit"
                | "busy"
                | "disconnect"
                | "join"
                | "admit"
                | "enter"
                | "begin"
                | "issue"
                | "connected"
                | "result"
                | "backoff"
                | "wake"
                | "cancel"
                | "terminal"
        ) || !transition.is_object()
            || transition.get("t").is_some()
        {
            gaps.insert(format!("{label}: unsupported projection"));
        }
        if op != "move_credit" && observation.time_ms.is_none() {
            gaps.insert(format!("{label}: timing mapping missing"));
        }
        if let Some((role, action_kind)) = expected {
            let event = locations.get(location);
            match (event, &observation.credit) {
                (Some((trace_index, event)), Some(credit)) => {
                    let from = match &event.action {
                        Action::Move { from, .. } => Some(from),
                        _ => None,
                    };
                    let matches: Vec<_> = declarations
                        .iter()
                        .filter(|site| {
                            site.role == role
                                && site.kind == action_kind
                                && site.site == event.site
                                && site.path == event.action.owner().path
                                && site.from_path.as_ref() == from.map(|o| &o.path)
                                && types
                                    .get(event.action.token())
                                    .is_some_and(|t| **t == site.value_type)
                        })
                        .collect();
                    if credit.role != role
                        || event.action.token() != &credit.token
                        || event.action.owner() != &credit.owner
                        || from != credit.from.as_ref()
                        || kind(&event.action) != action_kind
                        || matches.len() != 1
                        || last_credit.is_some_and(|last| last >= *trace_index)
                        || !joined.insert(*trace_index)
                    {
                        gaps.insert(format!("{label}: exact credit join mismatch"));
                    } else {
                        last_credit = Some(*trace_index);
                        let identity = transition["c"]
                            .as_u64()
                            .zip(transition["r"].as_u64())
                            .map(|(c, r)| (c, r, role));
                        if let Some(identity) = identity {
                            let valid = match action_kind {
                                Kind::Create => {
                                    assigned.insert(credit.token.clone(), identity).is_none()
                                        && held.insert(identity, credit.token.clone()).is_none()
                                }
                                Kind::Move => {
                                    assigned.get(&credit.token) == Some(&identity)
                                        && held.get(&identity) == Some(&credit.token)
                                }
                                Kind::Drop => {
                                    assigned.get(&credit.token) == Some(&identity)
                                        && held.remove(&identity).as_ref() == Some(&credit.token)
                                }
                            };
                            if !valid {
                                gaps.insert(format!("{label}: credit request/role changed"));
                            }
                        } else {
                            gaps.insert(format!("{label}: credit RPC identity missing"));
                        }
                        if gaps.is_empty() {
                            witnessed.push(json!({"transition": transition, "ownership": event}));
                        }
                    }
                }
                _ => {
                    gaps.insert(format!("{label}: credit ownership mapping missing"));
                }
            }
        } else if observation.credit.is_some() || op == "move_credit" {
            gaps.insert(format!("{label}: unexpected credit mapping"));
        }
        if gaps.is_empty() && op != "move_credit" {
            let mut event = transition.clone();
            event["t"] = json!(observation.time_ms);
            events.push(event);
        }
        missing.extend(gaps);
    }
    for (index, event) in trace.events.iter().enumerate() {
        if types.get(event.action.token()).is_some_and(|value_type| {
            declarations
                .iter()
                .any(|site| site.value_type == **value_type)
        }) && !joined.contains(&index)
        {
            missing.insert(format!(
                "ownership event {index}: RPC credit mapping missing"
            ));
        }
    }
    if !held.is_empty() {
        missing.insert("credit drop missing".into());
    }
    result(events, witnessed, missing)
}
