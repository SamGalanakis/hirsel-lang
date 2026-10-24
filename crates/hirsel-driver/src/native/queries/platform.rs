//! Frozen linker inputs selected by the host before runtime dispatch. Uses the
//! existing kit/resolver and S4 chunk transport; never supplies native authority.
use super::{
    link_tasks::{LinkExecution, NativeLinkExecutor},
    roots::{RootLinkInputs, RootProducts},
    *,
};
use crate::native::{Backend, TargetSpec, Toolchain};
use hirsel_artifact_store::Domain;
use hirsel_engine::{LinkInput, store::ResultStore};
use serde::Serialize;
use std::{
    collections::BTreeMap,
    io::Read,
    path::{Component, Path, PathBuf},
};

const PLATFORM: Domain = Domain {
    kind: 0x4e00_0025,
    schema: 1,
};
#[derive(Serialize)]
struct File {
    path: String,
    content: Reference,
    executable: bool,
}
/// Owns a private, immutable copy of every selected linker-tree/kit/archive
/// file. Source paths are consumed only during capture and absent from its
/// durable manifest. The allowance for platform bytes remains the selected
/// store/host admission's responsibility, not proof inferred from this owner.
pub struct FrozenPlatform {
    store: Arc<dyn ResultStore>,
    tools: Toolchain,
    target: TargetSpec,
    manifest: Reference,
    runtime: Reference,
    foreign: Vec<Reference>,
    foreign_files: Vec<PathBuf>,
    _directory: crate::native::executable::Scratch,
}
impl FrozenPlatform {
    pub fn capture(
        store: Arc<dyn ResultStore>,
        tools: &Toolchain,
        target: &TargetSpec,
        links: &[LinkInput],
    ) -> Result<Arc<Self>, String> {
        let directory = crate::native::executable::Scratch::new().map_err(|e| e.to_string())?;
        let kit = crate::native::kit::Kit::load(&tools.kit).map_err(|e| e.to_string())?;
        let resolved = crate::native::link::foreign_inputs(&kit, &target.triple, links)
            .map_err(|e| e.to_string())?;
        let tree = tools
            .lld
            .parent()
            .and_then(Path::parent)
            .ok_or("linker has no installed tree")?;
        let mut files = BTreeMap::<String, (PathBuf, bool)>::new();
        files.insert("llvm/bin/ld.lld".into(), (tools.lld.clone(), true));
        // bootstrap_llvm.py's self-contained tree ships the dynamic runtime
        // closure as top-level .so files; clang resources/headers aren't linker
        // inputs. Alias files retain their logical names and deduplicate bytes.
        for entry in std::fs::read_dir(tree.join("lib")).map_err(|e| e.to_string())? {
            let path = entry.map_err(|e| e.to_string())?.path();
            let name = path
                .file_name()
                .and_then(|name| name.to_str())
                .ok_or("non-UTF8 tool library name")?;
            if path.is_file() && (name.ends_with(".so") || name.contains(".so.")) {
                files.insert(format!("llvm/lib/{name}"), (path, false));
            }
        }
        for path in kit.link_files(&target.triple).map_err(|e| e.to_string())? {
            let relative = path
                .strip_prefix(kit.root())
                .map_err(|_| "kit input is outside its root")?;
            let relative = relative.to_str().ok_or("non-UTF8 kit input path")?;
            files.insert(format!("kit/{relative}"), (path, false));
        }
        let archive = tools
            .archive(&target.triple)
            .ok_or("missing selected runtime archive")?;
        let archive =
            crate::native::link::runtime_archive(tools, &target.triple, archive, tools.debug_info)
                .map_err(|e| e.to_string())?;
        files.insert("runtime/libhirsel_rt.a".into(), (archive.into(), false));
        for (index, path) in resolved.iter().enumerate() {
            let mut file = std::fs::File::open(path).map_err(|e| e.to_string())?;
            let mut prefix = [0u8; 8];
            file.read_exact(&mut prefix).map_err(|e| e.to_string())?;
            if &prefix != b"!<arch>\n" {
                if !prefix.starts_with(b"\x7fELF") {
                    return Err("a foreign linker script/thin archive needs an explicit transitive file closure".into());
                }
                crate::native::runner::frozen_foreign_name(
                    &std::fs::read(path).map_err(|e| e.to_string())?,
                )
                .map_err(|e| e.to_string())?;
            }
            files.insert(format!("foreign/{index}/input"), (path.clone(), false));
        }
        let mut captured = Vec::new();
        let mut copies = BTreeMap::new();
        for (relative, (source, executable)) in files {
            if Path::new(&relative)
                .components()
                .any(|part| !matches!(part, Component::Normal(_)))
            {
                return Err("noncanonical frozen platform path".into());
            }
            let file = std::fs::File::open(source).map_err(|e| e.to_string())?;
            if !file.metadata().map_err(|e| e.to_string())?.is_file() {
                return Err("platform input is not a regular file".into());
            }
            let content =
                crate::native::object_queries::store_link_input_reader(store.as_ref(), file)?;
            let destination = directory.0.join(&relative);
            if let Some(previous) = copies.get(&(content.output.0, executable)) {
                std::fs::create_dir_all(destination.parent().unwrap())
                    .map_err(|e| e.to_string())?;
                std::fs::hard_link(previous, &destination).map_err(|e| e.to_string())?;
            } else {
                crate::native::object_queries::materialize_link_input(
                    store.as_ref(),
                    content,
                    &destination,
                    executable,
                )?;
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(
                    &destination,
                    std::fs::Permissions::from_mode(if executable { 0o555 } else { 0o444 }),
                )
                .map_err(|e| e.to_string())?;
                copies.insert((content.output.0, executable), destination);
            }
            captured.push(File {
                path: relative,
                content,
                executable,
            });
        }
        let runtime = captured
            .iter()
            .find(|file| file.path == "runtime/libhirsel_rt.a")
            .unwrap()
            .content;
        let foreign = (0..resolved.len())
            .map(|index| {
                captured
                    .iter()
                    .find(|file| file.path == format!("foreign/{index}/input"))
                    .unwrap()
                    .content
            })
            .collect();
        let foreign_files = (0..resolved.len())
            .map(|index| directory.0.join(format!("foreign/{index}/input")))
            .collect();
        let bytes = serde_json::to_vec(&(
            1u32,
            crate::COMPILER_IDENTITY,
            target.digest(),
            tools.debug_info,
            (
                tools.runtime_build.diagnostic,
                tools.runtime_build.system_allocator,
                tools.runtime_build.measurement,
                tools.runtime_build.memcheck,
            ),
            &captured,
        ))
        .map_err(|e| e.to_string())?;
        let manifest = store
            .put_content(PLATFORM, &bytes)
            .ok_or("frozen platform manifest publication failed")?;
        if PLATFORM.reference(&bytes) != Some(manifest) {
            return Err("frozen platform manifest identity differs".into());
        }
        let mut selected = tools.clone();
        selected.lld = directory.0.join("llvm/bin/ld.lld");
        selected.kit = directory.0.join("kit");
        selected.runtime.clear();
        selected.runtime_debug.clear();
        let archive = directory.0.join("runtime/libhirsel_rt.a");
        selected.runtime.insert(
            (target.triple.clone(), tools.runtime_build),
            archive.clone(),
        );
        selected
            .runtime_debug
            .insert((target.triple.clone(), tools.runtime_build), archive);
        Ok(Arc::new(Self {
            store,
            tools: selected,
            target: target.clone(),
            manifest,
            runtime,
            foreign,
            foreign_files,
            _directory: directory,
        }))
    }
    pub fn manifest(&self) -> Reference {
        self.manifest
    }
    pub fn runtime_archives(&self) -> Vec<Reference> {
        vec![self.runtime]
    }
    pub fn foreign_inputs(&self) -> &[Reference] {
        &self.foreign
    }
    pub fn linker_options(
        &self,
        backend: Backend,
        entry: &hirsel_syntax::DefPath,
    ) -> Result<ContentHash, String> {
        let bytes = serde_json::to_vec(&(
            self.manifest,
            backend.name(),
            entry,
            hirsel_lir::ABI_VERSION,
            hirsel_lir::RUNTIME_ABI_VERSION,
            "standalone",
        ))
        .map_err(|e| e.to_string())?;
        Ok(ContentHash::of_parts(
            "hirsel.native.frozen-platform-link.v1",
            [bytes.as_slice()],
        ))
    }
    pub fn executor(
        self: &Arc<Self>,
        backend: Backend,
        entry: hirsel_syntax::DefPath,
        directory: PathBuf,
    ) -> Arc<dyn NativeLinkExecutor> {
        Arc::new(Executor {
            platform: self.clone(),
            backend,
            entry,
            directory,
        })
    }
}
struct Executor {
    platform: Arc<FrozenPlatform>,
    backend: Backend,
    entry: hirsel_syntax::DefPath,
    directory: PathBuf,
}
impl NativeLinkExecutor for Executor {
    fn link(&self, native: &mut RootProducts, input: LinkExecution<'_>) -> Result<Vec<u8>, String> {
        let platform = &self.platform;
        if input.key.target != platform.target.digest()
            || input.key.linker_options != platform.linker_options(self.backend, &self.entry)?
            || input.inputs.runtime_archives != vec![platform.runtime]
            || input.inputs.foreign_inputs != platform.foreign
            || platform.store.get_content(platform.manifest).is_none()
        {
            return Err("Link inputs differ from the selected frozen platform".into());
        }
        std::fs::create_dir_all(&self.directory).map_err(|e| e.to_string())?;
        native.link_standalone(
            input.root,
            input.ready,
            RootLinkInputs {
                entry: &self.entry,
                backend: self.backend,
                objects: input.objects,
                toolchain: &platform.tools,
                directory: &self.directory,
            },
            &platform.foreign_files,
        )
    }
}
