//! Measurement-only call sites. Keep LLVM's site identity and the runtime's existing
//! `ProfileSite` layout; generated helpers inherit the surrounding call's context.
use cranelift_codegen::ir::{self, AbiParam, types};
use cranelift_module::{DataDescription, DataId, FuncId, Linkage, Module};
use hirsel_lir::ir::{LirUnit, SiteRange};
use hirsel_lir::profile::ProfileSource;
use hirsel_lir::stage::{ObjectSymbolKind, Ready};
use hirsel_syntax::ContentHash;

use crate::CodegenError;
use crate::module::{Emitted, Emitter, PTR};

/// Object declarations, not SSA values: also valid in deferred failure/cleanup blocks.
#[derive(Clone, Copy)]
pub(crate) struct CallSite {
    pub(crate) metadata: DataId,
    pub(crate) enter: FuncId,
    pub(crate) leave: FuncId,
}

/// Port of `hirsel-llvm/src/emit/blocks.rs::profile_location`. Both supported
/// targets use little-endian, 64-bit pointers and the same C struct layout.
pub(crate) fn call_site(
    em: &mut Emitter<'_>,
    unit: &LirUnit<Ready>,
    source: &ProfileSource,
    site: SiteRange,
    operation: &str,
) -> Emitted<CallSite> {
    let position = source
        .site_location(site)
        .ok_or_else(|| CodegenError::Invalid {
            definition: unit.header.definition.clone(),
            what: "profile anchor is outside its source revision".into(),
        })?;
    let (file, _) = source.source(site.source).expect("resolved source");
    let digest = ContentHash::of_parts(
        "hirsel.profile-site.v2",
        [
            unit.header.definition.fingerprint().0.as_slice(),
            unit.header.implementation_revision.0.as_slice(),
            &site.source.to_le_bytes(),
            &site.start.to_le_bytes(),
            &site.end.to_le_bytes(),
            operation.as_bytes(),
        ],
    );
    let id = u64::from_le_bytes(digest.0[..8].try_into().expect("eight bytes")).max(1);
    let name = format!("hirsel.profile.site.{id}");
    let (metadata, fresh) = em.declare_data(&name, Linkage::Local)?;
    if fresh {
        // ProfileSite: id, four string pointers, line, column. Relocations supply
        // the pointers; the record and strings are immutable for the whole run.
        let mut bytes = vec![0; 48];
        bytes[..8].copy_from_slice(&id.to_le_bytes());
        bytes[40..44].copy_from_slice(&position.line.to_le_bytes());
        bytes[44..48].copy_from_slice(&position.column.to_le_bytes());
        let mut description = DataDescription::new();
        description.define(bytes.into_boxed_slice());
        description.set_align(8);
        for (index, text) in [
            unit.header.definition.0.package.name.as_str(),
            &file.logical_path().to_string_lossy(),
            &unit.header.definition.to_string(),
            operation,
        ]
        .into_iter()
        .enumerate()
        {
            let string = em.constant_data(1, &[text.as_bytes(), &[0]].concat())?;
            let reference = em.module.declare_data_in_data(string, &mut description);
            description.write_data_addr(8 + index as u32 * 8, reference, 0);
        }
        em.define_data(&name, metadata, &description)?;
    }
    let enter = hook(
        em,
        "hirsel_rt_v2_profile_enter",
        &[PTR, PTR],
        Some(types::I64),
    )?;
    let leave = hook(em, "hirsel_rt_v2_profile_leave", &[PTR, types::I64], None)?;
    Ok(CallSite {
        metadata,
        enter,
        leave,
    })
}

fn hook(
    em: &mut Emitter<'_>,
    name: &str,
    params: &[ir::Type],
    result: Option<ir::Type>,
) -> Emitted<FuncId> {
    let mut signature = ir::Signature::new(em.call_conv);
    signature
        .params
        .extend(params.iter().copied().map(AbiParam::new));
    signature.returns.extend(result.map(AbiParam::new));
    em.manifest_import(name, ObjectSymbolKind::Runtime, None, None);
    em.import_function(name, &signature)
}
