//! Canonical production request observations. The shared field walk is the
//! established Ordered projection; resource aliases remain separate from the
//! creator operation and initialized result field that name an authority.
use super::*;
use std::collections::BTreeMap;
use std::sync::Mutex;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(in crate::tasks::free) enum Authority {
    RootIo,
    Created { path: Vec<u32>, op: u64, field: u32 },
}
impl Wire for Authority {
    fn encode(&self, out: &mut Vec<u8>) {
        match self {
            Self::RootIo => 0u8.encode(out),
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
    pub(in crate::tasks::free) fn root(&self) {
        assert!(
            self.0
                .lock()
                .unwrap()
                .insert(crate::launch::IO_HANDLE, Authority::RootIo)
                .is_none()
        );
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
        let mut result = result.clone();
        let fields = handles::fields(&mut result).map_err(|error| Stop::Diverged(error.into()))?;
        let mut map = self.0.lock().unwrap();
        for (field, word) in fields.into_iter().flatten().enumerate() {
            map.entry(*word).or_insert_with(|| Authority::Created {
                path: path.to_vec(),
                op,
                field: field as u32,
            });
        }
        Ok(())
    }
}
