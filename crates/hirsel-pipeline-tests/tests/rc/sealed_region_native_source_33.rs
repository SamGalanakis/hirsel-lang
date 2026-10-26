//! Original source through independently checked Ready, real backend objects,
//! and the real runtime. The two component bodies remain unregistered.
use super::sealed_region_h_source_33::original;
use hirsel_driver::native::{Backend, RuntimeBuild, Toolchain, c_call};
use hirsel_lir::emission::{EmitInputs, GlueInputs, OptLevel};
use hirsel_lir::layout_types::SizeSpec;
use hirsel_lir::operation::{NativeType, Primitive};
use hirsel_lir::stage::InputIdentity;
use hirsel_types::TypeKind;
use std::collections::{BTreeMap, BTreeSet};
use std::process::Command;

fn execute(source: &str, root: TypeKind, native: NativeType) {
    let (h, declarations, manifest) = original(source, root, native);
    let mut raw = vec![];
    for unit in h.units {
        let ready =
            hirsel_rc::ready::ready(unit, &h.layouts, &hirsel_rc::cleanup::Finalizers::default())
                .unwrap();
        raw.push(ready);
    }
    hirsel_lir::validate::ScopedQualification::new(&h.target, &manifest, &declarations, None)
        .qualify(&raw, &h.layouts)
        .unwrap();
    let inputs = hirsel_lir::validate::Inputs {
        target: &h.target,
        layouts: &h.layouts,
        manifest: &manifest,
        operations: &declarations,
        discharge: None,
        memo: Default::default(),
    };
    let units: Vec<_> = raw
        .into_iter()
        .map(|unit| {
            hirsel_lir::validate::validate_ready(unit, &inputs)
                .unwrap_or_else(|e| panic!("Ready C: {:?}", e.violations))
        })
        .collect();
    let keys = hirsel_lir::LayoutKeys::new(&h.layouts);
    let mut glue = hirsel_lir::glue::UnitGlue::default();
    for unit in &units {
        glue.extend(
            &hirsel_lir::glue::UnitGlue::of(unit.unit(), &h.layouts, &keys, &manifest).unwrap(),
        );
    }
    let failures = hirsel_lir::glue::failure_codes(&glue.failure_rules);
    let finalizers = BTreeMap::new();
    let registry =
        hirsel_lir::descriptor_registry::DescriptorRegistry::new(&h.layouts, &finalizers);
    let glue_inputs = GlueInputs {
        target: &h.target,
        layouts: &h.layouts,
        keys: &keys,
        registry,
        finalizers: &finalizers,
        glue: &glue,
        failures: &failures,
        statics: &h.statics,
        callbacks: &[],
        foreign: &hirsel_lir::emission::NO_FOREIGN,
        aliasing_unstated: false,
    };
    let input = InputIdentity {
        interface_version: hirsel_lir::INTERFACE_VERSION,
        digest: hirsel_syntax::ContentHash::of_bytes(source.as_bytes()),
    };
    let empty = BTreeSet::new();
    let emit = EmitInputs {
        ahead: None,
        continuation: None,
        alternate: false,
        index: None,
        input: &input,
        target: &h.target,
        layouts: &h.layouts,
        keys: &keys,
        manifest: &manifest,
        statics: &h.statics,
        inline_instances: &empty,
        available_externally: &empty,
        callbacks: false,
        tracking: true,
        foreign: &hirsel_lir::emission::NO_FOREIGN,
        opt: OptLevel::None,
        verify_backend: true,
        measurement: false,
        fuel: false,
    };
    let entry = units
        .iter()
        .map(|u| u.unit())
        .find(|u| u.header.definition == crate::rc_support::path("main"))
        .unwrap();
    let abi = &h.layouts.fn_abis[entry.header.abi.0 as usize];
    let result = abi.result.as_ref().unwrap().layout;
    let SizeSpec::Fixed(size) = h.layouts.layouts[result.0 as usize].size else {
        panic!("fixed source result");
    };
    let symbol =
        hirsel_lir::emission::symbol_name(&entry.header.definition, Some(&entry.header.instance));
    let call = c_call(&h.layouts, abi, &symbol, "call_main");
    let root = tempfile::tempdir().unwrap();
    let builds = [
        RuntimeBuild::default(),
        RuntimeBuild {
            diagnostic: true,
            ..Default::default()
        },
    ];
    let tools =
        Toolchain::in_tree_with_runtimes(&root.path().join("runtime"), &[&h.target], &builds)
            .unwrap();
    let refs: Vec<_> = units.iter().collect();
    for backend in [Backend::Llvm, Backend::Cranelift] {
        let (object, glue) = match backend {
            Backend::Llvm => (
                hirsel_llvm::emit_object(&refs, &emit).unwrap(),
                hirsel_llvm::emit_glue(&glue_inputs).unwrap(),
            ),
            Backend::Cranelift => (
                hirsel_cranelift::emit_object(&refs, &emit).unwrap(),
                hirsel_cranelift::emit_glue(&glue_inputs).unwrap(),
            ),
        };
        let object_path = root.path().join(format!("{}.o", backend.name()));
        let glue_path = root.path().join(format!("{}-glue.o", backend.name()));
        std::fs::write(&object_path, &object.bytes).unwrap();
        std::fs::write(&glue_path, &glue.bytes).unwrap();
        for build in builds {
            let harness = root.path().join("probe.c");
            let text = HARNESS
                .replace("@CALL@", &call)
                .replace("@SIZE@", &size.max(1).to_string())
                .replace(
                    "@FLAGS@",
                    if build.diagnostic {
                        "HIRSEL_RT_V2_CONFIG_DIAGNOSTIC"
                    } else {
                        "0"
                    },
                );
            std::fs::write(&harness, text).unwrap();
            let exe = root
                .path()
                .join(format!("{}-{}", backend.name(), build.diagnostic));
            let selected = tools.with_runtime(build);
            let out = Command::new(&tools.clang)
                .args(["-std=c11", "-DHIRSEL_RT_ENABLE_STATS"])
                .arg("-I")
                .arg(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../hirsel-rt/include"))
                .arg(&harness)
                .arg(&object_path)
                .arg(&glue_path)
                .arg(selected.archive(&h.target.triple).unwrap())
                .args([
                    "-lgcc_s",
                    "-lutil",
                    "-lrt",
                    "-lpthread",
                    "-lm",
                    "-ldl",
                    "-o",
                ])
                .arg(&exe)
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "{backend:?}: {}",
                String::from_utf8_lossy(&out.stderr)
            );
            let out = Command::new(exe).output().unwrap();
            assert!(
                out.status.success(),
                "{backend:?} diagnostic={}: {}",
                build.diagnostic,
                String::from_utf8_lossy(&out.stderr)
            );
        }
    }
}
const HARNESS: &str = r#"
#include "hirsel_rt_v2.h"
#include <assert.h>
#include <stdint.h>
#include <string.h>
@CALL@
struct program_binding { const hirsel_rt_v2_layout *const *layouts; uint64_t count; };
extern const struct program_binding program __asm__("hirsel.program.registry");
int main(void) {
    hirsel_rt_v2_config config={.abi_version=HIRSEL_RT_V2_ABI_VERSION,.flags=@FLAGS@,
        .layouts=program.layouts,.layout_count=(uint32_t)program.count};
    hirsel_rt_v2_ctx *ctx=0;
    assert(hirsel_rt_v2_ctx_create(&config,&ctx)==0);
    _Alignas(16) unsigned char result[@SIZE@]; memset(result,0xff,sizeof result);
    uint64_t test=1;
    assert(call_main(ctx,0,result,&test)==0);
    hirsel_rt_v2_audit audit;
    hirsel_rt_v2_task_exit(ctx,&audit);
    assert(audit.live_blocks==0 && audit.retained_bytes==0 && audit.active_regions==0);
    hirsel_rt_v2_ctx_destroy(ctx);
}
"#;
#[test]
fn original_s4_unit_source_executes_on_both_native_backends_and_runtime_builds() {
    execute(
        include_str!("../../../../conformance/MEM/MEM-4-sealed-empty-unit-run.hir"),
        TypeKind::Tuple(vec![].into()),
        NativeType::Tuple(vec![]),
    );
}
#[test]
fn original_s4_scalar_source_executes_on_both_native_backends_and_runtime_builds() {
    execute(
        include_str!("../../../../conformance/MEM/MEM-4-sealed-scalar-zero-run.hir"),
        TypeKind::Primitive(hirsel_types::Primitive::U64),
        NativeType::Primitive(Primitive::U64),
    );
}
