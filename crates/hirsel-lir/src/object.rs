//! Reading the symbol interface of an emitted object, independently of the backend that
//! emitted it.
//!
//! The symbol and relocation tables are read from the ELF bytes here, after every backend
//! handle is gone, so an artifact's imports and exports can be compared with its manifest.

use crate::layout_types::TargetSpec;

/// The ELF `e_machine` of a target's objects, or None for a triple this crate cannot read.
fn elf_machine(target: &TargetSpec) -> Option<u16> {
    match target.triple.split('-').next() {
        Some("x86_64") => Some(62),
        Some("aarch64") => Some(183),
        _ => None,
    }
}

/// A relocatable ELF object's bytes and linkage interface. It owns its bytes and borrows
/// nothing from the backend or from the input it was compiled from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ObjectFile {
    pub bytes: Vec<u8>,
    /// Global and weak symbols the object defines, sorted.
    pub exports: Vec<String>,
    /// Global and weak symbols the object references without defining, sorted.
    pub imports: Vec<String>,
    /// Relocations against imported symbols, in section order.
    pub relocations: Vec<ImportRelocation>,
    /// Every thread-local storage use: a TLS section, a TLS symbol or a TLS relocation, each
    /// described. Generated code reads no thread-local state, since a task's frames may resume
    /// on another thread (its context argument names everything it reaches), so an emitter's
    /// object has none.
    pub thread_locals: Vec<String>,
}

/// One relocation whose symbol is an import.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImportRelocation {
    pub offset: u64,
    pub symbol: String,
    pub kind: u32,
    pub addend: i64,
}

impl ObjectFile {
    /// Checks that `bytes` is a little-endian ELF64 relocatable object for `target` and reads
    /// its symbol interface; the error says what is malformed.
    pub fn read(target: &TargetSpec, bytes: Vec<u8>) -> Result<Self, String> {
        let (mut exports, mut imports, relocations, thread_locals) =
            symbols(target, &bytes).map_err(str::to_owned)?;
        exports.sort();
        exports.dedup();
        imports.sort();
        imports.dedup();
        Ok(Self {
            bytes,
            exports,
            imports,
            relocations,
            thread_locals,
        })
    }
}

const ELF_HEADER_SIZE: usize = 64;
const SECTION_HEADER_SIZE: usize = 64;
const SYMBOL_SIZE: usize = 24;
const ET_REL: u16 = 1;
const SHT_SYMTAB: u32 = 2;
const SHT_RELA: u32 = 4;
const SHT_REL: u32 = 9;
const RELA_SIZE: usize = 24;
const SHN_UNDEF: u16 = 0;
const STB_GLOBAL: u8 = 1;
const STB_WEAK: u8 = 2;
const STT_TLS: u8 = 6;
const SHF_TLS: u64 = 0x400;

/// Whether ELF relocation type `kind` of machine `machine` addresses thread-local storage:
/// x86-64's TLS family (`R_X86_64_DTPMOD64` .. `R_X86_64_TPOFF32` and the TLS descriptors) and
/// AArch64's (`R_AARCH64_TLSGD_*` .. `R_AARCH64_TLSDESC_*` and the dynamic TLS types).
fn tls_relocation(machine: u16, kind: u32) -> bool {
    match machine {
        62 => matches!(kind, 16..=23 | 34..=36),
        183 => matches!(kind, 512..=573 | 1028..=1031),
        _ => false,
    }
}

fn read_bytes<const N: usize>(bytes: &[u8], offset: usize) -> Result<[u8; N], &'static str> {
    offset
        .checked_add(N)
        .and_then(|end| bytes.get(offset..end))
        .map(|slice| slice.try_into().expect("slice has length N"))
        .ok_or("a header or table runs past the end of the object")
}

fn u16_at(bytes: &[u8], offset: usize) -> Result<u16, &'static str> {
    read_bytes(bytes, offset).map(u16::from_le_bytes)
}

fn u32_at(bytes: &[u8], offset: usize) -> Result<u32, &'static str> {
    read_bytes(bytes, offset).map(u32::from_le_bytes)
}

fn offset_at(bytes: &[u8], offset: usize) -> Result<usize, &'static str> {
    let value = read_bytes(bytes, offset).map(u64::from_le_bytes)?;
    usize::try_from(value).map_err(|_| "an offset does not fit in usize")
}

/// A `(offset, size)` range of the object's bytes.
fn range(bytes: &[u8], offset: usize, size: usize) -> Result<&[u8], &'static str> {
    offset
        .checked_add(size)
        .and_then(|end| bytes.get(offset..end))
        .ok_or("a section runs past the end of the object")
}

fn name_at(strings: &[u8], offset: usize) -> Result<String, &'static str> {
    let tail = strings
        .get(offset..)
        .ok_or("a symbol name is outside its string table")?;
    let end = tail
        .iter()
        .position(|&byte| byte == 0)
        .ok_or("a symbol name is not NUL-terminated")?;
    String::from_utf8(tail[..end].to_vec()).map_err(|_| "a symbol name is not UTF-8")
}

/// The exports, imports, import relocations and thread-local storage uses of an ELF64
/// little-endian relocatable object.
type Interface = (Vec<String>, Vec<String>, Vec<ImportRelocation>, Vec<String>);

fn symbols(target: &TargetSpec, bytes: &[u8]) -> Result<Interface, &'static str> {
    let ident: [u8; 6] = read_bytes(bytes, 0)?;
    if ident[..4] != *b"\x7fELF" {
        return Err("not an ELF file");
    }
    if ident[4] != 2 || ident[5] != 1 {
        return Err("not a little-endian ELF64 file");
    }
    if u16_at(bytes, 16)? != ET_REL {
        return Err("not a relocatable object");
    }
    let machine = u16_at(bytes, 18)?;
    if Some(machine) != elf_machine(target) {
        return Err("the ELF machine is not the target's");
    }
    let section_table = offset_at(bytes, 40)?;
    if usize::from(u16_at(bytes, 58)?) != SECTION_HEADER_SIZE || section_table < ELF_HEADER_SIZE {
        return Err("unexpected section header table");
    }
    let section_count = usize::from(u16_at(bytes, 60)?);
    let section = |index: usize| -> Result<usize, &'static str> {
        let header = index
            .checked_mul(SECTION_HEADER_SIZE)
            .and_then(|offset| offset.checked_add(section_table))
            .ok_or("a section header offset overflows")?;
        read_bytes::<SECTION_HEADER_SIZE>(bytes, header)?;
        Ok(header)
    };
    let contents = |header: usize| -> Result<&[u8], &'static str> {
        range(
            bytes,
            offset_at(bytes, header + 24)?,
            offset_at(bytes, header + 32)?,
        )
    };

    let mut thread_locals = Vec::new();
    for index in 0..section_count {
        let header = section(index)?;
        if read_bytes(bytes, header + 8).map(u64::from_le_bytes)? & SHF_TLS != 0 {
            thread_locals.push(format!("TLS section {index}"));
        }
    }

    // The one symbol table: its section index, and per entry the name of a global or weak
    // symbol and whether it is undefined.
    type Names = Vec<Option<(String, bool)>>;
    let mut table: Option<(usize, Names)> = None;
    for index in 0..section_count {
        let header = section(index)?;
        if u32_at(bytes, header + 4)? != SHT_SYMTAB {
            continue;
        }
        if table.is_some() {
            return Err("a relocatable object has exactly one symbol table");
        }
        if offset_at(bytes, header + 56)? != SYMBOL_SIZE {
            return Err("unexpected symbol entry size");
        }
        let entries = contents(header)?;
        let link =
            usize::try_from(u32_at(bytes, header + 40)?).map_err(|_| "bad string table link")?;
        if link >= section_count {
            return Err("the symbol table links to a missing string table");
        }
        let strings = contents(section(link)?)?;
        if entries.len() % SYMBOL_SIZE != 0 {
            return Err("the symbol table size is not a whole number of entries");
        }
        let mut names = Vec::new();
        for symbol in entries.as_chunks::<SYMBOL_SIZE>().0 {
            if symbol[4] & 0xf == STT_TLS {
                let name = name_at(
                    strings,
                    usize::try_from(u32_at(symbol, 0)?).map_err(|_| "bad name offset")?,
                )?;
                thread_locals.push(format!("TLS symbol `{name}`"));
            }
            let binding = symbol[4] >> 4;
            if binding != STB_GLOBAL && binding != STB_WEAK {
                names.push(None);
                continue;
            }
            let name = name_at(
                strings,
                usize::try_from(u32_at(symbol, 0)?).map_err(|_| "bad name offset")?,
            )?;
            if name.is_empty() {
                return Err("a global symbol has no name");
            }
            names.push(Some((name, u16_at(symbol, 6)? == SHN_UNDEF)));
        }
        table = Some((index, names));
    }
    let Some((table_index, names)) = table else {
        return Err("a relocatable object has exactly one symbol table");
    };

    let mut relocations = Vec::new();
    for index in 0..section_count {
        let header = section(index)?;
        match u32_at(bytes, header + 4)? {
            SHT_RELA => {}
            SHT_REL => return Err("REL relocations without addends are not expected"),
            _ => continue,
        }
        if usize::try_from(u32_at(bytes, header + 40)?).ok() != Some(table_index) {
            return Err("a relocation section names another symbol table");
        }
        if offset_at(bytes, header + 56)? != RELA_SIZE {
            return Err("unexpected relocation entry size");
        }
        let entries = contents(header)?;
        if entries.len() % RELA_SIZE != 0 {
            return Err("the relocation table size is not a whole number of entries");
        }
        for entry in entries.as_chunks::<RELA_SIZE>().0 {
            let info = read_bytes(entry, 8).map(u64::from_le_bytes)?;
            if tls_relocation(machine, info as u32) {
                thread_locals.push(format!(
                    "TLS relocation type {} in section {index}",
                    info as u32
                ));
            }
            let symbol = usize::try_from(info >> 32).map_err(|_| "bad relocation symbol")?;
            let named = names
                .get(symbol)
                .ok_or("a relocation names a missing symbol")?;
            if let Some((name, true)) = named {
                relocations.push(ImportRelocation {
                    offset: read_bytes(entry, 0).map(u64::from_le_bytes)?,
                    symbol: name.clone(),
                    kind: info as u32,
                    addend: read_bytes(entry, 16).map(i64::from_le_bytes)?,
                });
            }
        }
    }

    let mut exports = Vec::new();
    let mut imports = Vec::new();
    for (name, undefined) in names.into_iter().flatten() {
        if undefined {
            imports.push(name);
        } else {
            exports.push(name);
        }
    }
    Ok((exports, imports, relocations, thread_locals))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A minimal ELF64 relocatable object: null section, symbol table, string table.
    fn object(machine: u16, symbols: &[(&str, u8, u16)]) -> Vec<u8> {
        let mut strings = vec![0u8];
        let mut table = vec![0u8; SYMBOL_SIZE];
        for &(name, binding, section) in symbols {
            let mut entry = [0u8; SYMBOL_SIZE];
            entry[..4].copy_from_slice(&(strings.len() as u32).to_le_bytes());
            entry[4] = binding << 4;
            entry[6..8].copy_from_slice(&section.to_le_bytes());
            table.extend_from_slice(&entry);
            strings.extend_from_slice(name.as_bytes());
            strings.push(0);
        }
        let table_offset = ELF_HEADER_SIZE;
        let strings_offset = table_offset + table.len();
        let sections = strings_offset + strings.len();
        let mut bytes = vec![0u8; ELF_HEADER_SIZE];
        bytes[..6].copy_from_slice(b"\x7fELF\x02\x01");
        bytes[16..18].copy_from_slice(&ET_REL.to_le_bytes());
        bytes[18..20].copy_from_slice(&machine.to_le_bytes());
        bytes[40..48].copy_from_slice(&(sections as u64).to_le_bytes());
        bytes[58..60].copy_from_slice(&(SECTION_HEADER_SIZE as u16).to_le_bytes());
        bytes[60..62].copy_from_slice(&3u16.to_le_bytes());
        bytes.extend_from_slice(&table);
        bytes.extend_from_slice(&strings);
        let mut header = |kind: u32, offset: usize, size: usize, link: u32, entry: usize| {
            let mut h = [0u8; SECTION_HEADER_SIZE];
            h[4..8].copy_from_slice(&kind.to_le_bytes());
            h[24..32].copy_from_slice(&(offset as u64).to_le_bytes());
            h[32..40].copy_from_slice(&(size as u64).to_le_bytes());
            h[40..44].copy_from_slice(&link.to_le_bytes());
            h[56..64].copy_from_slice(&(entry as u64).to_le_bytes());
            bytes.extend_from_slice(&h);
        };
        header(0, 0, 0, 0, 0);
        header(SHT_SYMTAB, table_offset, table.len(), 2, SYMBOL_SIZE);
        header(3, strings_offset, strings.len(), 0, 0);
        bytes
    }

    #[test]
    fn reads_global_definitions_and_references() {
        let bytes = object(
            62,
            &[
                ("local_helper", 0, 1),
                ("exported", STB_GLOBAL, 1),
                ("weak_export", STB_WEAK, 1),
                ("imported", STB_GLOBAL, SHN_UNDEF),
            ],
        );
        let artifact = ObjectFile::read(&TargetSpec::x86_64_unknown_linux_gnu(), bytes).unwrap();
        assert_eq!(artifact.exports, ["exported", "weak_export"]);
        assert_eq!(artifact.imports, ["imported"]);
        assert!(artifact.relocations.is_empty());
    }

    #[test]
    fn reports_thread_local_sections_symbols_and_relocations() {
        let x86 = TargetSpec::x86_64_unknown_linux_gnu();
        let plain = object(62, &[("word", STB_GLOBAL, 1)]);
        assert!(
            ObjectFile::read(&x86, plain.clone())
                .unwrap()
                .thread_locals
                .is_empty()
        );
        let mut symbol = plain.clone();
        symbol[ELF_HEADER_SIZE + SYMBOL_SIZE + 4] |= STT_TLS;
        assert_eq!(
            ObjectFile::read(&x86, symbol).unwrap().thread_locals,
            ["TLS symbol `word`"]
        );
        let mut section = plain;
        let sections =
            usize::try_from(u64::from_le_bytes(section[40..48].try_into().unwrap())).unwrap();
        let flags = sections + SECTION_HEADER_SIZE + 8;
        section[flags..flags + 8].copy_from_slice(&SHF_TLS.to_le_bytes());
        assert_eq!(
            ObjectFile::read(&x86, section).unwrap().thread_locals,
            ["TLS section 1"]
        );
        // x86-64 initial-exec and local-exec, AArch64 TLS descriptors; ordinary calls are not.
        assert!(tls_relocation(62, 22) && tls_relocation(62, 23) && tls_relocation(62, 36));
        assert!(tls_relocation(183, 562) && tls_relocation(183, 1031));
        assert!(!tls_relocation(62, 4) && !tls_relocation(183, 283));
    }

    fn rejection(target: TargetSpec, bytes: Vec<u8>) -> String {
        match ObjectFile::read(&target, bytes) {
            Err(message) => message,
            other => panic!("expected a malformed-object error, got {other:?}"),
        }
    }

    #[test]
    fn rejects_malformed_objects() {
        let good = object(62, &[("f", STB_GLOBAL, 1)]);
        assert_eq!(
            rejection(TargetSpec::x86_64_unknown_linux_gnu(), b"\x7fELF".to_vec()),
            "a header or table runs past the end of the object"
        );
        assert_eq!(
            rejection(
                TargetSpec::x86_64_unknown_linux_gnu(),
                b"MZ\x90\x00\x03\x00".to_vec()
            ),
            "not an ELF file"
        );
        assert_eq!(
            rejection(TargetSpec::aarch64_unknown_linux_gnu(), good.clone()),
            "the ELF machine is not the target's"
        );
        let mut executable = good.clone();
        executable[16] = 2;
        assert_eq!(
            rejection(TargetSpec::x86_64_unknown_linux_gnu(), executable),
            "not a relocatable object"
        );
        let mut truncated = good.clone();
        truncated.truncate(good.len() - 1);
        assert_eq!(
            rejection(TargetSpec::x86_64_unknown_linux_gnu(), truncated),
            "a header or table runs past the end of the object"
        );
        let mut unterminated = good.clone();
        let strings_end = ELF_HEADER_SIZE + 2 * SYMBOL_SIZE + 3;
        unterminated[strings_end - 1] = b'g';
        assert_eq!(
            rejection(TargetSpec::x86_64_unknown_linux_gnu(), unterminated),
            "a symbol name is not NUL-terminated"
        );
        assert_eq!(
            rejection(
                TargetSpec::x86_64_unknown_linux_gnu(),
                object(62, &[("", STB_GLOBAL, 1)])
            ),
            "a global symbol has no name"
        );
    }
}
