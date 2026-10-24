//! LLVM's final .stack_sizes records from the exact x86-64 ELF object.
//! LLVM 23 AsmPrinter.cpp:1698-1722 omits records for dynamic frames.
use super::*;
use std::collections::BTreeMap;

/// Final caller-SP extents, including the x86 call's pushed return address.
/// A missing function record is missing evidence, not a zero-sized frame.
pub fn llvm_stack_sizes(bytes: &[u8]) -> Result<BTreeMap<String, u64>, String> {
    read(bytes).map_err(str::to_owned)
}

fn read(bytes: &[u8]) -> Result<BTreeMap<String, u64>, &'static str> {
    ObjectFile::read(&TargetSpec::x86_64_unknown_linux_gnu(), bytes.to_vec())
        .map_err(|_| "invalid x86-64 ELF object")?;
    let base = offset_at(bytes, 40)?;
    let count = usize::from(u16_at(bytes, 60)?);
    let header = |i: usize| -> Result<usize, &'static str> {
        if i >= count {
            return Err("missing ELF section");
        }
        base.checked_add(
            i.checked_mul(SECTION_HEADER_SIZE)
                .ok_or("section index overflow")?,
        )
        .ok_or("section offset overflow")
    };
    let contents = |i: usize| -> Result<&[u8], &'static str> {
        let h = header(i)?;
        range(bytes, offset_at(bytes, h + 24)?, offset_at(bytes, h + 32)?)
    };
    let strings = contents(usize::from(u16_at(bytes, 62)?))?;
    let mut stack_sections = Vec::new();
    let mut symbols_index = None;
    for i in 0..count {
        let h = header(i)?;
        let name = name_at(strings, u32_at(bytes, h)? as usize)?;
        if name == ".stack_sizes" {
            stack_sections.push(i);
        }
        if u32_at(bytes, h + 4)? == SHT_SYMTAB {
            if symbols_index.replace(i).is_some() {
                return Err("duplicate symbol table");
            }
        }
    }
    if stack_sections.is_empty() {
        // Absence is missing machine evidence: emit only null-entry metadata.
        // Malformed records and relocations below remain object/cache errors.
        return Ok(BTreeMap::new());
    }
    let symbols_index = symbols_index.ok_or("missing symbol table")?;
    let h = header(symbols_index)?;
    let symbol_names = contents(u32_at(bytes, h + 40)? as usize)?;
    let raw_symbols = contents(symbols_index)?;
    let mut symbols = Vec::new();
    let mut functions = BTreeMap::new();
    for symbol in raw_symbols.as_chunks::<SYMBOL_SIZE>().0 {
        let name = name_at(symbol_names, u32_at(symbol, 0)? as usize)?;
        let section = usize::from(u16_at(symbol, 6)?);
        let offset = read_bytes(symbol, 8).map(u64::from_le_bytes)?;
        if symbol[4] & 0xf == 2 && section != 0 {
            if name.is_empty() || functions.insert((section, offset), name).is_some() {
                return Err("ambiguous function address");
            }
        }
        symbols.push((section, offset));
    }
    let mut result = BTreeMap::new();
    for section in stack_sections {
        let data = contents(section)?;
        let mut relocations = BTreeMap::new();
        for i in 0..count {
            let h = header(i)?;
            if u32_at(bytes, h + 4)? != SHT_RELA || u32_at(bytes, h + 44)? as usize != section {
                continue;
            }
            if u32_at(bytes, h + 40)? as usize != symbols_index
                || offset_at(bytes, h + 56)? != RELA_SIZE
            {
                return Err("invalid stack relocation table");
            }
            let entries = contents(i)?;
            if entries.len() % RELA_SIZE != 0 {
                return Err("truncated stack relocation");
            }
            for r in entries.as_chunks::<RELA_SIZE>().0 {
                let at = offset_at(r, 0)?;
                let info = read_bytes(r, 8).map(u64::from_le_bytes)?;
                if info as u32 != 1 {
                    return Err("non-absolute stack address relocation");
                }
                let (target, offset) = *symbols
                    .get((info >> 32) as usize)
                    .ok_or("unknown stack relocation symbol")?;
                let addend = read_bytes(r, 16).map(i64::from_le_bytes)?;
                let address = offset
                    .checked_add_signed(addend)
                    .ok_or("stack symbol address overflow")?;
                let name = functions
                    .get(&(target, address))
                    .ok_or("stack relocation does not name a function")?;
                if relocations.insert(at, name.clone()).is_some() {
                    return Err("duplicate stack address relocation");
                }
            }
        }
        let mut cursor = 0;
        while cursor < data.len() {
            let name = relocations
                .remove(&cursor)
                .ok_or("missing stack address relocation")?;
            let raw = range(data, cursor, 8)?;
            if raw != [0; 8] {
                return Err("nonzero relocated stack address");
            }
            cursor += 8;
            let frame = uleb(data, &mut cursor)?
                .checked_add(8)
                .ok_or("stack extent overflow")?;
            if result.insert(name, frame).is_some() {
                return Err("duplicate stack size record");
            }
        }
        if !relocations.is_empty() {
            return Err("stack relocation outside an entry");
        }
    }
    Ok(result)
}

fn uleb(bytes: &[u8], cursor: &mut usize) -> Result<u64, &'static str> {
    let mut result = 0u64;
    for shift in (0..70).step_by(7) {
        let byte = *bytes.get(*cursor).ok_or("truncated stack size ULEB128")?;
        *cursor += 1;
        if shift == 63 && byte > 1 {
            return Err("stack size ULEB128 overflow");
        }
        result |= u64::from(byte & 127) << shift;
        if byte & 128 == 0 {
            if shift != 0 && byte == 0 {
                return Err("noncanonical stack size ULEB128");
            }
            return Ok(result);
        }
    }
    Err("stack size ULEB128 overflow")
}
