//! Debug intervals from LLVM (design §1e). A unit is lowered with content-only locations:
//! one fixed file, each location's line the item-relative offset of its site plus one and
//! its column the index of its item in the module's `DebugPlan::items` plus one, so the
//! code is what a moved item would get. After codegen, `split` reads the object's own line
//! tables back as `DebugFunction`s and rebuilds the object without them: a unit's object
//! carries no DWARF, and the link's debug object resolves the intervals.
use hirsel_lir::debug::DebugFunction;
use hirsel_syntax::DefPath;
use object::read::{Object as _, ObjectComdat as _, ObjectSection as _, ObjectSymbol as _};
use std::collections::BTreeMap;

/// The module's items and the functions whose intervals `split` reads.
#[derive(Clone, Debug, Default)]
pub(crate) struct DebugPlan {
    /// The items locations name, by column minus one.
    pub items: Vec<DefPath>,
    /// Each function with a scope: its symbol (a library unit's body, not its root
    /// adapter), its own item and its declaration's offset.
    pub functions: Vec<(String, u32, u32)>,
}

impl DebugPlan {
    /// The column of `definition`'s locations, adding it as an item when new.
    pub(crate) fn column(&mut self, definition: &DefPath) -> u32 {
        let index = match self.items.iter().position(|item| item == definition) {
            Some(index) => index,
            None => {
                self.items.push(definition.clone());
                self.items.len() - 1
            }
        };
        index as u32 + 1
    }
}

/// A synthetic address: a section's index in the high half, an offset in the low half.
fn address(section: object::SectionIndex, offset: u64) -> u64 {
    ((section.0 as u64) << 32) | offset
}

/// The bytes of section `name` with its relocations applied: a reference to code is its
/// synthetic `address`, a reference to a debug section is its offset in that section.
fn relocated(file: &object::File<'_>, name: &str) -> Result<Option<Vec<u8>>, String> {
    let Some(section) = file.section_by_name(name) else {
        return Ok(None);
    };
    let mut bytes = section.data().map_err(|e| e.to_string())?.to_vec();
    for (offset, relocation) in section.relocations() {
        let object::RelocationTarget::Symbol(index) = relocation.target() else {
            return Err(format!("{name}: a relocation of no symbol"));
        };
        let symbol = file.symbol_by_index(index).map_err(|e| e.to_string())?;
        let base = match symbol.section() {
            object::SymbolSection::Section(target) => {
                let debug = file
                    .section_by_index(target)
                    .ok()
                    .and_then(|s| s.name().ok().map(|n| n.starts_with(".debug")))
                    .unwrap_or(false);
                if debug {
                    symbol.address()
                } else {
                    address(target, symbol.address())
                }
            }
            _ => return Err(format!("{name}: a relocation against an undefined symbol")),
        };
        let value = base.wrapping_add_signed(relocation.addend());
        let at = offset as usize;
        match relocation.size() {
            64 => bytes
                .get_mut(at..at + 8)
                .ok_or("a relocation outside its section")?
                .copy_from_slice(&value.to_le_bytes()),
            32 => bytes
                .get_mut(at..at + 4)
                .ok_or("a relocation outside its section")?
                .copy_from_slice(&(value as u32).to_le_bytes()),
            size => return Err(format!("{name}: a {size}-bit relocation")),
        }
    }
    Ok(Some(bytes))
}

/// Every line-table row of `debug_line`: (address, line, column, end of sequence).
fn rows(debug_line: &[u8]) -> Result<Vec<(u64, u64, u64, bool)>, String> {
    let error = |e: gimli::Error| e.to_string();
    let section = gimli::DebugLine::new(debug_line, gimli::LittleEndian);
    let mut rows = Vec::new();
    let mut offset = 0usize;
    while offset < debug_line.len() {
        let program = section
            .program(gimli::DebugLineOffset(offset), 8, None, None)
            .map_err(error)?;
        let length = program.header().unit_length() as usize;
        let mut program_rows = program.rows();
        while let Some((_, row)) = program_rows.next_row().map_err(error)? {
            let line = row.line().map_or(0, |l| l.get());
            let column = match row.column() {
                gimli::ColumnType::LeftEdge => 0,
                gimli::ColumnType::Column(c) => c.get(),
            };
            rows.push((row.address(), line, column, row.end_sequence()));
        }
        // A DWARF32 unit: its length word, then `length` bytes.
        offset += 4 + length;
    }
    Ok(rows)
}

/// The intervals of `plan`'s functions in `bytes`, and `bytes` rebuilt without its debug
/// sections and its address-significance table.
pub(crate) fn split(
    bytes: &[u8],
    plan: &DebugPlan,
) -> Result<(Vec<u8>, Vec<DebugFunction>), String> {
    let file = object::File::parse(bytes).map_err(|e| e.to_string())?;
    let rows = match relocated(&file, ".debug_line")? {
        Some(debug_line) => rows(&debug_line)?,
        None => Vec::new(),
    };
    // Ranges by section: (start, stop, line, column), from each row to the next.
    let mut mapped: BTreeMap<u64, Vec<(u64, u64, u64, u64)>> = BTreeMap::new();
    for pair in rows.windows(2) {
        let ((start, line, column, end), (stop, ..)) = (pair[0], pair[1]);
        if end || line == 0 || column == 0 || stop <= start {
            continue;
        }
        mapped.entry(start >> 32).or_default().push((
            start & 0xffff_ffff,
            stop & 0xffff_ffff,
            line,
            column,
        ));
    }
    let mut functions = Vec::new();
    for (symbol, own, declaration) in &plan.functions {
        let Some(found) = file.symbols().find(|s| s.name() == Ok(symbol.as_str())) else {
            continue;
        };
        let object::SymbolSection::Section(section) = found.section() else {
            continue;
        };
        let (begin, size) = (found.address(), found.size());
        let own = *own as usize;
        let mut items = vec![plan.items[own].clone()];
        let mut item = |column: u64| -> Option<u32> {
            let definition = plan.items.get(column as usize - 1)?;
            Some(match items.iter().position(|i| i == definition) {
                Some(index) => index as u32,
                None => {
                    items.push(definition.clone());
                    items.len() as u32 - 1
                }
            })
        };
        let mut ranges: Vec<(u64, u64, u32, u32)> = mapped
            .get(&(section.0 as u64))
            .into_iter()
            .flatten()
            .filter(|&&(start, stop, ..)| start >= begin && stop <= begin + size)
            .filter_map(|&(start, stop, line, column)| {
                Some((
                    start - begin,
                    stop - begin,
                    item(column)?,
                    u32::try_from(line - 1).ok()?,
                ))
            })
            .collect();
        ranges.sort();
        functions.push(DebugFunction::from_rows(
            symbol.clone(),
            size,
            items,
            *declaration,
            ranges,
        ));
    }
    Ok((strip(&file)?, functions))
}

/// `file` rebuilt with every section, symbol, relocation and COMDAT it has except its
/// debug sections, their relocations and its address-significance table (an optimization
/// hint whose symbol indices the rebuild would invalidate).
fn strip(file: &object::File<'_>) -> Result<Vec<u8>, String> {
    use object::write;
    let error = |e: object::Error| e.to_string();
    let dropped = |name: &str| name.starts_with(".debug") || name == ".llvm_addrsig";
    let mut out = write::Object::new(file.format(), file.architecture(), file.endianness());
    out.flags = file.flags();
    let mut sections = BTreeMap::new();
    for section in file.sections() {
        let name = section.name().map_err(error)?;
        // Symbol, string, relocation and group tables are the writer's to regenerate.
        if name.is_empty() || dropped(name) || section.kind() == object::SectionKind::Metadata {
            continue;
        }
        let id = out.add_section(Vec::new(), name.as_bytes().to_vec(), section.kind());
        let target = out.section_mut(id);
        target.flags = section.flags();
        if section.kind() == object::SectionKind::UninitializedData {
            target.append_bss(section.size(), section.align());
        } else {
            target.set_data(section.data().map_err(error)?.to_vec(), section.align());
        }
        sections.insert(section.index().0, id);
    }
    let mut symbols = BTreeMap::new();
    for symbol in file.symbols() {
        let section = match symbol.section() {
            object::SymbolSection::Section(index) => match sections.get(&index.0) {
                Some(&id) => write::SymbolSection::Section(id),
                None => continue,
            },
            object::SymbolSection::Undefined => write::SymbolSection::Undefined,
            object::SymbolSection::Absolute => write::SymbolSection::Absolute,
            object::SymbolSection::Common => write::SymbolSection::Common,
            object::SymbolSection::None => write::SymbolSection::None,
            _ => return Err("a symbol in an unknown section".into()),
        };
        if symbol.kind() == object::SymbolKind::Section {
            if let write::SymbolSection::Section(id) = section {
                symbols.insert(symbol.index().0, out.section_symbol(id));
            }
            continue;
        }
        let flags = match symbol.flags() {
            object::SymbolFlags::None => object::SymbolFlags::None,
            object::SymbolFlags::Elf { st_info, st_other } => {
                object::SymbolFlags::Elf { st_info, st_other }
            }
            _ => return Err("a symbol of another format's flags".into()),
        };
        let id = out.add_symbol(write::Symbol {
            name: symbol.name_bytes().map_err(error)?.to_vec(),
            value: symbol.address(),
            size: symbol.size(),
            kind: symbol.kind(),
            scope: symbol.scope(),
            weak: symbol.is_weak(),
            section,
            flags,
        });
        symbols.insert(symbol.index().0, id);
    }
    for section in file.sections() {
        let Some(&id) = sections.get(&section.index().0) else {
            continue;
        };
        for (offset, relocation) in section.relocations() {
            let object::RelocationTarget::Symbol(index) = relocation.target() else {
                return Err("a relocation of no symbol".into());
            };
            let symbol = *symbols
                .get(&index.0)
                .ok_or("a relocation against a dropped symbol")?;
            out.add_relocation(
                id,
                write::Relocation {
                    offset,
                    symbol,
                    addend: relocation.addend(),
                    flags: relocation.flags(),
                },
            )
            .map_err(|e| e.to_string())?;
        }
    }
    for comdat in file.comdats() {
        let symbol = *symbols
            .get(&comdat.symbol().0)
            .ok_or("a COMDAT of a dropped symbol")?;
        let members = comdat
            .sections()
            .filter_map(|index| sections.get(&index.0).copied())
            .collect();
        out.add_comdat(write::Comdat {
            kind: comdat.kind(),
            symbol,
            sections: members,
        });
    }
    out.write().map_err(|e| e.to_string())
}
