//! Frozen linker inputs selected by the host before runtime dispatch. Uses the
//! existing kit/resolver and S4 chunk transport; never supplies native authority.
use super::{
    link_tasks::{LinkExecution, NativeLinkExecutor},
    roots::RootProducts,
    *,
};
use crate::native::{Backend, TargetSpec, Toolchain};
use hirsel_artifact_store::{Action, Address, Domain};
use hirsel_engine::{LinkInput, store::ResultStore};
use serde::Serialize;
use std::{
    collections::BTreeMap,
    io::Read,
    path::{Component, Path, PathBuf},
};

const PLATFORM: Domain = Domain {
    kind: 0x4e00_0025,
    schema: 2,
};
#[derive(Clone, Copy, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum Launch {
    Standalone,
    Run,
    Tests,
}

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
    builtins: Option<Reference>,
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
        let mut resolved = crate::native::link::foreign_inputs(&kit, &target.triple, links)
            .map_err(|e| e.to_string())?;
        let tree = tools
            .lld
            .parent()
            .and_then(Path::parent)
            .ok_or("linker has no installed tree")?;
        // A thin archive's children are real linker inputs. Freeze every
        // member, then let LLVM's archiver produce the equivalent self-contained
        // archive in original member order. No original path survives admission.
        let normalized = crate::native::executable::Scratch::new().map_err(|e| e.to_string())?;
        for (index, path) in resolved.iter_mut().enumerate() {
            let mut file = std::fs::File::open(&*path).map_err(|e| e.to_string())?;
            let mut prefix = [0u8; 8];
            file.read_exact(&mut prefix).map_err(|e| e.to_string())?;
            if &prefix == b"!<thin>\n" {
                *path = freeze_thin_archive(
                    store.as_ref(),
                    tools,
                    path,
                    &normalized.0.join(index.to_string()),
                )?;
            }
        }
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
        if let Some(path) = tools.builtins.get(&target.triple) {
            files.insert("runtime/libhirsel_builtins.a".into(), (path.clone(), false));
        }
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
        let builtins = captured
            .iter()
            .find(|file| file.path == "runtime/libhirsel_builtins.a")
            .map(|file| file.content);
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
            2u32,
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
        selected.products = store.clone();
        selected.lld = directory.0.join("llvm/bin/ld.lld");
        selected.kit = directory.0.join("kit");
        selected.runtime.clear();
        selected.runtime_debug.clear();
        selected.builtins.clear();
        if builtins.is_some() {
            selected.builtins.insert(
                target.triple.clone(),
                directory.0.join("runtime/libhirsel_builtins.a"),
            );
        }
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
            builtins,
            foreign,
            foreign_files,
            _directory: directory,
        }))
    }
    pub fn manifest(&self) -> Reference {
        self.manifest
    }
    pub fn runtime_archives(&self) -> Vec<Reference> {
        std::iter::once(self.runtime).chain(self.builtins).collect()
    }
    pub fn foreign_inputs(&self) -> &[Reference] {
        &self.foreign
    }
    pub fn linker_options(
        &self,
        backend: Backend,
        entry: &hirsel_syntax::DefPath,
    ) -> Result<ContentHash, String> {
        self.linker_options_with_debug(backend, entry, false)
    }
    pub fn linker_options_with_debug(
        &self,
        backend: Backend,
        entry: &hirsel_syntax::DefPath,
        debug_sources: bool,
    ) -> Result<ContentHash, String> {
        self.linker_options_mode(backend, entry, debug_sources, Launch::Standalone)
    }
    pub fn linker_options_for_run(
        &self,
        backend: Backend,
        entry: &hirsel_syntax::DefPath,
    ) -> Result<ContentHash, String> {
        self.linker_options_mode(backend, entry, false, Launch::Run)
    }
    pub fn linker_options_for_tests(
        &self,
        backend: Backend,
        entry: &hirsel_syntax::DefPath,
    ) -> Result<ContentHash, String> {
        self.linker_options_mode(backend, entry, false, Launch::Tests)
    }
    fn linker_options_mode(
        &self,
        backend: Backend,
        entry: &hirsel_syntax::DefPath,
        debug_sources: bool,
        launch: Launch,
    ) -> Result<ContentHash, String> {
        let execution = match self.tools.execution {
            crate::native::Execution::Executable => "executable",
            crate::native::Execution::Runner => "runner",
            crate::native::Execution::RunnerExec => "runner-exec",
        };
        let bytes = serde_json::to_vec(&(
            self.manifest,
            backend.name(),
            entry,
            hirsel_lir::ABI_VERSION,
            hirsel_lir::RUNTIME_ABI_VERSION,
            launch,
            execution,
            // Current source/anchor contents are the root launch manifest's
            // explicit inputs; keep them out of this logical options address.
            debug_sources,
        ))
        .map_err(|e| e.to_string())?;
        Ok(ContentHash::of_parts(
            "hirsel.native.frozen-platform-link.v3",
            [bytes.as_slice()],
        ))
    }
    pub fn executor(
        self: &Arc<Self>,
        backend: Backend,
        entry: hirsel_syntax::DefPath,
        directory: PathBuf,
    ) -> Arc<dyn NativeLinkExecutor> {
        self.executor_debug(backend, entry, directory, false)
    }
    pub fn executor_debug(
        self: &Arc<Self>,
        backend: Backend,
        entry: hirsel_syntax::DefPath,
        directory: PathBuf,
        debug_sources: bool,
    ) -> Arc<dyn NativeLinkExecutor> {
        Arc::new(Executor {
            platform: self.clone(),
            backend,
            entry,
            directory,
            debug_sources,
            launch: Launch::Standalone,
        })
    }
    pub fn executor_for_run(
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
            debug_sources: false,
            launch: Launch::Run,
        })
    }
    pub fn executor_for_tests(
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
            debug_sources: false,
            launch: Launch::Tests,
        })
    }
    pub fn launch_test(
        &self,
        runnable: &crate::native::Runnable,
        directory: &Path,
        index: usize,
        scheduler: Option<&hirsel_run_schema::Scheduler>,
    ) -> Result<(crate::native::Launched, std::time::Duration), crate::native::NativeError> {
        crate::native::link::launch_test(
            runnable,
            &self.target,
            &self.tools,
            directory,
            index,
            scheduler,
        )
    }
    /// Restore the selected output and recheck its existing executable/image
    /// protocol. Keep this platform alive through launch: it owns the frozen
    /// linker/kit/runtime files used by its toolchain.
    pub fn runnable(
        &self,
        product: &crate::native::object_queries::LinkedProduct,
        destination: &Path,
    ) -> Result<crate::native::Runnable, String> {
        crate::native::object_queries::materialize_executable(
            self.store.as_ref(),
            product,
            destination,
        )?;
        crate::native::runner::restore_runnable(&self.tools, &self.target, destination)
            .map_err(|e| e.to_string())
    }
    pub fn toolchain(&self) -> &Toolchain {
        &self.tools
    }
}
// Prior art: llvm-project 2b6e769dd3add8bbd8d8be60e59f8d7e5f30ec54,
// llvm/tools/llvm-ar/llvm-ar.cpp:799-815,1008-1021: normal archives retain
// basename member names and quick append preserves input order/multiplicity.
// Object's existing checked archive reader resolves the thin member inventory;
// LLVM writes the actual format and symbol index. No custom archive codec.
fn freeze_thin_archive(
    store: &dyn ResultStore,
    tools: &Toolchain,
    path: &Path,
    directory: &Path,
) -> Result<PathBuf, String> {
    use std::os::unix::ffi::OsStrExt;
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    let archive =
        object::read::archive::ArchiveFile::parse(bytes.as_slice()).map_err(|e| e.to_string())?;
    if !archive.is_thin() {
        return Err("expected an admitted thin archive".into());
    }
    let mut members = Vec::new();
    for member in archive.members() {
        let member = member.map_err(|e| e.to_string())?;
        if !member.is_thin() {
            return Err("unexpected embedded thin archive member".into());
        }
        let name = std::ffi::OsStr::from_bytes(member.name());
        let source = path
            .parent()
            .ok_or("thin archive has no parent")?
            .join(name);
        let file = std::fs::File::open(&source)
            .map_err(|e| format!("thin archive member {}: {e}", source.display()))?;
        if !file.metadata().map_err(|e| e.to_string())?.is_file() {
            return Err("thin member is not a regular file".into());
        }
        let reference = crate::native::object_queries::store_link_input_reader(store, file)?;
        members.push((member.name().to_vec(), reference));
    }
    let tree = tools
        .lld
        .parent()
        .and_then(Path::parent)
        .ok_or("archiver has no installed tree")?;
    let transformation =
        crate::native::link::runtime_derivation(tree, "llvm-ar").map_err(|e| e.to_string())?;
    let encoding =
        serde_json::to_vec(&(1u32, transformation, &members)).map_err(|e| e.to_string())?;
    let domain = Domain {
        kind: 0x4f00_0009,
        schema: 1,
    };
    let address = Address(ContentHash::of_parts(
        "hirsel.native.thin-archive-address.v1",
        [encoding.as_slice()],
    ));
    let action = Action(address.0);
    let output = directory.join("archive.a");
    if let Some(row) = store.lookup_product(address)
        && row.action == action
        && row.reference.domain == domain
        && let Some(record) = store.get_content(row.reference)
        && domain.reference(&record) == Some(row.reference)
        && let Ok(reference) = serde_json::from_slice::<Reference>(&record)
        && crate::native::object_queries::materialize_link_input(store, reference, &output, false)
            .is_ok()
    {
        return Ok(output);
    }
    std::fs::create_dir_all(directory).map_err(|e| e.to_string())?;
    match std::fs::remove_file(&output) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.to_string()),
    }
    let mut inputs = Vec::new();
    for (index, (name, reference)) in members.iter().enumerate() {
        let basename = Path::new(std::ffi::OsStr::from_bytes(name))
            .file_name()
            .ok_or("thin member has no filename")?;
        let destination = directory
            .join("members")
            .join(index.to_string())
            .join(basename);
        crate::native::object_queries::materialize_link_input(
            store,
            *reference,
            &destination,
            false,
        )?;
        inputs.push(destination);
    }
    // Bound each argv independently of member count. q appends duplicates in
    // exact order; the final s operation writes LLVM's ordinary symbol index.
    if inputs.is_empty() {
        let mut command = archiver(tree);
        crate::native::link::checked(
            crate::native::StageName::Link,
            command.args(["qcD", "--format=gnu"]).arg(&output),
        )
        .map_err(|e| e.to_string())?;
    } else {
        for group in inputs.chunks(64) {
            let mut command = archiver(tree);
            crate::native::link::checked(
                crate::native::StageName::Link,
                command
                    .args(["qcDS", "--format=gnu"])
                    .arg(&output)
                    .args(group),
            )
            .map_err(|e| e.to_string())?;
        }
        let mut command = archiver(tree);
        crate::native::link::checked(
            crate::native::StageName::Link,
            command.arg("sD").arg(&output),
        )
        .map_err(|e| e.to_string())?;
    }
    let reference = crate::native::object_queries::store_link_input_reader(
        store,
        std::fs::File::open(&output).map_err(|e| e.to_string())?,
    )?;
    let bytes = serde_json::to_vec(&reference).map_err(|e| e.to_string())?;
    let record = store
        .put_content(domain, &bytes)
        .ok_or("normalized archive publication failed")?;
    if domain.reference(&bytes) != Some(record) {
        return Err("normalized archive identity differs".into());
    }
    store.associate_product(address, action, record);
    if store
        .lookup_product(address)
        .is_none_or(|row| row.action != action || row.reference != record)
    {
        return Err("normalized archive association failed".into());
    }
    Ok(output)
}
fn archiver(tree: &Path) -> std::process::Command {
    let mut command = std::process::Command::new(tree.join("bin/llvm-ar"));
    command.env_clear().env("LD_LIBRARY_PATH", tree.join("lib"));
    command
}

struct Executor {
    platform: Arc<FrozenPlatform>,
    backend: Backend,
    entry: hirsel_syntax::DefPath,
    directory: PathBuf,
    debug_sources: bool,
    launch: Launch,
}
impl NativeLinkExecutor for Executor {
    fn link(&self, native: &mut RootProducts, input: LinkExecution<'_>) -> Result<Vec<u8>, String> {
        let platform = &self.platform;
        if input.key.target != platform.target.digest()
            || input.key.linker_options
                != platform.linker_options_mode(
                    self.backend,
                    &self.entry,
                    self.debug_sources,
                    self.launch,
                )?
            || input.inputs.runtime_archives != platform.runtime_archives()
            || input.inputs.foreign_inputs != platform.foreign
            || platform.store.get_content(platform.manifest).is_none()
        {
            return Err("Link inputs differ from the selected frozen platform".into());
        }
        std::fs::create_dir_all(&self.directory).map_err(|e| e.to_string())?;
        let sources = if self.debug_sources {
            Some(native.profile_sources(input.root)?)
        } else {
            None
        };
        let requested = native.requested_run(input.root)?;
        let tests = native.test_entries(input.root)?;
        let matches = match self.launch {
            Launch::Standalone => requested.is_none() && tests.is_empty(),
            Launch::Run => {
                tests.is_empty()
                    && requested
                        .as_ref()
                        .is_some_and(|(entry, _)| entry == &self.entry)
            }
            Launch::Tests => {
                requested.is_none() && tests.first().is_some_and(|(entry, _)| entry == &self.entry)
            }
        };
        if !matches {
            return Err("Link launch differs from the selected root inputs".into());
        }
        native.with_link_context(input.root, input.ready, |program| {
            crate::native::check_runtime_calls(program).map_err(|e| e.to_string())?;
            let registry = hirsel_lir::descriptor_registry::DescriptorRegistry::new(
                &program.table,
                &program.finalizers,
            );
            let mut objects = input.objects.to_vec();
            objects.push(
                crate::native::emit_glue(program, self.backend, &registry)
                    .map_err(|e| e.to_string())?,
            );
            if let Some(sources) = &sources {
                objects.push(
                    crate::native::link::debug_artifact(program, &objects, sources)
                        .map_err(|e| e.to_string())?,
                );
            }
            let path = if self.launch == Launch::Tests {
                match crate::native::link::link_tests_frozen(
                    program,
                    &tests,
                    &objects,
                    &platform.tools,
                    &self.directory,
                    &platform.foreign_files,
                )
                .map_err(|e| e.to_string())?
                {
                    crate::native::Runnable::Executable(path) => path,
                    crate::native::Runnable::Image(image) => image.path,
                }
            } else if let Some((_, run)) = &requested {
                match crate::native::link::link_runnable_frozen(
                    program,
                    &self.entry,
                    &objects,
                    &platform.tools,
                    &self.directory,
                    run,
                    &platform.foreign_files,
                )
                .map_err(|e| e.to_string())?
                {
                    crate::native::Runnable::Executable(path) => path,
                    crate::native::Runnable::Image(image) => image.path,
                }
            } else {
                crate::native::link::link_standalone_frozen(
                    program,
                    &self.entry,
                    &objects,
                    &platform.tools,
                    &self.directory,
                    &platform.foreign_files,
                )
                .map_err(|e| e.to_string())?
            };
            std::fs::read(path).map_err(|e| e.to_string())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native::{OptLevel, RuntimeBuild};

    fn object(target: &TargetSpec, name: &str, value: u64) -> Vec<u8> {
        hirsel_llvm::compile_ir(
            &format!(
                "target triple = \"{}\"\ndefine i64 @{name}() {{ ret i64 {value} }}",
                target.triple
            ),
            target,
            OptLevel::None,
        )
        .unwrap()
        .bytes
    }
    #[test]
    fn frozen_thin_archives_preserve_duplicate_members_symbols_and_real_link_inputs() {
        hirsel_engine::with_compiler_stack(|| {
            let root = tempfile::tempdir().unwrap();
            let target = TargetSpec::x86_64_unknown_linux_gnu();
            let store = Arc::new(hirsel_engine::store::DirStore::new(
                root.path().join("products"),
            ));
            let tools = Toolchain::in_tree_with_products(
                &root.path().join("runtime"),
                &[&target],
                &[RuntimeBuild::default()],
                None,
                store.clone(),
            )
            .unwrap();
            let tree = tools.lld.parent().unwrap().parent().unwrap();
            let source = root.path().join("original with spaces");
            let first = source.join("first/member.o");
            let second = source.join("second/member.o");
            std::fs::create_dir_all(first.parent().unwrap()).unwrap();
            std::fs::create_dir_all(second.parent().unwrap()).unwrap();
            std::fs::write(&first, object(&target, "first", 20)).unwrap();
            std::fs::write(&second, object(&target, "second", 22)).unwrap();
            let thin = source.join("thin.a");
            let mut command = archiver(tree);
            crate::native::link::checked(
                crate::native::StageName::Link,
                command.current_dir(&source).args([
                    "qcTD",
                    "thin.a",
                    "first/member.o",
                    "second/member.o",
                ]),
            )
            .unwrap();
            let header = std::fs::read(&thin).unwrap();
            let frozen =
                freeze_thin_archive(store.as_ref(), &tools, &thin, &root.path().join("one"))
                    .unwrap();
            let bytes = std::fs::read(&frozen).unwrap();
            let parsed = object::read::archive::ArchiveFile::parse(bytes.as_slice()).unwrap();
            assert!(!parsed.is_thin());
            assert_eq!(
                parsed
                    .members()
                    .map(|member| member.unwrap().name().to_vec())
                    .collect::<Vec<_>>(),
                vec![b"member.o".to_vec(), b"member.o".to_vec()]
            );
            let symbols = parsed
                .symbols()
                .unwrap()
                .unwrap()
                .map(|symbol| symbol.unwrap().name().to_vec())
                .collect::<std::collections::BTreeSet<_>>();
            assert!(
                symbols.contains(b"first".as_slice()) && symbols.contains(b"second".as_slice())
            );
            let repeated =
                freeze_thin_archive(store.as_ref(), &tools, &thin, &root.path().join("two"))
                    .unwrap();
            assert_eq!(std::fs::read(repeated).unwrap(), bytes);
            // Same thin header, edited external member: a different ordinary
            // transformation output, without any stale-header/path-only hit.
            std::fs::write(&first, object(&target, "first", 21)).unwrap();
            assert_eq!(std::fs::read(&thin).unwrap(), header);
            let changed =
                freeze_thin_archive(store.as_ref(), &tools, &thin, &root.path().join("changed"))
                    .unwrap();
            assert_ne!(std::fs::read(&changed).unwrap(), bytes);
            std::fs::remove_file(&second).unwrap();
            assert!(
                freeze_thin_archive(store.as_ref(), &tools, &thin, &root.path().join("missing"))
                    .is_err()
            );
            std::fs::remove_dir_all(&source).unwrap();
            let caller = hirsel_llvm::compile_ir(
                &format!("target triple = \"{}\"\ndeclare i64 @first()\ndeclare i64 @second()\ndefine i32 @main() {{\n %a = call i64 @first()\n %b = call i64 @second()\n %sum = add i64 %a, %b\n %exit = trunc i64 %sum to i32\n ret i32 %exit\n}}", target.triple),
                &target, OptLevel::None,
            ).unwrap();
            let caller_path = root.path().join("caller.o");
            std::fs::write(&caller_path, caller.bytes).unwrap();
            for (archive, expected) in [(&frozen, 42), (&changed, 43)] {
                let executable = root.path().join(format!("program-{expected}"));
                let kit = crate::native::kit::Kit::load(&tools.kit).unwrap();
                let objects = [caller_path.clone()];
                let foreign = [archive.clone()];
                let link = crate::native::kit::Link {
                    kit: &kit,
                    triple: &target.triple,
                    kind: crate::native::kit::Kind::Executable { loader: None },
                    objects: &objects,
                    runtime: None,
                    unwind: false,
                    foreign: &foreign,
                };
                let (mut command, _) = link.command(&tools.lld, &executable).unwrap();
                crate::native::link::checked(crate::native::StageName::Link, &mut command).unwrap();
                assert_eq!(
                    std::process::Command::new(executable)
                        .status()
                        .unwrap()
                        .code(),
                    Some(expected)
                );
            }
        });
    }
}
