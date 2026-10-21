//! The program's glue object (`hirsel_lir::glue`), as `hirsel-llvm` emits it: every number
//! the program assigns as content-named symbols the unit objects read, the runtime
//! descriptors and registry that give those numbers meaning, the program's statics and
//! text literals, codec records and C callback thunks. Regenerated at every link, so a
//! unit object never depends on the program it is linked into.
use cranelift_codegen::ir::{AbiParam, types};
use cranelift_module::{DataDescription, Linkage, Module};
use hirsel_lir::emission::{GlueCallback, GlueInputs, glue_artifact, symbol_name};
use hirsel_lir::glue::{
    GlueCodec, codec_symbol, descriptor_index_symbol, failure_code_symbol, literal_symbol,
    static_symbol,
};
use hirsel_lir::object::ObjectFile;
use hirsel_lir::stage::{ObjectArtifact, ObjectSymbol, ObjectSymbolKind};
use hirsel_lir::statics::StaticData;
use hirsel_syntax::DefPath;

use crate::CodegenError;
use crate::data::StaticBlock;
use crate::module::{CheckThunk, Emitted, Emitter, Helper, Hidden, Inputs, PTR};

/// Glue's errors name no unit; they name this definition.
fn glue_definition() -> DefPath {
    DefPath::core("$native.glue")
}

impl Emitter<'_> {
    /// Declares the data export `name`.
    fn export_data(&mut self, name: &str) {
        self.manifested.exports.insert(
            name.to_owned(),
            ObjectSymbol {
                name: name.to_owned(),
                kind: ObjectSymbolKind::Data,
                definition: None,
                abi: None,
            },
        );
    }

    /// The exported u32 cell `name` holding `value`. Declared through the module, so a
    /// cell this object also imports becomes its definition.
    fn cell(&mut self, name: &str, value: u32) -> Emitted {
        let id = self
            .module
            .declare_data(name, Linkage::Export, false, false)
            .map_err(|e| CodegenError::Cranelift {
                name: name.to_owned(),
                message: e.to_string(),
            })?;
        self.data.insert(name.to_owned(), id);
        let mut description = DataDescription::new();
        description.define(value.to_le_bytes().to_vec().into_boxed_slice());
        description.set_align(4);
        self.define_data(name, id, &description)?;
        self.export_data(name);
        Ok(())
    }

    /// Every registry layout's index cell and every rule's failure-code cell.
    pub(crate) fn cells(&mut self) -> Emitted {
        let glue = self.inputs.glue();
        let definition = glue_definition();
        for id in glue.registry.layouts.clone() {
            let index = self.descriptor_index(id, &definition)?;
            self.cell(&descriptor_index_symbol(&self.inputs.keys.key(id)), index)?;
        }
        for definition in &glue.glue.foreign {
            let name = hirsel_lir::glue::foreign_dispatch_symbol(definition);
            let value = glue.foreign.get(definition).copied().unwrap_or(0);
            let id = self
                .module
                .declare_data(&name, Linkage::Export, false, false)
                .map_err(|e| CodegenError::Cranelift {
                    name: name.clone(),
                    message: e.to_string(),
                })?;
            self.data.insert(name.clone(), id);
            let mut description = DataDescription::new();
            description.define(value.to_le_bytes().to_vec().into_boxed_slice());
            description.set_align(8);
            self.define_data(&name, id, &description)?;
            self.export_data(&name);
        }
        for (rule, code) in &glue.failures.codes {
            self.cell(&failure_code_symbol(rule), code.rule)?;
        }
        Ok(())
    }

    /// The codec `codec` as the exported `hirsel_rt_v2_codec` record `hirsel.codec.<key>`:
    /// `{descriptor, descriptor length, program, program length, check thunks, check
    /// count}`, with every layout its program allocates described.
    fn codec(&mut self, codec: &GlueCodec) -> Emitted {
        let definition = glue_definition();
        let plan = &codec.plan;
        let layouts = self.inputs.layouts;
        for layout in plan.allocated(layouts) {
            self.descriptor(layout, &definition)?;
        }
        let registry = &self.inputs.glue().registry;
        let words = plan
            .program(layouts, &|id| registry.index(id))
            .map_err(|error| CodegenError::Manifest {
                problem: format!("a codec plan off its layouts: {error}"),
            })?;
        let descriptor = self.constant("hirsel.codec.descriptor", plan.descriptor.clone(), 1)?;
        let bytes = words.iter().flat_map(|w| w.to_le_bytes()).collect();
        let program = self.constant("hirsel.codec.program", bytes, 8)?;
        let checks = self.codec_checks(codec)?;
        let mut image = vec![0u8; 48];
        image[8..16].copy_from_slice(&(plan.descriptor.len() as u64).to_le_bytes());
        image[24..32].copy_from_slice(&(words.len() as u64).to_le_bytes());
        image[40..48].copy_from_slice(&(codec.checks.len() as u64).to_le_bytes());
        let name = codec_symbol(&codec.key);
        let (id, _) = self.declare_data(&name, Linkage::Export)?;
        let mut description = DataDescription::new();
        description.define(image.into_boxed_slice());
        description.set_align(8);
        for (offset, target) in [(0, descriptor), (16, program)]
            .into_iter()
            .chain(checks.map(|table| (32, table)))
        {
            let gv = self.module.declare_data_in_data(target, &mut description);
            description.write_data_addr(offset, gv, 0);
        }
        self.define_data(&name, id, &description)?;
        self.export_data(&name);
        Ok(())
    }

    /// The table of a decode plan's check thunks (SER-1), or None without checks.
    fn codec_checks(&mut self, codec: &GlueCodec) -> Emitted<Option<cranelift_module::DataId>> {
        if codec.checks.is_empty() {
            return Ok(None);
        }
        let invalid = |what: &str| CodegenError::Invalid {
            definition: glue_definition(),
            what: what.into(),
        };
        let mut thunks = Vec::with_capacity(codec.checks.len());
        for (check, layout) in codec.checks.iter().zip(codec.plan.check_layouts()) {
            let layout = layout.ok_or_else(|| invalid("a codec check no CHECKED node calls"))?;
            let definition = check.definition.clone();
            let abi = self.fn_abi(check.abi, &definition)?;
            let name = symbol_name(&definition, check.instance.as_ref());
            self.manifested
                .import(&name, ObjectSymbolKind::Function, Some(&definition));
            let signature = self.entry_signature(abi, &definition)?;
            if signature
                .hidden
                .iter()
                .any(|h| !matches!(h, Hidden::Context | Hidden::Result))
            {
                return Err(invalid(
                    "a codec check with a hidden slot besides its result",
                ));
            }
            let body = self.import_function(&name, &signature.ir)?;
            let [parameter] = abi.parameters.as_slice() else {
                return Err(invalid("a codec check without exactly one parameter"));
            };
            let slots = match parameter.passing {
                hirsel_lir::abi_types::ParameterPassing::ReadOnlyAddress => None,
                hirsel_lir::abi_types::ParameterPassing::ByValue => {
                    let record = self
                        .inputs
                        .layouts
                        .layout(layout)
                        .ok_or_else(|| invalid("a codec check of no layout"))?;
                    let scalars = &record.abi.scalars;
                    if scalars.len() != parameter.slots.len() {
                        return Err(invalid("a codec check's slots differ from its layout's"));
                    }
                    Some(
                        scalars
                            .iter()
                            .zip(&parameter.slots)
                            .map(|(&(offset, _), slot)| (offset, crate::module::physical(slot.ty)))
                            .collect(),
                    )
                }
                _ => return Err(invalid("a codec check's value passed other than by borrow")),
            };
            let register = signature.value.is_some();
            let mut sig = cranelift_codegen::ir::Signature::new(self.call_conv);
            sig.params
                .extend([AbiParam::new(PTR), AbiParam::new(PTR), AbiParam::new(PTR)]);
            sig.returns.push(AbiParam::new(types::I8).uext());
            let thunk = self.helper(&format!("hirsel.check.{name}"), &sig, |_| {
                Helper::Check(CheckThunk {
                    body,
                    slots,
                    register,
                })
            })?;
            thunks.push(thunk);
        }
        let word = u64::from(self.inputs.target.pointer_bytes) as usize;
        self.anonymous += 1;
        let name = format!("hirsel.codec.checks.{}", self.anonymous);
        let (id, _) = self.declare_data(&name, Linkage::Local)?;
        let mut description = DataDescription::new();
        description.define(vec![0u8; word * thunks.len()].into_boxed_slice());
        description.set_align(8);
        for (i, thunk) in thunks.into_iter().enumerate() {
            let f = self.module.declare_func_in_data(thunk, &mut description);
            description.write_function_addr((i * word) as u32, f);
        }
        self.define_data(&name, id, &description)?;
        Ok(Some(id))
    }

    /// The static immortal blocks of `data`: block 0 exported as `static_symbol`, the
    /// blocks it reaches private beside it.
    fn statics(&mut self, data: &StaticData) -> Emitted {
        let definition = glue_definition();
        data.validate(
            self.inputs.layouts,
            u64::from(self.inputs.target.pointer_bytes),
        )
        .map_err(|what| CodegenError::Invalid {
            definition: glue_definition(),
            what,
        })?;
        let name = static_symbol(&data.symbol);
        let invalid = |what: &str| CodegenError::Invalid {
            definition: glue_definition(),
            what: what.into(),
        };
        // A block names only blocks after it, so each is emitted after its targets.
        let mut emitted: Vec<Option<StaticBlock>> = vec![None; data.blocks.len()];
        for (index, block) in data.blocks.iter().enumerate().rev() {
            let record = self
                .inputs
                .layouts
                .layout(block.layout)
                .ok_or_else(|| invalid("static data of no layout"))?;
            if let hirsel_lir::layout_types::SizeSpec::Fixed(size) = record.size
                && size != block.bytes.len() as u64
            {
                return Err(invalid("static data of another size than its layout"));
            }
            let mut references = Vec::with_capacity(block.references.len());
            for r in &block.references {
                let target = r.target as usize;
                let Some(Some(target)) = emitted.get(target).filter(|_| target > index) else {
                    return Err(invalid("a static reference to no later block"));
                };
                references.push((r.offset, *target));
            }
            let descriptor = self.descriptor_index(block.layout, &definition)?;
            let block_name = match index {
                0 => name.clone(),
                _ => format!("{name}.{index}"),
            };
            emitted[index] = Some(self.static_block_functions(
                &block_name,
                descriptor,
                record.align,
                &block.bytes,
                &references,
                &block.functions,
                index == 0,
                &definition,
            )?);
        }
        self.export_data(&name);
        Ok(())
    }

    /// The text literal block of `data` at the sequence payload `payload`, exported as
    /// `literal_symbol`.
    fn literal(&mut self, payload: hirsel_lir::LayoutId, data: &[u8]) -> Emitted {
        let definition = glue_definition();
        let descriptor = self.descriptor_index(payload, &definition)?;
        let Some(record) = self.inputs.layouts.layout(payload) else {
            return Err(CodegenError::Invalid {
                definition,
                what: "no literal payload layout".into(),
            });
        };
        let name = literal_symbol(&self.inputs.keys.key(payload), data);
        self.static_block(
            &name,
            descriptor,
            record.align,
            data,
            &[],
            true,
            &definition,
        )?;
        self.export_data(&name);
        Ok(())
    }

    /// The C callback thunk of `callback` (PROG-4), calling its unit's entry.
    fn callback(&mut self, callback: &GlueCallback) -> Emitted {
        let definition = &callback.definition;
        let abi = self.fn_abi(callback.abi, definition)?;
        let name = symbol_name(definition, callback.instance.as_ref());
        let sig = self.entry_signature(abi, definition)?;
        let target = self.import_function(&name, &sig.ir)?;
        self.manifested.imports.insert(
            name.clone(),
            ObjectSymbol {
                name,
                kind: ObjectSymbolKind::Function,
                definition: Some(definition.clone()),
                abi: Some(hirsel_lir::abi::abi_digest(abi)),
            },
        );
        let thunk =
            hirsel_lir::emission::callback_thunk_name(definition, callback.instance.as_ref());
        self.manifested.exports.insert(
            thunk.clone(),
            ObjectSymbol {
                name: thunk.clone(),
                kind: ObjectSymbolKind::Function,
                definition: Some(definition.clone()),
                abi: Some(hirsel_lir::abi::abi_digest(abi)),
            },
        );
        self.define_callback(&thunk, target, &sig, Linkage::Export, definition)
    }
}

/// The program's glue object for `inputs` (see the module).
pub(crate) fn emit_glue(inputs: &GlueInputs<'_>) -> Emitted<ObjectArtifact> {
    if !inputs.registry.validates(inputs.layouts, inputs.finalizers) {
        return Err(CodegenError::Manifest {
            problem: "foreign or noncanonical descriptor registry".into(),
        });
    }
    let mut emitter = Emitter::new(Inputs::program(inputs), false)?;
    let definition = glue_definition();
    emitter.registry_table(&definition)?;
    emitter.cells()?;
    for codec in &inputs.glue.codecs {
        emitter.codec(codec)?;
    }
    for symbol in &inputs.glue.statics {
        let Some(data) = inputs.statics.iter().find(|d| d.symbol == *symbol) else {
            return Err(CodegenError::Invalid {
                definition: symbol.definition.clone(),
                what: "a static without its data".into(),
            });
        };
        emitter.statics(data)?;
    }
    for (payload, data) in &inputs.glue.literals {
        emitter.literal(*payload, data)?;
    }
    for callback in inputs.callbacks {
        emitter.callback(callback)?;
    }
    emitter.drain_helpers(&definition)?;
    emitter.manifested.settle_imports();
    let (module, manifested) = (emitter.module, emitter.manifested);
    let product = module.finish();
    let bytes = crate::object::finish(product)?;
    let file = ObjectFile::read(inputs.target, bytes)
        .map_err(|message| CodegenError::Object { message })?;
    glue_artifact(file, manifested, inputs.target, crate::TOOLCHAIN)
        .map_err(|problem| CodegenError::Manifest { problem })
}
