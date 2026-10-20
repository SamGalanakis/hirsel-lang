//! The frames a caller's `remote.tls@1` provider and a runner host exchange (RUN-13): the
//! core `RunFrame` type as Rust data, and its canonical codec form. A frame on the wire is
//! a little-endian `u64` length followed by one SER-2 envelope of `RunFrame`, so every byte
//! of it is checked by the same codec that checks a program's own data.
use crate::data::Data;
use hirsel_run_schema::{
    Access, CallForm, CallOutcome, GrantSet, NET_OPERATIONS, NetRule, ProcessRule,
};

/// A call's reference on its stream: the session epoch, the stream position and the call
/// key (core `CallRef`).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CallRef {
    pub epoch: u64,
    pub position: u64,
    pub key: [u8; 32],
}

/// A request (core `RemoteCall`): its stream position, the caller's request ordinal and
/// call key, the callee's identity, the requested grant set and allowlist, the argument
/// bytes and the highest position whose outcome the caller holds with every lower one.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Call {
    pub epoch: u64,
    pub position: u64,
    pub ordinal: u64,
    pub key: [u8; 32],
    pub form: CallForm,
    pub path: String,
    pub fingerprint: [u8; 32],
    pub code: [u8; 32],
    pub grants: GrantSet,
    pub definitions: Option<Vec<String>>,
    pub argument: Vec<u8>,
    pub acknowledged: u64,
}
impl Call {
    pub fn reference(&self) -> CallRef {
        CallRef {
            epoch: self.epoch,
            position: self.position,
            key: self.key,
        }
    }
}

/// A decided position's outcome (core `RemoteOutcome`): the request digest it answers,
/// how the call ended, the callee run's instance when the callee ran, and the definition
/// hashes the place lacks for `NotRun(VersionMismatch())`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Outcome {
    pub epoch: u64,
    pub position: u64,
    pub key: [u8; 32],
    pub digest: [u8; 32],
    pub end: CallOutcome,
    pub callee: Option<[u8; 16]>,
    pub missing: Vec<[u8; 32]>,
}

/// One frame (core `RunFrame`, variants in ordinal order).
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Frame {
    /// The caller opens a session of its stream at `epoch`.
    Open {
        stream: [u8; 32],
        instance: [u8; 16],
        epoch: u64,
    },
    /// The host accepted the session: its incarnation, the stream's highest decided position,
    /// and whether it records call keys durably before admitting them.
    Opened {
        epoch: u64,
        incarnation: [u8; 16],
        applied: u64,
        reconciles: bool,
    },
    /// The host refused the session with a `NotRunReason` ordinal.
    Refused(u32),
    Call(Box<Call>),
    Outcome(Box<Outcome>),
    /// Cancel a sent call, with a `CancelCause` ordinal.
    Cancel(CallRef, u32),
    /// The host received a cancellation of a call whose callee had started.
    Acknowledged(CallRef),
    /// Ask how a sent call stands: the host answers with its outcome, `Running` or `Unknown`.
    Status(CallRef),
    Running(CallRef),
    /// The host never decided this call key.
    Unknown(CallRef),
    /// The caller ends its session, holding every outcome up to `acknowledged`.
    Close {
        epoch: u64,
        acknowledged: u64,
    },
}
impl Frame {
    /// The session epoch the frame belongs to; `Refused` belongs to none.
    pub fn epoch(&self) -> Option<u64> {
        Some(match self {
            Self::Open { epoch, .. } | Self::Opened { epoch, .. } | Self::Close { epoch, .. } => {
                *epoch
            }
            Self::Refused(_) => return None,
            Self::Call(call) => call.epoch,
            Self::Outcome(outcome) => outcome.epoch,
            Self::Cancel(r, _)
            | Self::Acknowledged(r)
            | Self::Status(r)
            | Self::Running(r)
            | Self::Unknown(r) => r.epoch,
        })
    }
}

/// A frame whose data does not fit `RunFrame`'s invariants (lengths of fixed hashes,
/// variant ordinals).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Malformed(pub &'static str);

fn u(n: u64) -> Data {
    Data::Unsigned(n.into())
}
fn bytes(b: &[u8]) -> Data {
    Data::Bytes(b.to_vec())
}
fn unit(ordinal: u32) -> Data {
    Data::Variant(ordinal, vec![])
}
fn option(value: Option<Data>) -> Data {
    match value {
        None => Data::Variant(0, vec![]),
        Some(value) => Data::Variant(1, vec![value]),
    }
}
fn texts(list: &[String]) -> Data {
    Data::List(list.iter().cloned().map(Data::Text).collect())
}

/// `GrantSet` as data.
pub fn grant_set_data(set: &GrantSet) -> Data {
    Data::Record(vec![
        Data::Bool(set.stdout),
        Data::Bool(set.stderr),
        Data::Bool(set.tasks),
        Data::Bool(set.monotonic_clock),
        Data::Bool(set.wall_clock),
        Data::Bool(set.random),
        Data::List(
            set.dirs
                .iter()
                .map(|(path, access)| {
                    Data::Record(vec![
                        Data::Text(path.clone()),
                        unit(u32::from(*access == Access::ReadWrite)),
                    ])
                })
                .collect(),
        ),
        Data::List(
            set.net
                .iter()
                .map(|rule| {
                    let operation = NET_OPERATIONS
                        .iter()
                        .position(|o| *o == rule.operation)
                        .unwrap_or(0) as u32;
                    Data::Record(vec![
                        Data::Text(rule.address.clone()),
                        Data::Unsigned(rule.service.parse::<u16>().unwrap_or(0).into()),
                        unit(operation),
                    ])
                })
                .collect(),
        ),
        Data::List(
            set.process
                .iter()
                .map(|rule| Data::Record(vec![Data::Text(rule.program.clone()), texts(&rule.env)]))
                .collect(),
        ),
    ])
}
/// An allowlist as data: `Option[List[String]]`.
pub fn allowlist_data(definitions: Option<&[String]>) -> Data {
    option(definitions.map(texts))
}

fn end_data(end: &CallOutcome) -> Data {
    match end {
        CallOutcome::Ok(b) => Data::Variant(0, vec![bytes(b)]),
        CallOutcome::Failed(b) => Data::Variant(1, vec![bytes(b)]),
        CallOutcome::Faulted(b) => Data::Variant(2, vec![bytes(b)]),
        CallOutcome::NotRun(r) => Data::Variant(3, vec![unit(*r)]),
        CallOutcome::InDoubt(r) => Data::Variant(4, vec![unit(*r)]),
        CallOutcome::Canceled(c) => Data::Variant(5, vec![unit(*c)]),
    }
}
fn reference_data(r: &CallRef) -> Data {
    Data::Record(vec![u(r.epoch), u(r.position), bytes(&r.key)])
}

impl Frame {
    /// The frame as `RunFrame` data.
    pub fn data(&self) -> Data {
        match self {
            Self::Open {
                stream,
                instance,
                epoch,
            } => Data::Variant(
                0,
                vec![Data::Record(vec![
                    bytes(stream),
                    bytes(instance),
                    u(*epoch),
                ])],
            ),
            Self::Opened {
                epoch,
                incarnation,
                applied,
                reconciles,
            } => Data::Variant(
                1,
                vec![Data::Record(vec![
                    u(*epoch),
                    bytes(incarnation),
                    u(*applied),
                    Data::Bool(*reconciles),
                ])],
            ),
            Self::Refused(reason) => Data::Variant(2, vec![unit(*reason)]),
            Self::Call(call) => Data::Variant(
                3,
                vec![Data::Record(vec![
                    u(call.epoch),
                    u(call.position),
                    u(call.ordinal),
                    bytes(&call.key),
                    unit(match call.form {
                        CallForm::Compute => 0,
                        CallForm::Call => 1,
                    }),
                    Data::Text(call.path.clone()),
                    bytes(&call.fingerprint),
                    bytes(&call.code),
                    grant_set_data(&call.grants),
                    allowlist_data(call.definitions.as_deref()),
                    bytes(&call.argument),
                    u(call.acknowledged),
                ])],
            ),
            Self::Outcome(outcome) => Data::Variant(
                4,
                vec![Data::Record(vec![
                    u(outcome.epoch),
                    u(outcome.position),
                    bytes(&outcome.key),
                    bytes(&outcome.digest),
                    end_data(&outcome.end),
                    option(outcome.callee.map(|c| bytes(&c))),
                    Data::List(outcome.missing.iter().map(|h| bytes(h)).collect()),
                ])],
            ),
            Self::Cancel(r, cause) => Data::Variant(5, vec![reference_data(r), unit(*cause)]),
            Self::Acknowledged(r) => Data::Variant(6, vec![reference_data(r)]),
            Self::Status(r) => Data::Variant(7, vec![reference_data(r)]),
            Self::Running(r) => Data::Variant(8, vec![reference_data(r)]),
            Self::Unknown(r) => Data::Variant(9, vec![reference_data(r)]),
            Self::Close {
                epoch,
                acknowledged,
            } => Data::Variant(10, vec![u(*epoch), u(*acknowledged)]),
        }
    }

    /// The frame `RunFrame` data holds.
    pub fn from_data(data: Data) -> Result<Self, Malformed> {
        let Data::Variant(ordinal, fields) = data else {
            return Err(Malformed("RunFrame"));
        };
        let mut fields = Reader(fields.into_iter());
        let frame = match ordinal {
            0 => {
                let mut r = fields.record()?;
                Self::Open {
                    stream: r.fixed()?,
                    instance: r.fixed()?,
                    epoch: r.u64()?,
                }
            }
            1 => {
                let mut r = fields.record()?;
                Self::Opened {
                    epoch: r.u64()?,
                    incarnation: r.fixed()?,
                    applied: r.u64()?,
                    reconciles: r.bool()?,
                }
            }
            2 => Self::Refused(fields.unit()?),
            3 => {
                let mut r = fields.record()?;
                Self::Call(Box::new(Call {
                    epoch: r.u64()?,
                    position: r.u64()?,
                    ordinal: r.u64()?,
                    key: r.fixed()?,
                    form: match r.unit()? {
                        0 => CallForm::Compute,
                        1 => CallForm::Call,
                        _ => return Err(Malformed("CallForm")),
                    },
                    path: r.text()?,
                    fingerprint: r.fixed()?,
                    code: r.fixed()?,
                    grants: grant_set(r.next()?)?,
                    definitions: match r.option()? {
                        None => None,
                        Some(list) => Some(text_list(list)?),
                    },
                    argument: r.bytes()?,
                    acknowledged: r.u64()?,
                }))
            }
            4 => {
                let mut r = fields.record()?;
                Self::Outcome(Box::new(Outcome {
                    epoch: r.u64()?,
                    position: r.u64()?,
                    key: r.fixed()?,
                    digest: r.fixed()?,
                    end: end(r.next()?)?,
                    callee: match r.option()? {
                        None => None,
                        Some(Data::Bytes(b)) => {
                            Some(b.try_into().map_err(|_| Malformed("callee"))?)
                        }
                        Some(_) => return Err(Malformed("callee")),
                    },
                    missing: match r.next()? {
                        Data::List(hashes) => hashes
                            .into_iter()
                            .map(|h| match h {
                                Data::Bytes(b) => b.try_into().map_err(|_| Malformed("hash")),
                                _ => Err(Malformed("hash")),
                            })
                            .collect::<Result<_, _>>()?,
                        _ => return Err(Malformed("missing")),
                    },
                }))
            }
            5 => Self::Cancel(fields.reference()?, fields.unit()?),
            6 => Self::Acknowledged(fields.reference()?),
            7 => Self::Status(fields.reference()?),
            8 => Self::Running(fields.reference()?),
            9 => Self::Unknown(fields.reference()?),
            10 => Self::Close {
                epoch: fields.u64()?,
                acknowledged: fields.u64()?,
            },
            _ => return Err(Malformed("RunFrame ordinal")),
        };
        fields.done()?;
        Ok(frame)
    }
}

/// Reads one composite's fields in order.
struct Reader(std::vec::IntoIter<Data>);
impl Reader {
    fn next(&mut self) -> Result<Data, Malformed> {
        self.0.next().ok_or(Malformed("too few fields"))
    }
    fn done(mut self) -> Result<(), Malformed> {
        match self.0.next() {
            None => Ok(()),
            Some(_) => Err(Malformed("too many fields")),
        }
    }
    fn record(&mut self) -> Result<Reader, Malformed> {
        match self.next()? {
            Data::Record(fields) => Ok(Reader(fields.into_iter())),
            _ => Err(Malformed("record")),
        }
    }
    fn u64(&mut self) -> Result<u64, Malformed> {
        match self.next()? {
            Data::Unsigned(n) => u64::try_from(n).map_err(|_| Malformed("u64")),
            _ => Err(Malformed("u64")),
        }
    }
    fn bool(&mut self) -> Result<bool, Malformed> {
        match self.next()? {
            Data::Bool(b) => Ok(b),
            _ => Err(Malformed("bool")),
        }
    }
    fn text(&mut self) -> Result<String, Malformed> {
        match self.next()? {
            Data::Text(t) => Ok(t),
            _ => Err(Malformed("String")),
        }
    }
    fn bytes(&mut self) -> Result<Vec<u8>, Malformed> {
        match self.next()? {
            Data::Bytes(b) => Ok(b),
            _ => Err(Malformed("Bytes")),
        }
    }
    fn fixed<const N: usize>(&mut self) -> Result<[u8; N], Malformed> {
        self.bytes()?
            .try_into()
            .map_err(|_| Malformed("a fixed-length hash"))
    }
    fn unit(&mut self) -> Result<u32, Malformed> {
        match self.next()? {
            Data::Variant(ordinal, fields) if fields.is_empty() => Ok(ordinal),
            _ => Err(Malformed("a payload-free variant")),
        }
    }
    fn option(&mut self) -> Result<Option<Data>, Malformed> {
        match self.next()? {
            Data::Variant(0, fields) if fields.is_empty() => Ok(None),
            Data::Variant(1, mut fields) if fields.len() == 1 => Ok(fields.pop()),
            _ => Err(Malformed("Option")),
        }
    }
    fn reference(&mut self) -> Result<CallRef, Malformed> {
        let mut r = self.record()?;
        let reference = CallRef {
            epoch: r.u64()?,
            position: r.u64()?,
            key: r.fixed()?,
        };
        r.done()?;
        Ok(reference)
    }
}

fn text_list(data: Data) -> Result<Vec<String>, Malformed> {
    match data {
        Data::List(items) => items
            .into_iter()
            .map(|item| match item {
                Data::Text(t) => Ok(t),
                _ => Err(Malformed("String")),
            })
            .collect(),
        _ => Err(Malformed("List")),
    }
}

fn end(data: Data) -> Result<CallOutcome, Malformed> {
    let Data::Variant(ordinal, fields) = data else {
        return Err(Malformed("CallEnd"));
    };
    let mut r = Reader(fields.into_iter());
    let end = match ordinal {
        0 => CallOutcome::Ok(r.bytes()?),
        1 => CallOutcome::Failed(r.bytes()?),
        2 => CallOutcome::Faulted(r.bytes()?),
        3 => CallOutcome::NotRun(r.unit()?),
        4 => CallOutcome::InDoubt(r.unit()?),
        5 => CallOutcome::Canceled(r.unit()?),
        _ => return Err(Malformed("CallEnd ordinal")),
    };
    r.done()?;
    Ok(end)
}

/// The plain grant set `GrantSet` data holds.
pub fn grant_set(data: Data) -> Result<GrantSet, Malformed> {
    let Data::Record(fields) = data else {
        return Err(Malformed("GrantSet"));
    };
    let mut r = Reader(fields.into_iter());
    let mut set = GrantSet {
        stdout: r.bool()?,
        stderr: r.bool()?,
        tasks: r.bool()?,
        monotonic_clock: r.bool()?,
        wall_clock: r.bool()?,
        random: r.bool()?,
        ..GrantSet::default()
    };
    let list = |data: Data| match data {
        Data::List(items) => Ok(items),
        _ => Err(Malformed("List")),
    };
    for dir in list(r.next()?)? {
        let Data::Record(fields) = dir else {
            return Err(Malformed("DirGrant"));
        };
        let mut d = Reader(fields.into_iter());
        let path = d.text()?;
        let access = match d.unit()? {
            0 => Access::Read,
            1 => Access::ReadWrite,
            _ => return Err(Malformed("GrantAccess")),
        };
        d.done()?;
        set.dirs.push((path, access));
    }
    for rule in list(r.next()?)? {
        let Data::Record(fields) = rule else {
            return Err(Malformed("NetRule"));
        };
        let mut n = Reader(fields.into_iter());
        let address = n.text()?;
        let port = match n.next()? {
            Data::Unsigned(p) => u16::try_from(p).map_err(|_| Malformed("port"))?,
            _ => return Err(Malformed("port")),
        };
        let operation = *NET_OPERATIONS
            .get(n.unit()? as usize)
            .ok_or(Malformed("NetOperation"))?;
        n.done()?;
        set.net.push(NetRule {
            address,
            service: port.to_string(),
            operation: operation.into(),
        });
    }
    for rule in list(r.next()?)? {
        let Data::Record(fields) = rule else {
            return Err(Malformed("ProcessRule"));
        };
        let mut p = Reader(fields.into_iter());
        let program = p.text()?;
        let env = text_list(p.next()?)?;
        p.done()?;
        set.process.push(ProcessRule { program, env });
    }
    r.done()?;
    Ok(set)
}
