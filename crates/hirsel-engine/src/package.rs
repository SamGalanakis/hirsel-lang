//! PROG-1: explicit source packages and the toolchain std for manifestless sources.
mod dependency_card;
pub(crate) mod diagnostic_guidance;
mod slots;
pub use dependency_card::render_dependency_cards;
pub use slots::{ImplementationSlot, PackageSlots};

use crate::*;
use hirsel_resolve::DependencyExposure;
use hirsel_syntax::PackageId;
use serde_json::{Map, Value};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

/// Select only explicitly declared root sources, using ordinary manifest
/// validation and canonical paths without visiting dependency manifests.
pub(crate) fn format_sources(path: &Path) -> Result<Vec<PathBuf>, ToolError> {
    if !path.extension().is_some_and(|e| e == "json") {
        return Ok(vec![path.to_path_buf()]);
    }
    let mut graph = Graph::default();
    let package = graph.select(path, None, None)?;
    let mut seen = BTreeSet::new();
    Ok(graph.packages[&package]
        .sources
        .iter()
        .map(|(path, _)| path.clone())
        .filter(|path| seen.insert(path.clone()))
        .collect())
}

/// A manifest-selected declaration graph, including libraries without an entry.
/// Loading parses and indexes sources; it never checks or evaluates a body.
pub struct PackageDeclarations {
    pub package: PackageId,
    pub engine: QueryEngine,
    pub files: Vec<(FileId, FileContext)>,
    /// Each selected package's effective implementation slots (PROG-1).
    pub slots: PackageSlots,
}
impl PackageDeclarations {
    /// Whether a declaration is visible from outside every selected package.
    pub fn is_exported(&self, definition: &DefPath) -> bool {
        let viewer = FileContext {
            // This reserved spelling cannot name a source package.
            package: PackageId {
                name: "$doc".into(),
                version: "0".into(),
            },
            modules: vec![],
        };
        self.engine
            .declarations
            .frozen()
            .index(self.package.clone())
            .access(&viewer, definition)
            .is_ok()
    }
    /// Resolve a header reference using the importing package's selected aliases
    /// and the compiler's ordinary PROG-2 access rules.
    pub fn lookup(
        &self,
        context: &FileContext,
        path: &hirsel_syntax::Path,
    ) -> Result<DefPath, hirsel_types::LookupError> {
        let index = self
            .engine
            .declarations
            .frozen()
            .index(context.package.clone());
        match index.lookup(context, path) {
            Err(hirsel_types::LookupError::Missing)
                if path.package.is_none() && path.segments.len() == 1 =>
            {
                let definition = registry::BuiltinRegistry::new()
                    .lookup_name(path)
                    .ok_or(hirsel_types::LookupError::Missing)?;
                index.access(context, &definition)
            }
            result => result,
        }
    }
}

pub(crate) fn load_package_declarations(
    compiler: &Compiler,
    manifest: &Path,
) -> Result<PackageDeclarations, ToolError> {
    let (graph, package) = Graph::program(manifest)?;
    let mut engine = compiler.engine(default_environment());
    // This API indexes declarations only. Keep the configured provider for
    // later demands, without querying or publishing parse-head products here.
    let result_store = engine.result_store.take();
    engine.declarations.inputs_mut().package_exposures = graph
        .packages
        .iter()
        .map(|(id, p)| (id.clone(), p.exposures.clone()))
        .collect();
    engine.declarations.inputs_mut().package_dependencies = graph
        .packages
        .iter()
        .map(|(id, p)| (id.clone(), p.dependencies.clone()))
        .collect();
    let mut files = Vec::new();
    let mut assigned = BTreeSet::new();
    for selected in graph.packages.values() {
        for (source, context) in &selected.sources {
            if !assigned.insert(source.clone()) {
                return Err(invalid("one source path has more than one FileContext"));
            }
            let file = engine
                .sources
                .add_logical(
                    source,
                    &selected.logical_sources[source],
                    crate::tools::read_source(source)?,
                )
                .ok_or(ToolError::SourceTooLarge)?;
            engine
                .index_source(file, context)
                .map_err(|error| ToolError::Query {
                    error,
                    phase: None,
                    sources: engine.sources.clone(),
                })?;
            files.push((file, context.clone()));
        }
    }
    slots::check_targets(&engine, &graph.slots)?;
    slots::check_conformance(&mut engine, &graph.slots)?;
    engine.result_store = result_store;
    Ok(PackageDeclarations {
        package,
        engine,
        files,
        slots: graph.slots,
    })
}

/// A selected program, with every source indexed before any header is queried.
pub struct LoadedProgram {
    pub engine: QueryEngine,
    pub files: Vec<(FileId, FileContext)>,
    pub entry: DefPath,
    has_launch_entry: bool,
    pub primary: FileId,
    /// The selected packages marked `unsafe ffi`, in package order (PROG-4).
    pub unsafe_ffi: Vec<UnsafeFfiPackage>,
    /// Each selected package's effective implementation slots (PROG-1).
    pub slots: PackageSlots,
    /// The files other than sources that loading read or probed: each package
    /// manifest and, for a source, its sidecar path. A change to any of them
    /// selects another package graph, so a resident session loads again.
    pub inputs: Vec<std::path::PathBuf>,
}
impl LoadedProgram {
    /// Check using the host's configured lowering policy.
    pub fn check_configured(&mut self) -> Result<CheckOutput, QueryError> {
        self.check(self.engine.configured_lowering)
    }
    /// Check every package of the program, lowering what `lowering` selects:
    /// the diagnostics do not depend on it. The root package lowers under
    /// `lowering`, each dependency only what compile-time evaluation executes,
    /// and `Reachable` then lowers the dependency bodies the root reaches.
    pub fn check(&mut self, lowering: LoweringPolicy) -> Result<CheckOutput, QueryError> {
        let _reads = self
            .engine
            .result_store
            .as_ref()
            .and_then(|store| store.read_scope());
        self.check_with(lowering, |_, _| Ok(()))
    }
    /// `check`, calling `checked` with each file that checked from source
    /// complete and with no diagnostics, in source order after S1/P2.
    fn check_with(
        &mut self,
        lowering: LoweringPolicy,
        mut checked: impl FnMut(&mut QueryEngine, FileId) -> Result<(), QueryError>,
    ) -> Result<CheckOutput, QueryError> {
        // The program check's own steps; per-definition checking and
        // lowering, GEN-5 and publication below are their own intervals.
        let _interval = hirsel_intern::timings::interval(hirsel_intern::timings::Interval::S0);
        let mut diagnostics = Vec::new();
        let mut completion = CheckCompletion::Complete;
        for (file, context) in &self.files {
            match crate::tools::association_diagnostics(&mut self.engine, *file, context) {
                Ok(errors) | Err(QueryError::Diagnostics(errors)) => {
                    diagnostics.extend(errors);
                }
                Err(error) => return Err(error),
            }
        }
        if crate::queries::has_errors(&diagnostics) {
            return Ok(CheckOutput {
                diagnostics: select_diagnostics(diagnostics, Some(&self.engine.sources)),
                completion,
                sources: self.engine.sources.clone(),
                unsafe_ffi: self.unsafe_ffi.clone(),
            });
        }
        let root = self.entry.0.package.clone();
        let policy = |package: &PackageId| {
            if *package == root {
                lowering
            } else {
                LoweringPolicy::Comptime
            }
        };
        let files: Vec<_> = self
            .files
            .iter()
            .map(|(file, context)| (*file, context, policy(&context.package)))
            .collect();
        for (file, output) in crate::tools::check_files(&mut self.engine, &files)? {
            if output.completion != CheckCompletion::Complete {
                completion = output.completion;
            } else if output.diagnostics.is_empty() {
                checked(&mut self.engine, file)?;
            }
            diagnostics.extend(output.diagnostics);
        }
        // The root package's dead error arms (LANG-8) are warnings of the
        // files declaring them.
        if !crate::queries::has_errors(&diagnostics) && completion == CheckCompletion::Complete {
            let files: Vec<_> = self
                .files
                .iter()
                .filter(|(_, c)| c.package == root)
                .map(|(file, _)| *file)
                .collect();
            diagnostics.extend(self.engine.dead_arm_warnings(&files));
        }
        // Dependencies lower only what the program's own package reaches.
        if lowering == LoweringPolicy::Reachable
            && !crate::queries::has_errors(&diagnostics)
            && completion == CheckCompletion::Complete
        {
            let roots = self
                .files
                .iter()
                .filter(|(_, c)| c.package == root)
                .flat_map(|(file, _)| self.engine.file_definitions(*file))
                .collect::<Vec<_>>();
            match self.engine.lower_reachable(roots) {
                Ok(()) => {}
                Err(QueryError::Diagnostics(errors)) => diagnostics.extend(errors),
                Err(QueryError::NotImplemented) => {
                    let phase = self
                        .engine
                        .unavailable
                        .or(self.engine.last_phase)
                        .unwrap_or(QueryKind::Lower);
                    self.engine.unavailable = Some(phase);
                    completion = CheckCompletion::Incomplete { phase };
                }
                Err(error) => return Err(error),
            }
        }
        let _merge = hirsel_intern::timings::interval(hirsel_intern::timings::Interval::Merge);
        let mut diagnostics = select_diagnostics(diagnostics, Some(&self.engine.sources));
        let files: Vec<_> = self.files.iter().map(|(file, cx)| (*file, cx)).collect();
        diagnostic_guidance::append(
            &files,
            &self.engine.sources,
            &self.engine.declarations.frozen(),
            &mut diagnostics,
        );
        Ok(CheckOutput {
            diagnostics,
            completion,
            sources: self.engine.sources.clone(),
            unsafe_ffi: self.unsafe_ffi.clone(),
        })
    }
    /// The manifest-selected entry, if this package can be launched.
    pub fn launch_entry(&self) -> Option<&DefPath> {
        self.has_launch_entry.then_some(&self.entry)
    }
    pub fn check_for_execution(&mut self) -> Result<(), ToolError> {
        let _reads = self
            .engine
            .result_store
            .as_ref()
            .and_then(|store| store.read_scope());
        if !self.has_launch_entry {
            return Err(invalid("launch manifest requires entry"));
        }
        let result = self.check(LoweringPolicy::Reachable).and_then(|output| {
            if crate::queries::has_errors(&output.diagnostics) {
                Err(QueryError::Diagnostics(output.diagnostics))
            } else if output.completion != CheckCompletion::Complete {
                Err(QueryError::NotImplemented)
            } else {
                Ok(())
            }
        });
        result.map_err(|error| self.error(error))?;
        if !self.unsafe_ffi.is_empty() {
            let entry = self.entry.clone();
            let foreign = self.engine.foreign_use(&entry).map_err(|e| self.error(e))?;
            self.engine.foreign_unreplayable = !foreign.unmodeled.is_empty();
        }
        Ok(())
    }
    /// The escape from capability checking the entry reaches, if any (CAP-6).
    pub fn unsafe_reach(&mut self) -> Result<Option<crate::UnsafeReach>, ToolError> {
        let entry = self.entry.clone();
        self.engine.unsafe_reach(&entry).map_err(|e| self.error(e))
    }
    /// The reference interpreter calls no C: a program whose entry reaches an
    /// extern refuses before entry (PROG-4).
    pub fn refuse_foreign_interpretation(&mut self) -> Result<(), ToolError> {
        // Externs check only inside `unsafe ffi` packages.
        if self.unsafe_ffi.is_empty() {
            return Ok(());
        }
        let entry = self.entry.clone();
        let foreign = self.engine.foreign_use(&entry).map_err(|e| self.error(e))?;
        match foreign.externs.first() {
            Some(extern_) => Err(ToolError::Runtime(format!(
                "launch: the reference interpreter does not call C, and {extern_} is an \
                 extern \"c\" function; build or run the program natively (PROG-4)"
            ))),
            None => Ok(()),
        }
    }
    /// Recording or replaying a run whose entry reaches an extern through anything but
    /// a deterministic wrapper is a launch refusal naming that use (PROG-4).
    pub fn refuse_unreplayable(&mut self) -> Result<(), ToolError> {
        if self.unsafe_ffi.is_empty() {
            return Ok(());
        }
        let entry = self.entry.clone();
        let foreign = self.engine.foreign_use(&entry).map_err(|e| self.error(e))?;
        match foreign.unmodeled.first_key_value() {
            Some((user, extern_)) => Err(ToolError::Runtime(format!(
                "launch: {user} calls the extern {extern_} and is not a wrapper with a \
                 deterministic model, so this run cannot be recorded or replayed (PROG-4)"
            ))),
            None => Ok(()),
        }
    }
    /// The path of every source the program checks, in check order.
    pub fn source_paths(&self) -> Vec<PathBuf> {
        self.files
            .iter()
            .map(|(file, _)| self.engine.sources.file(*file).unwrap().path.clone())
            .collect()
    }
    /// Replace the text of the source at `path` with a new snapshot under the
    /// same logical name, as a resident session does after an edit: the next
    /// check indexes it in place of the old one, and only the memos whose reads
    /// it changes are recomputed. False, changing nothing, when `path` is not a
    /// source of a package checked from source (a prechecked package's sources
    /// are part of its precheck's key): the caller loads again.
    pub fn replace_source(&mut self, path: &Path, text: String) -> Result<bool, ToolError> {
        let Some(index) = self
            .files
            .iter()
            .position(|(file, _)| self.engine.sources.file(*file).unwrap().path == path)
        else {
            return Ok(false);
        };
        let (old, _) = self.files[index].clone();
        let file = self
            .engine
            .sources
            .add_snapshot(old, text)
            .ok_or(ToolError::SourceTooLarge)?;
        // Source replacement precedes indexing and its memo revision. Until
        // the next complete check, no prior receipt describes these inputs.
        self.engine.checked_scope = Default::default();
        self.files[index].0 = file;
        if self.primary == old {
            self.primary = file;
        }
        Ok(true)
    }
    /// Observe every source in check order, as loading does. Ordinary heads
    /// avoid parsing unchanged bytes; changed invalid bytes fail as on a fresh load.
    pub fn parse_sources(&mut self) -> Result<(), ToolError> {
        for (file, context) in self.files.clone() {
            self.engine
                .index_source(file, &context)
                .map_err(|e| self.error(e))?;
        }
        Ok(())
    }
    pub fn error(&self, error: QueryError) -> ToolError {
        ToolError::Query {
            error,
            phase: self.engine.unavailable,
            sources: self.engine.sources.clone(),
        }
    }
}

/// Preserve a package corpus input for replay after moving its artifact directory.
/// Returns false for a standalone input, whose source alone is its complete fixture.
pub fn bundle_program(path: &Path, source: &str, dir: &Path) -> Result<bool, ToolError> {
    let sidecar = path.with_extension("hir.json");
    if !sidecar.try_exists()? {
        return Ok(false);
    }
    let primary = std::fs::canonicalize(path)?;
    let mut graph = Graph::default();
    let root = graph.visit(&sidecar, None)?;
    graph.expand_slots()?;
    std::fs::create_dir_all(dir)?;
    let manifests: BTreeMap<_, _> = graph
        .packages
        .iter()
        .map(|(id, p)| {
            let name = if id == &root {
                "source.hir.json".into()
            } else {
                format!("package-{}-{}.json", id.name, id.version)
            };
            (p.path.clone(), name)
        })
        .collect();
    for (id, p) in &graph.packages {
        let mut value: Value = serde_json::from_slice(&p.manifest).unwrap();
        for (index, (file, _)) in p.sources.iter().enumerate() {
            let name = if file == &primary {
                "source.hir".into()
            } else {
                format!("source-{}-{}-{index}.hir", id.name, id.version)
            };
            let text = if file == &primary {
                source.into()
            } else {
                crate::tools::read_source(file)?
            };
            std::fs::write(dir.join(&name), text)?;
            value["sources"][index]["path"] = name.into();
        }
        let base = p.path.parent().unwrap();
        if let Some(catalog) = value
            .get_mut("dependency_catalog")
            .and_then(Value::as_object_mut)
        {
            // An unselected descriptor need not exist; it keeps its spelling.
            for descriptor in catalog.values_mut() {
                let declared = base.join(descriptor["manifest"].as_str().unwrap());
                if let Some(name) = std::fs::canonicalize(declared)
                    .ok()
                    .and_then(|target| manifests.get(&target))
                {
                    descriptor["manifest"] = name.clone().into();
                }
            }
        } else {
            for dependency in value["dependencies"].as_array_mut().unwrap() {
                let target =
                    std::fs::canonicalize(base.join(dependency["manifest"].as_str().unwrap()))?;
                dependency["manifest"] = manifests[&target].clone().into();
            }
        }
        std::fs::write(
            dir.join(&manifests[&p.path]),
            serde_json::to_vec_pretty(&value).unwrap(),
        )?;
    }
    Ok(true)
}
/// A source's exact sidecar path: `<source>.json`.
fn sidecar_path(path: &Path) -> PathBuf {
    let mut sidecar = path.as_os_str().to_os_string();
    sidecar.push(".json");
    PathBuf::from(sidecar)
}
/// Load sources using the configured ordinary product provider.
pub(crate) fn load_program_source(
    compiler: &Compiler,
    path: &Path,
    replacement: Option<&str>,
    entry_selection: EntrySelection,
) -> Result<LoadedProgram, ToolError> {
    load_program_inputs(
        compiler,
        path,
        &crate::loader_inputs::DiskInputs,
        Loading {
            replacement,
            entry_selection,
            parse_sources: true,
        },
    )
}

#[derive(Clone, Copy)]
pub(crate) enum EntrySelection {
    Launch,
    /// Validated declaration export, before foreign link artifacts exist.
    Export,
    Check,
    Test,
}
pub(crate) struct Loading<'a> {
    pub replacement: Option<&'a str>,
    pub entry_selection: EntrySelection,
    pub parse_sources: bool,
}
pub(crate) fn load_program_inputs(
    compiler: &Compiler,
    path: &Path,
    inputs: &dyn crate::loader_inputs::LoaderInputs,
    loading: Loading<'_>,
) -> Result<LoadedProgram, ToolError> {
    load_program_inputs_with_source_index(compiler, path, inputs, loading, None)
}

pub(crate) fn load_program_inputs_with_source_index(
    compiler: &Compiler,
    path: &Path,
    inputs: &dyn crate::loader_inputs::LoaderInputs,
    loading: Loading<'_>,
    source_index: Option<crate::SourceIndexState>,
) -> Result<LoadedProgram, ToolError> {
    let Loading {
        replacement,
        entry_selection,
        parse_sources,
    } = loading;
    let _timing = hirsel_intern::timings::interval(hirsel_intern::timings::Interval::S0);
    let (graph, package) = Graph::program_inputs_with_links(
        path,
        inputs,
        !matches!(entry_selection, EntrySelection::Export),
    )?;
    let root = graph.packages.get(&package).unwrap();
    let entry = if matches!(entry_selection, EntrySelection::Test)
        || (matches!(entry_selection, EntrySelection::Check) && root.entry.is_none())
    {
        entry_path(
            &root
                .sources
                .first()
                .ok_or_else(|| invalid("package has no sources"))?
                .1,
        )
    } else {
        root.entry
            .clone()
            .ok_or_else(|| invalid("launch manifest requires entry"))?
    };
    let primary_path = if path.extension().is_some_and(|e| e == "json") {
        root.sources
            .iter()
            .find(|(_, c)| c.modules == entry.0.modules)
            .map(|(p, _)| p.clone())
            .ok_or_else(|| invalid("entry module has no source"))?
    } else {
        inputs.canonicalize(path)?
    };
    if !root.sources.iter().any(|(p, _)| p == &primary_path) {
        return Err(invalid(
            "sidecar must assign its source to the root package",
        ));
    }
    // A package's effective slot map is chosen by its selecting frames, so it
    // keys the package's own products; a package without slots keys as before.
    let selection = |id: &PackageId| {
        let slots = graph
            .slots
            .get(id)
            .map(slots::canonical)
            .unwrap_or_default();
        let mut parts = vec![
            id.name.as_bytes(),
            id.version.as_bytes(),
            graph.packages[id].manifest.as_slice(),
        ];
        if !slots.is_empty() {
            parts.push(&slots);
        }
        ContentHash::of_parts("hirsel.package.selection.v1", parts)
    };
    let manifest_hash = |packages: &mut dyn Iterator<Item = &PackageId>| {
        let parts: Vec<_> = packages.map(selection).collect();
        ContentHash::of_parts(
            "hirsel.package.graph.v1",
            parts.iter().map(|h| h.0.as_slice()),
        )
    };
    let mut environment = default_environment();
    environment.package_manifest_hash = manifest_hash(&mut graph.packages.keys());
    let mut files = Vec::new();
    let mut texts = Vec::new();
    let mut assigned = BTreeMap::new();
    for p in graph.packages.values() {
        for (source, context) in &p.sources {
            if assigned.insert(source.clone(), context.clone()).is_some() {
                return Err(invalid("one source path has more than one FileContext"));
            }
            let text = if source == &primary_path && replacement.is_some() {
                replacement.unwrap().to_string()
            } else {
                // Preserve the standalone UTF-8 diagnostic policy and original bytes.
                crate::tools::decode_source(source, inputs.read(source)?.to_vec())?
            };
            files.push((source.clone(), context.clone()));
            texts.push(text);
        }
    }
    // Dependencies key on their own closure; the program package on the graph.
    let dependencies: Vec<_> = graph.packages.keys().filter(|id| **id != package).collect();
    let manifests: BTreeMap<_, _> = dependencies
        .iter()
        .map(|id| {
            let closure = graph.closure(id);
            ((*id).clone(), manifest_hash(&mut closure.iter()))
        })
        .collect();
    let limits = compiler.comptime_limits();
    let unsafe_ffi: Vec<UnsafeFfiPackage> = graph
        .packages
        .iter()
        .filter_map(|(id, p)| {
            let (link, headers_checked, wrappers, threads) = p.unsafe_ffi.clone()?;
            Some(UnsafeFfiPackage {
                package: id.clone(),
                link,
                headers_checked,
                wrappers,
                threads,
            })
        })
        .collect();
    let unsafe_ffi_metadata: BTreeMap<_, _> = unsafe_ffi
        .iter()
        .map(|p| {
            (
                p.package.clone(),
                hirsel_types::UnsafeFfi {
                    wrappers: p.wrappers.clone(),
                    threads: p.threads,
                },
            )
        })
        .collect();
    let mut engine = compiler.engine(environment);
    let _reads = engine
        .result_store
        .as_ref()
        .and_then(|store| store.read_scope());
    let publication = engine
        .result_store
        .as_ref()
        .and_then(|store| store.local_product_store())
        .map(|store| store.request());
    if let Some(source_index) = source_index {
        source_index
            .restore(&mut engine)
            .map_err(|_| invalid("source index cannot restore into this compiler"))?;
    }
    engine.begin_memo_touch_tracking();
    engine.comptime_limits = limits;
    engine.package_manifests = manifests;
    let package_exposures: BTreeMap<_, _> = graph
        .packages
        .iter()
        .map(|(id, p)| (id.clone(), p.exposures.clone()))
        .collect();
    let package_dependencies: BTreeMap<_, _> = graph
        .packages
        .iter()
        .map(|(id, p)| (id.clone(), p.dependencies.clone()))
        .collect();
    let previous = engine.declarations.inputs();
    if previous.package_exposures != package_exposures
        || previous.package_dependencies != package_dependencies
        || previous.signature_metadata.unsafe_ffi != unsafe_ffi_metadata
    {
        let inputs = engine.declarations.inputs_mut();
        inputs.package_exposures = package_exposures;
        inputs.package_dependencies = package_dependencies;
        inputs.signature_metadata.unsafe_ffi = unsafe_ffi_metadata;
    }
    // Each path's latest immutable snapshot is the only candidate for reuse.
    // ParsedSource values are absent after carry; their indexing contexts are
    // retained independently instead of treating archived Catalog.files as all sources.
    let mut previous: BTreeMap<PathBuf, (FileId, FileContext)> = engine
        .indexed_sources
        .iter()
        .filter_map(|(file, (context, _))| {
            Some((
                engine.sources.file(*file)?.path.clone(),
                (*file, context.clone()),
            ))
        })
        .collect();
    let mut primary = None;
    let mut ids = Vec::new();
    for ((source, context), text) in files.iter().zip(texts) {
        let logical = &graph.packages[&context.package].logical_sources[source];
        let file = match previous.remove(source) {
            Some((file, old_context))
                if old_context == *context
                    && engine
                        .sources
                        .file(file)
                        .is_some_and(|old| old.logical_path() == logical) =>
            {
                if engine.sources.file(file).unwrap().text() == text {
                    file
                } else {
                    engine.indexed_sources.remove(&file);
                    engine
                        .sources
                        .add_snapshot(file, text)
                        .ok_or(ToolError::SourceTooLarge)?
                }
            }
            Some((file, _)) => {
                engine.indexed_sources.remove(&file);
                engine
                    .sources
                    .add_logical(source, logical, text)
                    .ok_or(ToolError::SourceTooLarge)?
            }
            None => engine
                .sources
                .add_logical(source, logical, text)
                .ok_or(ToolError::SourceTooLarge)?,
        };
        if source == &primary_path {
            primary = Some(file);
        }
        ids.push((file, context.clone()));
    }
    engine.foreign_links = unsafe_ffi
        .iter()
        .flat_map(|p| p.link.iter().cloned())
        .collect();
    engine.foreign_threads = unsafe_ffi
        .iter()
        .map(|p| (p.package.clone(), p.threads))
        .collect();
    let mut program = LoadedProgram {
        engine,
        files: ids,
        entry,
        has_launch_entry: root.entry.is_some(),
        primary: primary.ok_or_else(|| invalid("missing primary source"))?,
        unsafe_ffi,
        slots: graph.slots.clone(),
        inputs: path
            .extension()
            .is_none_or(|e| e != "json")
            .then(|| sidecar_path(path))
            .into_iter()
            .chain(graph.packages.values().map(|p| p.path.clone()))
            .collect(),
    };
    if parse_sources {
        // Removed sources pass through the ordinary empty-source index diff:
        // declaration heads and all typed subscribers retire before checking.
        // Keep their old anchors alive only for detached outputs that own them.
        for (_, (file, context)) in previous {
            program.engine.indexed_sources.remove(&file);
            let retired = program
                .engine
                .sources
                .add_snapshot(file, String::new())
                .ok_or(ToolError::SourceTooLarge)?;
            program
                .engine
                .parse_file(retired, &context)
                .map_err(|e| program.error(e))?;
            program.engine.indexed_sources.remove(&retired);
        }
        for (file, context) in &program.files {
            program
                .engine
                .index_source(*file, context)
                .map_err(|e| program.error(e))?;
        }
        slots::check_targets(&program.engine, &program.slots)?;
        slots::check_conformance(&mut program.engine, &program.slots)?;
        // A declared wrapper names an ordinary function of its own package.
        for package in &program.unsafe_ffi {
            // Wrapper classification consumes syntax only for this package.
            for (file, context) in &program.files {
                if context.package == package.package {
                    program
                        .engine
                        .parse_file(*file, context)
                        .map_err(|e| program.error(e))?;
                }
            }
            let functions: BTreeMap<_, _> = program
                .files
                .iter()
                .filter(|(_, c)| c.package == package.package)
                .flat_map(|(file, _)| program.engine.file_functions(*file))
                .collect();
            for wrapper in package.wrappers.keys() {
                if functions.get(wrapper) != Some(&false) {
                    return Err(invalid(&format!(
                        "unsafe_ffi wrapper {wrapper} is not a non-extern function of {}@{}",
                        package.package.name, package.package.version
                    )));
                }
            }
        }
    }
    if let Some(publication) = publication {
        let _ = publication.finish();
    }
    Ok(program)
}

/// The manifest is canonical; the source retains its declared spelling (so a
/// symlink does not leak its physical target into the artifact). Their relationship
/// enters artifacts; no host root becomes part of a source name. This also
/// supports sources outside the manifest directory and absolute manifest entries.
fn logical_source_path(source: &Path, manifest: &Path, package: &PackageId) -> PathBuf {
    let base = manifest.parent().unwrap();
    let common = base
        .ancestors()
        .find(|ancestor| source.starts_with(ancestor))
        .unwrap();
    let mut relative = PathBuf::new();
    for _ in base.strip_prefix(common).unwrap().components() {
        relative.push("..");
    }
    relative.push(source.strip_prefix(common).unwrap());
    PathBuf::from(format!("{}@{}", package.name, package.version)).join(relative)
}

struct SelectedPackage {
    path: PathBuf,
    manifest: Vec<u8>,
    sources: Vec<(PathBuf, FileContext)>,
    logical_sources: BTreeMap<PathBuf, PathBuf>,
    exposures: Vec<DependencyExposure>,
    entry: Option<DefPath>,
    dependencies: Vec<PackageId>,
    unsafe_ffi: Option<ForeignManifest>,
    /// Schema 2: declared implementation slots, the dependency catalog
    /// (descriptor manifest and package by edge ID) and the selected edges.
    parameters: BTreeMap<String, slots::Slot>,
    catalog: BTreeMap<String, (PathBuf, PackageId)>,
    uses: BTreeMap<String, PackageId>,
}
/// `unsafe_ffi`: link inputs, headers_checked, wrappers (PROG-4) and threads (S-13).
type ForeignManifest = (
    Vec<LinkInput>,
    bool,
    BTreeMap<DefPath, hirsel_types::ForeignWrapper>,
    hirsel_types::ForeignThreads,
);
struct Graph<'a> {
    inputs: &'a dyn crate::loader_inputs::LoaderInputs,
    require_link_files: bool,
    packages: BTreeMap<PackageId, SelectedPackage>,
    active: Vec<PathBuf>,
    /// Every edge selecting each package, in discovery order.
    selections: BTreeMap<PackageId, Vec<slots::Selection>>,
    /// The effective slot maps, once expanded.
    slots: PackageSlots,
}
impl Default for Graph<'static> {
    fn default() -> Self {
        Self::with_inputs(&crate::loader_inputs::DiskInputs)
    }
}
impl Graph<'static> {
    fn program(path: &Path) -> Result<(Self, PackageId), ToolError> {
        Self::program_inputs(path, &crate::loader_inputs::DiskInputs)
    }
}
impl<'a> Graph<'a> {
    /// Use the same graph for declarations, identities and executable programs.
    fn program_inputs(
        path: &Path,
        inputs: &'a dyn crate::loader_inputs::LoaderInputs,
    ) -> Result<(Self, PackageId), ToolError> {
        Self::program_inputs_with_links(path, inputs, true)
    }
    fn program_inputs_with_links(
        path: &Path,
        inputs: &'a dyn crate::loader_inputs::LoaderInputs,
        require_link_files: bool,
    ) -> Result<(Self, PackageId), ToolError> {
        let mut graph = Self::with_inputs(inputs);
        graph.require_link_files = require_link_files;
        let package = if path.extension().is_some_and(|e| e == "json") {
            graph.visit(path, None)?
        } else {
            let mut sidecar = path.as_os_str().to_os_string();
            sidecar.push(".json");
            let sidecar = PathBuf::from(sidecar);
            match inputs.metadata(&sidecar) {
                Ok(_) => graph.visit(&sidecar, None)?,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    let source = inputs.canonicalize(path)?;
                    let mut manifest = source.as_os_str().to_os_string();
                    manifest.push(".json");
                    let value = serde_json::json!({
                        "schema": 1,
                        "package": {"name": "standalone", "version": "0"},
                        "sources": [{"path": source.file_name().unwrap().to_string_lossy(), "module": []}],
                        "entry": {"module": [], "name": "main"},
                        "dependencies": [{
                            "manifest": crate::tools::toolchain_std_manifest_inputs(inputs)?,
                            "package": {"name": "std", "version": "1"},
                            "expose": [],
                        }],
                    });
                    let context = default_context();
                    let package = context.package.clone();
                    graph
                        .selections
                        .entry(package.clone())
                        .or_default()
                        .push(slots::Selection {
                            parent: None,
                            edge: None,
                            bindings: BTreeMap::new(),
                        });
                    let std = graph.visit_from(
                        &crate::tools::toolchain_std_manifest_inputs(inputs)?,
                        Some(&PackageId {
                            name: "std".into(),
                            version: "1".into(),
                        }),
                        slots::Selection {
                            parent: Some(package.clone()),
                            edge: None,
                            bindings: BTreeMap::new(),
                        },
                    )?;
                    let exposures = graph.packages[&std]
                        .sources
                        .iter()
                        .filter_map(|(_, context)| {
                            let head = context.modules.first()?.clone();
                            Some(DependencyExposure {
                                head: head.clone(),
                                package: std.clone(),
                                modules: vec![head],
                            })
                        })
                        .collect();
                    let logical = PathBuf::from("standalone@0").join(source.file_name().unwrap());
                    graph.packages.insert(
                        package.clone(),
                        SelectedPackage {
                            path: PathBuf::from(manifest),
                            manifest: serde_json::to_vec(&value).unwrap(),
                            sources: vec![(source.clone(), context.clone())],
                            logical_sources: BTreeMap::from([(source, logical)]),
                            exposures,
                            entry: Some(entry_path(&context)),
                            dependencies: vec![std],
                            unsafe_ffi: None,
                            parameters: BTreeMap::new(),
                            catalog: BTreeMap::new(),
                            uses: BTreeMap::new(),
                        },
                    );
                    package
                }
                Err(e) => return Err(e.into()),
            }
        };
        graph.expand_slots()?;
        Ok((graph, package))
    }
    fn with_inputs(inputs: &'a dyn crate::loader_inputs::LoaderInputs) -> Self {
        Self {
            inputs,
            require_link_files: true,
            packages: BTreeMap::new(),
            active: Vec::new(),
            selections: BTreeMap::new(),
            slots: PackageSlots::new(),
        }
    }
    fn closure(&self, id: &PackageId) -> BTreeSet<PackageId> {
        let mut result = BTreeSet::new();
        let mut todo = vec![id.clone()];
        while let Some(id) = todo.pop() {
            if result.insert(id.clone()) {
                todo.extend(self.packages[&id].dependencies.clone());
            }
        }
        result
    }
    fn revision(
        &self,
        id: &PackageId,
        environment: &QueryEnvironment,
        files: &[(PathBuf, FileContext)],
        texts: &[String],
    ) -> ContentHash {
        let mut parts = vec![
            environment.interface_version.to_le_bytes().to_vec(),
            environment.compiler_version.as_bytes().to_vec(),
            environment.spec_hash.0.to_vec(),
            environment.builtin_signatures_hash.0.to_vec(),
        ];
        for id in self.closure(id) {
            let package = &self.packages[&id];
            parts.extend([
                id.name.as_bytes().to_vec(),
                id.version.as_bytes().to_vec(),
                ContentHash::of_bytes(&package.manifest).0.to_vec(),
            ]);
            for (source, context) in &package.sources {
                let index = files
                    .iter()
                    .position(|(path, c)| c == context && path == source)
                    .unwrap();
                parts.push(ContentHash::of_bytes(texts[index].as_bytes()).0.to_vec());
            }
        }
        ContentHash::of_parts("hirsel.checked-package.v1", parts.iter().map(Vec::as_slice))
    }
    /// Select a root manifest and its dependencies.
    fn visit(&mut self, path: &Path, expected: Option<&PackageId>) -> Result<PackageId, ToolError> {
        let root = slots::Selection {
            parent: None,
            edge: None,
            bindings: BTreeMap::new(),
        };
        self.select(path, expected, Some(root))
    }
    fn visit_from(
        &mut self,
        path: &Path,
        expected: Option<&PackageId>,
        via: slots::Selection,
    ) -> Result<PackageId, ToolError> {
        self.select(path, expected, Some(via))
    }
    /// Select one manifest through `via`; without it, only that manifest is
    /// validated and no dependency is opened.
    fn select(
        &mut self,
        path: &Path,
        expected: Option<&PackageId>,
        via: Option<slots::Selection>,
    ) -> Result<PackageId, ToolError> {
        let follow_dependencies = via.is_some();
        let path = self.inputs.canonicalize(path)?;
        if self.active.contains(&path) {
            return Err(invalid("cyclic package dependency"));
        }
        let bytes = self.inputs.read(&path)?.to_vec();
        let value: UniqueJson = serde_json::from_slice(&bytes)
            .map_err(|e| invalid(&format!("{}: {e}", path.display())))?;
        let schema = value.0.get("schema").and_then(Value::as_u64);
        let fields: &[&str] = match schema {
            Some(1) => &[
                "schema",
                "package",
                "sources",
                "dependencies",
                "entry",
                "unsafe_ffi",
            ],
            Some(2) => &[
                "schema",
                "package",
                "sources",
                "parameters",
                "dependency_catalog",
                "dependencies",
                "entry",
                "unsafe_ffi",
            ],
            _ => return Err(invalid("package schema must be 1 or 2")),
        };
        let object = object_value(&value.0, fields)?;
        let package = package_value(required(object, "package")?)?;
        if package.package_is("core", "1") {
            return Err(invalid("core@1 is compiler-owned"));
        }
        if expected.is_some_and(|e| e != &package) {
            return Err(invalid(
                "selected package version does not match dependency",
            ));
        }
        if let Some(old) = self.packages.get(&package) {
            if old.path != path {
                return Err(invalid(
                    "two implementation revisions selected for one package",
                ));
            }
            self.selections
                .entry(package.clone())
                .or_default()
                .extend(via);
            return Ok(package);
        }
        self.selections
            .entry(package.clone())
            .or_default()
            .extend(via);
        self.active.push(path.clone());
        let base = path.parent().unwrap();
        let mut logical_sources = BTreeMap::new();
        let sources = array(required(object, "sources")?)?
            .iter()
            .map(|s| {
                let s = object_value(s, &["path", "module"])?;
                let declared = Path::new(string(required(s, "path")?)?);
                let source = self.inputs.canonicalize(&base.join(declared))?;
                // Absolute entries may name the same directory through an alias
                // such as /tmp. Compare canonical paths in that case; relative
                // entries retain their declared names, including symlinks.
                let logical = if declared.is_absolute() {
                    source.clone()
                } else {
                    base.join(declared)
                };
                logical_sources.insert(
                    source.clone(),
                    logical_source_path(&logical, &path, &package),
                );
                let modules = modules(required(s, "module")?)?;
                Ok((
                    source,
                    FileContext {
                        package: package.clone(),
                        modules,
                    },
                ))
            })
            .collect::<Result<Vec<_>, ToolError>>()?;
        if sources.is_empty() {
            return Err(invalid("package requires at least one source"));
        }
        let entry = object
            .get("entry")
            .map(|e| -> Result<DefPath, ToolError> {
                let e = object_value(e, &["module", "name"])?;
                Ok(DefPath(hirsel_syntax::ItemPath {
                    package: package.clone(),
                    modules: modules(required(e, "module")?)?,
                    name: identifier(required(e, "name")?)?,
                }))
            })
            .transpose()?;
        let parameters = object
            .get("parameters")
            .map(slots::parameters)
            .transpose()?
            .unwrap_or_default();
        let mut catalog = BTreeMap::new();
        if let Some(entries) = object.get("dependency_catalog") {
            let entries = entries
                .as_object()
                .ok_or_else(|| invalid("dependency_catalog must be an object"))?;
            for (id, descriptor) in entries {
                let id = identifier(&Value::String(id.clone()))?;
                if id == "self" {
                    return Err(invalid("self is reserved and cannot be a catalog ID"));
                }
                let d = object_value(descriptor, &["manifest", "package"])?;
                let target = package_value(required(d, "package")?)?;
                catalog.insert(id, (base.join(string(required(d, "manifest")?)?), target));
            }
        }
        // Each edge: its manifest, package, written bindings and catalog ID.
        let mut edges = Vec::new();
        let mut uses = BTreeMap::new();
        for d in array(required(object, "dependencies")?)? {
            if schema == Some(1) {
                let o = object_value(d, &["manifest", "package", "expose"])?;
                let expected = package_value(required(o, "package")?)?;
                let manifest = base.join(string(required(o, "manifest")?)?);
                edges.push((manifest, expected, BTreeMap::new(), None, o));
                continue;
            }
            let o = object_value(d, &["use", "bindings", "expose"])?;
            let id = identifier(required(o, "use")?)?;
            let Some((manifest, expected)) = catalog.get(&id).cloned() else {
                return Err(invalid(&format!(
                    "dependency uses {id:?}, which is not in the dependency_catalog"
                )));
            };
            if uses.insert(id.clone(), expected.clone()).is_some() {
                return Err(invalid(&format!("two dependencies use {id:?}")));
            }
            let bindings = slots::bindings(required(o, "bindings")?)?;
            edges.push((manifest, expected, bindings, Some(id), o));
        }
        slots::check_frame(
            &package,
            &catalog,
            &uses,
            &parameters,
            edges.iter().flat_map(|e| e.2.values().cloned()),
        )?;
        let mut exposures = Vec::new();
        let mut dependencies = Vec::new();
        for (manifest, expected, bindings, edge, d) in edges {
            if !follow_dependencies {
                dependency_exposures(d, &expected)?;
                continue;
            }
            let selected = self.visit_from(
                &manifest,
                Some(&expected),
                slots::Selection {
                    parent: Some(package.clone()),
                    edge,
                    bindings,
                },
            )?;
            // Every foreign library of a program is chosen by its root (PROG-4).
            if self.active.len() > 1 && self.packages[&selected].unsafe_ffi.is_some() {
                return Err(invalid(&format!(
                    "only the root launch manifest may select the unsafe ffi package {}@{}",
                    selected.name, selected.version
                )));
            }
            dependencies.push(selected.clone());
            // Module heads are source conveniences; package/version identity is
            // still pinned by this dependency's manifest selection.
            for (_, context) in &self.packages[&selected].sources {
                if let Some(head) = context.modules.first() {
                    exposures.push(DependencyExposure {
                        head: head.clone(),
                        package: selected.clone(),
                        modules: vec![head.clone()],
                    });
                }
            }
            exposures.extend(dependency_exposures(d, &selected)?);
        }
        if package.name_is("std") {
            for (_, context) in &sources {
                if let Some(head) = context.modules.first() {
                    exposures.push(DependencyExposure {
                        head: head.clone(),
                        package: package.clone(),
                        modules: vec![head.clone()],
                    });
                }
            }
        }
        let unsafe_ffi = object
            .get("unsafe_ffi")
            .map(|v| foreign_manifest(v, base, &package, self.inputs, self.require_link_files))
            .transpose()?;
        self.active.pop();
        self.packages.insert(
            package.clone(),
            SelectedPackage {
                path,
                manifest: bytes,
                sources,
                logical_sources,
                exposures,
                entry,
                dependencies,
                unsafe_ffi,
                parameters,
                catalog,
                uses,
            },
        );
        Ok(package)
    }
}
/// Parse `{"link": [..], "headers_checked": b, "threads": t, "wrappers": [..]}` (PROG-4,
/// S-13); `threads` defaults to `"home"`.
fn foreign_manifest(
    v: &Value,
    base: &Path,
    package: &PackageId,
    inputs: &dyn crate::loader_inputs::LoaderInputs,
    require_link_files: bool,
) -> Result<ForeignManifest, ToolError> {
    let o = object_value(v, &["link", "headers_checked", "threads", "wrappers"])?;
    let link = array(required(o, "link")?)?
        .iter()
        .map(|l| {
            let l = string(l)?;
            if l.contains('/') || [".a", ".o", ".so"].iter().any(|s| l.ends_with(s)) {
                // Export consumes declarations and checked layouts, not link bytes.
                // `base` is the canonical manifest directory. Ordinary loads still
                // resolve the actual file, and build/link still read its bytes.
                let path = base.join(l);
                Ok(LinkInput::File(if require_link_files {
                    inputs.canonicalize(&path)?
                } else {
                    path
                }))
            } else if l
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"_.+-".contains(&b))
                && !l.starts_with('-')
            {
                Ok(LinkInput::Library(l.into()))
            } else {
                Err(invalid(&format!("invalid link library {l:?}")))
            }
        })
        .collect::<Result<Vec<_>, ToolError>>()?;
    let headers_checked = required(o, "headers_checked")?
        .as_bool()
        .ok_or_else(|| invalid("headers_checked must be a boolean"))?;
    let threads = match o.get("threads").map(string).transpose()? {
        None | Some("home") => hirsel_types::ForeignThreads::Home,
        Some("serial") => hirsel_types::ForeignThreads::Serial,
        Some("any") => hirsel_types::ForeignThreads::Any,
        Some(other) => return Err(invalid(&format!("unknown unsafe_ffi threads {other:?}"))),
    };
    let mut wrappers = BTreeMap::new();
    for w in array(required(o, "wrappers")?)? {
        let w = object_value(w, &["module", "name", "kind", "model"])?;
        let definition = DefPath(hirsel_syntax::ItemPath {
            package: package.clone(),
            modules: modules(required(w, "module")?)?,
            name: identifier(required(w, "name")?)?,
        });
        let kind = match string(required(w, "kind")?)? {
            "pure" => hirsel_types::WrapperKind::Pure,
            "handle" => hirsel_types::WrapperKind::Handle,
            "effect" => hirsel_types::WrapperKind::Effect,
            other => return Err(invalid(&format!("unknown wrapper kind {other:?}"))),
        };
        let deterministic = match string(required(w, "model")?)? {
            "deterministic" => true,
            "none" => false,
            other => return Err(invalid(&format!("unknown wrapper model {other:?}"))),
        };
        let wrapper = hirsel_types::ForeignWrapper {
            kind,
            deterministic,
        };
        if wrappers.insert(definition.clone(), wrapper).is_some() {
            return Err(invalid(&format!("duplicate wrapper {definition}")));
        }
    }
    Ok((link, headers_checked, wrappers, threads))
}
fn dependency_exposures(
    dependency: &Map<String, Value>,
    package: &PackageId,
) -> Result<Vec<DependencyExposure>, ToolError> {
    array(required(dependency, "expose")?)?
        .iter()
        .map(|exposure| {
            let e = object_value(exposure, &["head", "module"])?;
            Ok(DependencyExposure {
                head: identifier(required(e, "head")?)?,
                package: package.clone(),
                modules: modules(required(e, "module")?)?,
            })
        })
        .collect()
}
fn invalid(message: &str) -> ToolError {
    ToolError::Runtime(format!("package manifest: {message}"))
}
fn object_value<'a>(v: &'a Value, keys: &[&str]) -> Result<&'a Map<String, Value>, ToolError> {
    let o = v.as_object().ok_or_else(|| invalid("expected object"))?;
    if let Some(k) = o.keys().find(|k| !keys.contains(&k.as_str())) {
        return Err(invalid(&format!("unknown field {k}")));
    }
    Ok(o)
}
fn required<'a>(o: &'a Map<String, Value>, k: &str) -> Result<&'a Value, ToolError> {
    o.get(k)
        .ok_or_else(|| invalid(&format!("missing field {k}")))
}
fn array(v: &Value) -> Result<&Vec<Value>, ToolError> {
    v.as_array().ok_or_else(|| invalid("expected array"))
}
fn string(v: &Value) -> Result<&str, ToolError> {
    v.as_str()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| invalid("expected nonempty string"))
}
/// Validate a package name with the manifest's ordinary identifier rules.
pub fn validate_package_name(name: &str) -> Result<(), ToolError> {
    identifier(&Value::String(name.into())).map(|_| ())
}
fn identifier(v: &Value) -> Result<String, ToolError> {
    let s = string(v)?;
    if !s
        .bytes()
        .enumerate()
        .all(|(i, b)| b == b'_' || b.is_ascii_alphabetic() || i > 0 && b.is_ascii_digit())
    {
        return Err(invalid("noncanonical identifier"));
    }
    let mut sources = SourceMap::default();
    let file = sources.add("manifest identifier", s.into()).unwrap();
    let tokens = hirsel_lexer::lex(file, &sources);
    if !tokens.diagnostics.is_empty()
        || !matches!(
            tokens.tokens.first().map(|t| &t.kind),
            Some(hirsel_syntax::TokenKind::Ident(_))
        )
    {
        return Err(invalid("reserved keyword cannot be a manifest name"));
    }
    Ok(s.into())
}
fn modules(v: &Value) -> Result<Vec<String>, ToolError> {
    array(v)?.iter().map(identifier).collect()
}
fn package_value(v: &Value) -> Result<PackageId, ToolError> {
    let o = object_value(v, &["name", "version"])?;
    let version = string(required(o, "version")?)?;
    if version.parse::<u64>().is_err() || version != version.parse::<u64>().unwrap().to_string() {
        return Err(invalid(
            "package version must be a canonical decimal u64 major",
        ));
    }
    Ok(PackageId {
        name: identifier(required(o, "name")?)?,
        version: version.into(),
    })
}

/// Strict JSON for negotiated input protocols and package selection. Unlike
/// Value's decoder, rejects duplicate keys at every nesting depth.
#[doc(hidden)]
pub fn decode_unique_json(bytes: &[u8]) -> Result<Value, serde_json::Error> {
    serde_json::from_slice::<UniqueJson>(bytes).map(|value| value.0)
}

// serde_json::Value alone permits duplicate object keys by overwriting them.
// Package selection must reject ambiguous input instead.
struct UniqueJson(Value);
impl<'de> serde::Deserialize<'de> for UniqueJson {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct Visitor;
        impl<'de> serde::de::Visitor<'de> for Visitor {
            type Value = UniqueJson;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("JSON with unique object keys")
            }
            fn visit_map<A: serde::de::MapAccess<'de>>(
                self,
                mut map: A,
            ) -> Result<UniqueJson, A::Error> {
                let mut out = Map::new();
                while let Some(key) = map.next_key::<String>()? {
                    if out.contains_key(&key) {
                        return Err(serde::de::Error::custom(format!("duplicate field {key}")));
                    }
                    out.insert(key, map.next_value::<UniqueJson>()?.0);
                }
                Ok(UniqueJson(Value::Object(out)))
            }
            fn visit_seq<A: serde::de::SeqAccess<'de>>(
                self,
                mut seq: A,
            ) -> Result<UniqueJson, A::Error> {
                let mut out = Vec::new();
                while let Some(v) = seq.next_element::<UniqueJson>()? {
                    out.push(v.0);
                }
                Ok(UniqueJson(Value::Array(out)))
            }
            fn visit_bool<E: serde::de::Error>(self, v: bool) -> Result<UniqueJson, E> {
                Ok(UniqueJson(v.into()))
            }
            fn visit_i64<E: serde::de::Error>(self, v: i64) -> Result<UniqueJson, E> {
                Ok(UniqueJson(v.into()))
            }
            fn visit_u64<E: serde::de::Error>(self, v: u64) -> Result<UniqueJson, E> {
                Ok(UniqueJson(v.into()))
            }
            fn visit_f64<E: serde::de::Error>(self, v: f64) -> Result<UniqueJson, E> {
                serde_json::Number::from_f64(v)
                    .map(|n| UniqueJson(Value::Number(n)))
                    .ok_or_else(|| E::custom("nonfinite number"))
            }
            fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<UniqueJson, E> {
                Ok(UniqueJson(v.into()))
            }
            fn visit_unit<E: serde::de::Error>(self) -> Result<UniqueJson, E> {
                Ok(UniqueJson(Value::Null))
            }
        }
        d.deserialize_any(Visitor)
    }
}
