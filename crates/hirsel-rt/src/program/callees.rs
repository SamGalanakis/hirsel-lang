//! Canonical runner admission metadata. Code relocations must name the same
//! entries as RUN-3; diagnostic VALUE identities never grant that authority.
use super::ProgramDescriptor;
use hirsel_run_schema::CalleeFacts;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

/// Descriptor identity for canonical facts and their sorted Entry paths. Code
/// addresses are checked against relocations at load, never hashed as ASLR data.
pub fn identity(program: &[u8; 32], facts: &BTreeMap<String, CalleeFacts>) -> [u8; 32] {
    let mut hash = Sha256::new();
    hash.update(b"hirsel.program-callees.v1\0");
    hash.update(program);
    let bytes = hirsel_run_schema::callees_json(facts);
    hash.update((bytes.len() as u64).to_le_bytes());
    hash.update(bytes.as_bytes());
    hash.finalize().into()
}

/// # Safety
/// All descriptor data is immutable and readable for the program lifetime.
pub(super) unsafe fn load(
    d: &ProgramDescriptor,
    entries: &[(usize, String)],
) -> Result<Option<BTreeMap<String, CalleeFacts>>, String> {
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
    let facts = hirsel_run_schema::parse_callees(text)?;
    if hirsel_run_schema::callees_json(&facts).as_bytes() != bytes {
        return Err("noncanonical compiled callee facts".into());
    }
    if identity(&d.digest, &facts) != d.callee_identity {
        return Err("compiled callee facts identity mismatch".into());
    }
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
    Ok(Some(facts))
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
