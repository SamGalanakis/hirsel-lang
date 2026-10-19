//! DWARF 5 line tables over Cranelift's final machine source-location ranges.
use std::collections::HashMap;

use cranelift_codegen::CompiledCode;
use cranelift_module::FuncId;
use cranelift_object::{ObjectProduct, object};
use gimli::write::{
    Address, AttributeValue, DwarfUnit, EndianVec, LineProgram, LineString, Range, RangeList,
    RelocateWriter, Relocation, RelocationTarget, Sections,
};
use gimli::{Encoding, Format, LineEncoding, LittleEndian};
use hirsel_lir::ir::LirUnit;
use hirsel_lir::profile::ProfileSource;
use hirsel_lir::stage::Ready;

use crate::CodegenError;
use crate::module::Emitted;

pub(crate) struct DebugFunction {
    id: FuncId,
    name: String,
    linkage: String,
    path: String,
    line: u32,
    size: u64,
    rows: Vec<(u64, u64, u32, u32)>,
}

impl DebugFunction {
    pub(crate) fn new(
        id: FuncId,
        unit: &LirUnit<Ready>,
        source: &ProfileSource,
        code: &CompiledCode,
    ) -> Emitted<Self> {
        let position = |offset| {
            source
                .location(offset)
                .ok_or_else(|| CodegenError::Invalid {
                    definition: unit.header.definition.clone(),
                    what: "debug anchor is outside its source revision".into(),
                })
        };
        let entry = unit.tables.source_anchors[unit.blocks[unit.entry.index()].anchor.index()].site;
        let mut rows = Vec::new();
        for range in code.buffer.get_srclocs_sorted() {
            if range.loc.is_default() || range.start == range.end {
                continue;
            }
            let location = position(range.loc.bits())?;
            rows.push((
                u64::from(range.start),
                u64::from(range.end),
                location.line,
                location.column,
            ));
        }
        Ok(Self {
            id,
            name: unit.header.definition.to_string(),
            linkage: hirsel_lir::emission::symbol_name(
                &unit.header.definition,
                Some(&unit.header.instance),
            ),
            path: source.logical_path().to_string_lossy().into_owned(),
            line: position(entry.start)?.line,
            size: code.code_buffer().len() as u64,
            rows,
        })
    }
}

#[derive(Clone)]
struct Section {
    bytes: EndianVec<LittleEndian>,
    relocations: Vec<Relocation>,
}

impl RelocateWriter for Section {
    type Writer = EndianVec<LittleEndian>;
    fn writer(&self) -> &Self::Writer {
        &self.bytes
    }
    fn writer_mut(&mut self) -> &mut Self::Writer {
        &mut self.bytes
    }
    fn relocate(&mut self, relocation: Relocation) {
        self.relocations.push(relocation);
    }
}

fn error(message: impl ToString) -> CodegenError {
    CodegenError::Object {
        message: message.to_string(),
    }
}

pub(crate) fn write(product: &mut ObjectProduct, functions: Vec<DebugFunction>) -> Emitted {
    let Some(first) = functions.first() else {
        return Ok(());
    };
    let encoding = Encoding {
        format: Format::Dwarf32,
        version: 5,
        address_size: 8,
    };
    let mut dwarf = DwarfUnit::new(encoding);
    let mut lines = LineProgram::new(
        encoding,
        LineEncoding::default(),
        LineString::String(ProfileSource::COMPILATION_DIRECTORY.as_bytes().to_vec()),
        None,
        LineString::String(first.path.as_bytes().to_vec()),
        None,
    );
    let root = dwarf.unit.root();
    dwarf.unit.get_mut(root).set(
        gimli::DW_AT_name,
        AttributeValue::String(first.path.as_bytes().to_vec()),
    );
    dwarf.unit.get_mut(root).set(
        gimli::DW_AT_comp_dir,
        AttributeValue::String(ProfileSource::COMPILATION_DIRECTORY.as_bytes().to_vec()),
    );
    dwarf.unit.get_mut(root).set(
        gimli::DW_AT_producer,
        AttributeValue::String(crate::TOOLCHAIN.as_bytes().to_vec()),
    );
    let mut ranges = Vec::new();
    let mut symbols = Vec::new();
    for (symbol, function) in functions.iter().enumerate() {
        symbols.push(product.function_symbol(function.id));
        let address = Address::Symbol { symbol, addend: 0 };
        ranges.push(Range::StartLength {
            begin: address,
            length: function.size,
        });
        let file = lines.add_file(
            LineString::String(function.path.as_bytes().to_vec()),
            lines.default_directory(),
            None,
        );
        // End a sequence at each gap too: generated code without a source range must
        // not inherit the preceding row's line. Sequence state resets at each begin.
        let mut end = None;
        for &(start, stop, line, column) in &function.rows {
            if end != Some(start) {
                if let Some(end) = end {
                    lines.end_sequence(end);
                }
                lines.begin_sequence(Some(address));
            }
            let row = lines.row();
            row.address_offset = start;
            row.file = file;
            row.line = u64::from(line);
            row.column = u64::from(column);
            lines.generate_row();
            end = Some(stop);
        }
        if let Some(end) = end {
            lines.end_sequence(end);
        }
        let entry = dwarf.unit.add(root, gimli::DW_TAG_subprogram);
        let entry = dwarf.unit.get_mut(entry);
        entry.set(
            gimli::DW_AT_name,
            AttributeValue::String(function.name.as_bytes().to_vec()),
        );
        entry.set(
            gimli::DW_AT_linkage_name,
            AttributeValue::String(function.linkage.as_bytes().to_vec()),
        );
        entry.set(gimli::DW_AT_external, AttributeValue::Flag(true));
        entry.set(
            gimli::DW_AT_decl_file,
            AttributeValue::FileIndex(Some(file)),
        );
        entry.set(
            gimli::DW_AT_decl_line,
            AttributeValue::Udata(u64::from(function.line)),
        );
        entry.set(gimli::DW_AT_low_pc, AttributeValue::Address(address));
        entry.set(gimli::DW_AT_high_pc, AttributeValue::Udata(function.size));
    }
    let ranges = dwarf.unit.ranges.add(RangeList(ranges));
    dwarf
        .unit
        .get_mut(root)
        .set(gimli::DW_AT_ranges, AttributeValue::RangeListRef(ranges));
    dwarf.unit.line_program = lines;
    let mut sections = Sections::new(Section {
        bytes: EndianVec::new(LittleEndian),
        relocations: Vec::new(),
    });
    dwarf.write(&mut sections).map_err(error)?;
    let mut ids = HashMap::new();
    sections.for_each(|id, section| -> Emitted {
        if !section.bytes.slice().is_empty() {
            let object_id = product.object.add_section(
                Vec::new(),
                id.name().as_bytes().to_vec(),
                object::SectionKind::Debug,
            );
            product
                .object
                .section_mut(object_id)
                .set_data(section.bytes.slice().to_vec(), 1);
            ids.insert(id, object_id);
        }
        Ok(())
    })?;
    sections.for_each(|id, section| -> Emitted {
        for relocation in &section.relocations {
            let symbol = match relocation.target {
                RelocationTarget::Symbol(index) => symbols[index],
                RelocationTarget::Section(id) => product.object.section_symbol(ids[&id]),
            };
            product
                .object
                .add_relocation(
                    ids[&id],
                    object::write::Relocation {
                        offset: relocation.offset as u64,
                        symbol,
                        addend: relocation.addend,
                        flags: object::RelocationFlags::Generic {
                            kind: object::RelocationKind::Absolute,
                            encoding: object::RelocationEncoding::Generic,
                            size: relocation.size * 8,
                        },
                    },
                )
                .map_err(error)?;
        }
        Ok(())
    })
}
