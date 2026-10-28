//! Canonical runner admission metadata. Code relocations must name the same
//! entries as RUN-3; diagnostic VALUE identities never grant that authority.
use super::ProgramDescriptor;
use hirsel_run_schema::CalleeFacts;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

/// Compiler-derived SER-2 descriptors for the two request fields whose values
/// the runtime constructs. Entry argument and reply codecs remain generated code.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RequestSchemas {
    grants: Vec<u8>,
    definitions: Vec<u8>,
}
impl RequestSchemas {
    /// Retained buffers of the copied, body-free request schema descriptors.
    pub fn retained_metadata_bytes(&self) -> u64 {
        self.grants
            .capacity()
            .saturating_add(self.definitions.capacity()) as u64
    }

    pub fn new(grants: Vec<u8>, definitions: Vec<u8>) -> Result<Self, String> {
        if [&grants, &definitions]
            .iter()
            .any(|bytes| bytes.is_empty() || bytes.len() > 1 << 20)
        {
            return Err("invalid runner request schema descriptor".into());
        }
        Ok(Self {
            grants,
            definitions,
        })
    }

    /// Frame the existing canonical payloads exactly as SER-2 does. The
    /// compiler obtains each descriptor from the checked registry's codec,
    /// never from a hardcoded copy of the core schema.
    /// See hirsel-types/src/wire.rs, Codec::envelope.
    pub(crate) fn encode(&self, grants: &[u8], definitions: &[u8]) -> (Vec<u8>, Vec<u8>) {
        fn envelope(descriptor: &[u8], payload: &[u8]) -> Vec<u8> {
            let mut bytes = Vec::with_capacity(24 + descriptor.len() + payload.len());
            bytes.extend_from_slice(b"HSC1");
            bytes.extend_from_slice(&1u32.to_le_bytes());
            bytes.extend_from_slice(&(descriptor.len() as u64).to_le_bytes());
            bytes.extend_from_slice(descriptor);
            bytes.extend_from_slice(&(payload.len() as u64).to_le_bytes());
            bytes.extend_from_slice(payload);
            bytes
        }
        (
            envelope(&self.grants, grants),
            envelope(&self.definitions, definitions),
        )
    }
}

/// Canonical metadata in ABI49's existing length-delimited JSON field. Facts-only
/// metadata remains readable; production runner requests additionally need schemas.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Metadata {
    pub facts: BTreeMap<String, CalleeFacts>,
    pub request_schemas: Option<RequestSchemas>,
}
impl Metadata {
    pub fn json(&self) -> String {
        let facts: serde_json::Value =
            serde_json::from_str(&hirsel_run_schema::callees_json(&self.facts)).unwrap();
        match &self.request_schemas {
            Some(schemas) => serde_json::json!({
                "version": 1,
                "facts": facts,
                "request_schemas": {
                    "grants": hirsel_run_schema::hex(&schemas.grants),
                    "definitions": hirsel_run_schema::hex(&schemas.definitions),
                },
            })
            .to_string(),
            None => facts.to_string(),
        }
    }

    fn parse(text: &str) -> Result<Self, String> {
        let value: serde_json::Value = serde_json::from_str(text).map_err(|e| e.to_string())?;
        if value.get("version").is_none() {
            return Ok(Self {
                facts: hirsel_run_schema::parse_callees(text)?,
                request_schemas: None,
            });
        }
        if value.get("version").and_then(serde_json::Value::as_u64) != Some(1) {
            return Err("unsupported compiled callee metadata version".into());
        }
        let facts = hirsel_run_schema::parse_callees(
            &value
                .get("facts")
                .ok_or("missing compiled callee facts")?
                .to_string(),
        )?;
        let descriptor = |name| -> Result<Vec<u8>, String> {
            let hex = value
                .get("request_schemas")
                .and_then(|s| s.get(name))
                .and_then(serde_json::Value::as_str)
                .ok_or("missing runner request schema")?;
            // Canonical re-encoding below rejects uppercase and any extra fields.
            if hex.len() > 2 << 20 || !hex.len().is_multiple_of(2) {
                return Err("invalid runner request schema hex".into());
            }
            hex.as_bytes()
                .chunks_exact(2)
                .map(|pair| {
                    let digit = |b| match b {
                        b'0'..=b'9' => Some(b - b'0'),
                        b'a'..=b'f' => Some(b - b'a' + 10),
                        _ => None,
                    };
                    Ok(
                        digit(pair[0]).ok_or("invalid runner request schema hex")? * 16
                            + digit(pair[1]).ok_or("invalid runner request schema hex")?,
                    )
                })
                .collect()
        };
        let schemas = RequestSchemas::new(descriptor("grants")?, descriptor("definitions")?)?;
        Ok(Self {
            facts,
            request_schemas: Some(schemas),
        })
    }
}

/// Identity of the complete canonical metadata, including request descriptors.
pub fn metadata_identity(program: &[u8; 32], metadata: &Metadata) -> [u8; 32] {
    identity_bytes(program, metadata.json().as_bytes())
}

/// Descriptor identity for canonical facts and their sorted Entry paths. Code
/// addresses are checked against relocations at load, never hashed as ASLR data.
pub fn identity(program: &[u8; 32], facts: &BTreeMap<String, CalleeFacts>) -> [u8; 32] {
    identity_bytes(program, hirsel_run_schema::callees_json(facts).as_bytes())
}
fn identity_bytes(program: &[u8; 32], bytes: &[u8]) -> [u8; 32] {
    let mut hash = Sha256::new();
    hash.update(b"hirsel.program-callees.v1\0");
    hash.update(program);
    hash.update((bytes.len() as u64).to_le_bytes());
    hash.update(bytes);
    hash.finalize().into()
}

/// # Safety
/// All descriptor data is immutable and readable for the program lifetime.
pub(super) unsafe fn load(
    d: &ProgramDescriptor,
    entries: &[(usize, String)],
) -> Result<Option<Metadata>, String> {
    if d.callee_facts_length == 0 {
        if !d.callee_facts.is_null()
            || d.callee_code_count != 0
            || !d.callee_codes.is_null()
            || d.callee_identity != [0; 32]
        {
            return Err("incomplete compiled callee facts".into());
        }
        return Ok(None);
    }
    if d.callee_facts_length > 16 << 20
        || d.callee_facts.is_null()
        || d.callee_code_count > 65_536
        || (d.callee_code_count != 0 && d.callee_codes.is_null())
    {
        return Err("invalid compiled callee facts table".into());
    }
    let bytes =
        unsafe { std::slice::from_raw_parts(d.callee_facts, d.callee_facts_length as usize) };
    let text = std::str::from_utf8(bytes).map_err(|_| "invalid compiled callee facts text")?;
    let metadata = Metadata::parse(text)?;
    if metadata.json().as_bytes() != bytes {
        return Err("noncanonical compiled callee facts".into());
    }
    if identity_bytes(&d.digest, bytes) != d.callee_identity {
        return Err("compiled callee facts identity mismatch".into());
    }
    let facts = &metadata.facts;
    if facts.len() as u64 != d.callee_code_count {
        return Err("compiled callee facts/code count mismatch".into());
    }
    let mut checked = std::collections::BTreeSet::new();
    for index in 0..d.callee_code_count as usize {
        let row = unsafe { &*d.callee_codes.add(index) };
        if row.code.is_null()
            || row.path.is_null()
            || row.path_length == 0
            || row.path_length > 1 << 20
        {
            return Err("invalid compiled callee code reference".into());
        }
        let path = std::str::from_utf8(unsafe {
            std::slice::from_raw_parts(row.path, row.path_length as usize)
        })
        .map_err(|_| "invalid compiled callee code path")?;
        if !facts.contains_key(path)
            || !checked.insert(path)
            || !entries
                .iter()
                .any(|(code, entry)| *code == row.code as usize && entry == path)
        {
            return Err("compiled callee facts do not match the Entry code table".into());
        }
    }
    Ok(Some(metadata))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::program::{EntryRow, Program};

    const PATH: &[u8] = b"worker@1/half";
    static FAILURE: [u8; 8] = [0; 8];
    unsafe extern "C" fn entry(_: *mut crate::TaskCtx, _: *mut u8, _: *mut u8, _: *mut u64) -> u8 {
        crate::STATUS_OK
    }
    fn row(code: usize) -> EntryRow {
        EntryRow {
            code: code as *const _,
            path: PATH.as_ptr(),
            path_length: PATH.len() as u64,
        }
    }
    fn facts() -> BTreeMap<String, CalleeFacts> {
        BTreeMap::from([(
            "worker@1/half".into(),
            CalleeFacts {
                program: [1; 32],
                code: [2; 32],
                fingerprint: [3; 32],
                reach: hirsel_run_schema::CodeReach::Safe,
            },
        )])
    }
    fn descriptor(text: &[u8], codes: &[EntryRow]) -> ProgramDescriptor {
        ProgramDescriptor {
            abi_version: crate::ABI_VERSION,
            flags: 0,
            roots: 0,
            layout_count: 0,
            entry,
            result_size: 0,
            layouts: std::ptr::null(),
            failure_table: FAILURE.as_ptr(),
            failure_table_length: FAILURE.len() as u64,
            entry_name: PATH.as_ptr(),
            entry_name_length: PATH.len() as u64,
            digest: [9; 32],
            config: crate::launch::Config::default(),
            entries: codes.as_ptr(),
            entry_count: codes.len() as u64,
            return_kind: 0,
            success_offset: 0,
            value_codes: std::ptr::null(),
            value_code_count: 0,
            callee_facts: text.as_ptr(),
            callee_facts_length: text.len() as u64,
            callee_codes: codes.as_ptr(),
            callee_code_count: codes.len() as u64,
            callee_identity: identity(&[9; 32], &facts()),
        }
    }
    #[test]
    fn descriptor_callee_facts_bind_every_semantic_field_and_entry_code() {
        let expected = facts();
        let text = hirsel_run_schema::callees_json(&expected);
        let codes = [row(7)];
        let mut d = descriptor(text.as_bytes(), &codes);
        let loaded = unsafe { Program::load(&d) }.unwrap();
        assert_eq!(loaded.runner_callees().unwrap(), &expected);
        for mutation in ["program", "code", "signature", "reach"] {
            let mut forged = expected.clone();
            let f = forged.get_mut("worker@1/half").unwrap();
            match mutation {
                "program" => f.program[0] ^= 1,
                "code" => f.code[0] ^= 1,
                "signature" => f.fingerprint[0] ^= 1,
                "reach" => {
                    f.reach = hirsel_run_schema::CodeReach::Escapes(vec!["unsafe@1/raw".into()])
                }
                _ => unreachable!(),
            }
            let bytes = hirsel_run_schema::callees_json(&forged);
            d.callee_facts = bytes.as_ptr();
            d.callee_facts_length = bytes.len() as u64;
            assert!(
                unsafe { Program::load(&d) }
                    .err()
                    .unwrap()
                    .contains("identity mismatch")
            );
        }
        d = descriptor(text.as_bytes(), &codes);
        let forged_code = [row(8)];
        d.callee_codes = forged_code.as_ptr();
        assert!(
            unsafe { Program::load(&d) }
                .err()
                .unwrap()
                .contains("Entry code table")
        );
        d = descriptor(text.as_bytes(), &codes);
        let mut wrong_path = expected;
        let f = wrong_path.remove("worker@1/half").unwrap();
        wrong_path.insert("other@1/callee".into(), f);
        let bytes = hirsel_run_schema::callees_json(&wrong_path);
        d.callee_facts = bytes.as_ptr();
        d.callee_facts_length = bytes.len() as u64;
        d.callee_identity = identity(&d.digest, &wrong_path);
        assert!(
            unsafe { Program::load(&d) }
                .err()
                .unwrap()
                .contains("Entry code table")
        );
    }

    #[test]
    fn descriptor_request_schemas_are_canonical_and_bound_to_program_identity() {
        let metadata = Metadata {
            facts: facts(),
            request_schemas: Some(RequestSchemas::new(vec![1, 2], vec![3, 4]).unwrap()),
        };
        let text = metadata.json();
        let codes = [row(7)];
        let mut d = descriptor(text.as_bytes(), &codes);
        d.callee_identity = metadata_identity(&d.digest, &metadata);
        let loaded = unsafe { Program::load(&d) }.unwrap();
        assert_eq!(loaded.callees.as_ref(), Some(&metadata));
        for field in ["grants", "definitions"] {
            let mut forged = metadata.clone();
            let schemas = forged.request_schemas.as_mut().unwrap();
            match field {
                "grants" => schemas.grants[0] ^= 1,
                _ => schemas.definitions[0] ^= 1,
            }
            let bytes = forged.json();
            d.callee_facts = bytes.as_ptr();
            d.callee_facts_length = bytes.len() as u64;
            assert!(
                unsafe { Program::load(&d) }
                    .err()
                    .unwrap()
                    .contains("identity mismatch")
            );
        }
        for change in ["extra", "missing", "odd", "uppercase", "version"] {
            let mut value: serde_json::Value = serde_json::from_str(&text).unwrap();
            match change {
                "extra" => value["extra"] = true.into(),
                "missing" => {
                    value["request_schemas"]
                        .as_object_mut()
                        .unwrap()
                        .remove("grants");
                }
                "odd" => value["request_schemas"]["grants"] = "a".into(),
                "uppercase" => value["request_schemas"]["grants"] = "AB".into(),
                _ => value["version"] = 2.into(),
            }
            let bytes = value.to_string();
            d.callee_facts = bytes.as_ptr();
            d.callee_facts_length = bytes.len() as u64;
            assert!(unsafe { Program::load(&d) }.is_err(), "{change}");
        }
    }

    #[test]
    fn missing_callee_facts_and_old_abi_cannot_supply_runner_authority() {
        let mut d = descriptor(b"", &[]);
        d.callee_facts = std::ptr::null();
        d.callee_codes = std::ptr::null();
        d.callee_identity = [0; 32];
        let program = unsafe { Program::load(&d) }.unwrap();
        assert!(
            program
                .runner_callees()
                .unwrap_err()
                .contains("require compiled callee facts")
        );
        // Only an old ABI prefix is readable: reject it before accessing any
        // newly appended descriptor fields or even constructing their view.
        let old = crate::ABI_VERSION - 1;
        assert!(
            unsafe { Program::load((&raw const old).cast()) }
                .err()
                .unwrap()
                .contains("runtime ABI")
        );
        d.callee_code_count = 1;
        assert!(
            unsafe { Program::load(&d) }
                .err()
                .unwrap()
                .contains("incomplete")
        );
    }
}
