//! Canonical production request observations. The shared field walk is the
//! established Ordered projection; resource aliases remain separate from the
//! creator operation and initialized result field that name an authority.
use super::*;
use std::collections::BTreeMap;
use std::sync::Mutex;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(in crate::tasks::free) enum Authority {
    RootIo,
    RootEnv,
    Created { path: Vec<u32>, op: u64, field: u32 },
}
impl Wire for Authority {
    fn encode(&self, out: &mut Vec<u8>) {
        match self {
            Self::RootIo => 0u8.encode(out),
            Self::RootEnv => 2u8.encode(out),
            Self::Created { path, op, field } => {
                1u8.encode(out);
                path.encode(out);
                op.encode(out);
                field.encode(out);
            }
        }
    }
    fn decode(input: &mut crate::tape_wire::Decode<'_>) -> Result<Self, String> {
        match u8::decode(input)? {
            0 => Ok(Self::RootIo),
            2 => Ok(Self::RootEnv),
            1 => Ok(Self::Created {
                path: Wire::decode(input)?,
                op: Wire::decode(input)?,
                field: Wire::decode(input)?,
            }),
            _ => Err("invalid Free host authority".into()),
        }
    }
}

#[derive(Default)]
pub(in crate::tasks::free) struct Authorities(Mutex<BTreeMap<u64, Authority>>);
impl Authorities {
    pub(super) fn reference(&self, word: u64) -> Stopped<Authority> {
        self.0
            .lock()
            .unwrap()
            .get(&word)
            .cloned()
            .ok_or_else(|| Stop::Refused("host refusal needs a logical handle identity".into()))
    }
    pub(super) fn resolve(&self, authority: &Authority) -> Stopped<u64> {
        self.0
            .lock()
            .unwrap()
            .iter()
            .find_map(|(&word, known)| (known == authority).then_some(word))
            .ok_or_else(|| Stop::Diverged("unmapped logical host refusal".into()))
    }
    pub(in crate::tasks::free) fn root(&self, env: bool) {
        assert!(
            self.0
                .lock()
                .unwrap()
                .insert(crate::launch::IO_HANDLE, Authority::RootIo)
                .is_none()
        );
        if env {
            assert!(
                self.0
                    .lock()
                    .unwrap()
                    .insert(crate::io::ENV_HANDLE, Authority::RootEnv)
                    .is_none()
            );
        }
    }
    pub(in crate::tasks::free) fn project(&self, request: &Request) -> Stopped<Vec<u8>> {
        let map = self.0.lock().unwrap();
        let value = crate::io_request_observation::project(request, |word| map.get(&word).cloned())
            .map_err(|error| Stop::Diverged(error.into()))?;
        let mut bytes = Vec::new();
        value.encode(&mut bytes);
        Ok(bytes)
    }
    pub(in crate::tasks::free) fn observe(
        &self,
        path: &[u32],
        op: u64,
        result: &Result<Completion, AdapterError>,
    ) -> Stopped<()> {
        if super::handles::refusal_word(result).is_some() {
            return Ok(());
        }
        let mut result = result.clone();
        let fields = handles::fields(&mut result).map_err(|error| Stop::Diverged(error.into()))?;
        let mut map = self.0.lock().unwrap();
        for (field, word) in fields.into_iter().flatten().enumerate() {
            // NetJoin restores a consumed stream's physical word. Every
            // initialized result field gets this creating operation's identity;
            // Ended/Done observations of that same field are idempotent.
            map.insert(
                *word,
                Authority::Created {
                    path: path.to_vec(),
                    op,
                    field: field as u32,
                },
            );
        }
        Ok(())
    }
}
