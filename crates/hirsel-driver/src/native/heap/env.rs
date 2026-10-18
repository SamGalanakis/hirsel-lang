//! Driver-only conversion between H values and the runtime's Env model: the snapshot,
//! the root's names and every narrowed Env's live in hirsel-rt.
use super::{Io, io_ok};
use hirsel_heapcheck::{OpOutcome, OpValue};
use hirsel_rt::io::{Completion as C, ENV_HANDLE, EnvError, Request as R};

fn text(v: &OpValue) -> Option<String> {
    match v {
        OpValue::Bytes(b) => String::from_utf8(b.clone()).ok(),
        _ => None,
    }
}
fn v(tag: u32, value: OpValue) -> OpValue {
    OpValue::Variant {
        tag,
        value: Box::new(value),
    }
}

impl Io {
    /// The runtime model's Env: the root has no row and keeps its own identity.
    fn env_id(&self, value: &OpValue) -> Option<u64> {
        match value {
            OpValue::Handle(ENV_HANDLE) => Some(ENV_HANDLE),
            other => self.network_id(other),
        }
    }
    /// `env.get` and `env.narrow`; `None` for any other operation.
    pub(super) fn env(&mut self, name: &str, args: &[OpValue]) -> Option<OpOutcome> {
        let request = match (name, args) {
            ("get", [env, name]) => R::EnvGet {
                env: self.env_id(env)?,
                name: text(name)?,
            },
            ("narrow", [env, OpValue::Sequence(names)]) => R::EnvNarrow {
                env: self.env_id(env)?,
                names: names.iter().map(text).collect::<Option<_>>()?,
            },
            _ => return None,
        };
        Some(OpOutcome::Value(match self.network.call(request) {
            Ok(C::Env(Ok(Some(value)))) => io_ok(v(1, OpValue::ByteSequence(value.into_bytes()))),
            Ok(C::Env(Ok(None))) => io_ok(v(0, OpValue::Unit)),
            Ok(C::Env(Err(error))) => v(
                1,
                v(
                    match error {
                        EnvError::NotGranted => 0,
                        EnvError::InvalidUtf8 => 1,
                    },
                    OpValue::Unit,
                ),
            ),
            Ok(C::Handle(Ok(id))) => io_ok(self.network_handle(id, false)),
            Ok(C::Handle(Err(_))) => v(1, v(0, OpValue::Unit)),
            _ => return Some(OpOutcome::Fail("CAP-2".into())),
        }))
    }
}
