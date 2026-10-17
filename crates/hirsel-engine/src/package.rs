//! PROG-1: explicit source packages. No directory discovery or std fallback.
use crate::queries::Lowering;
use crate::*;
use hirsel_resolve::DependencyExposure;
use hirsel_syntax::PackageId;
use serde_json::{Map, Value};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

/// A selected program, with every source indexed before any header is queried.
pub struct LoadedProgram {
    pub engine: QueryEngine,
    pub files: Vec<(FileId, FileContext)>,
    pub entry: DefPath,
    pub primary: FileId,
    packages: Vec<crate::package_cache::PackageRevision>,
    cache: PackageCache,
    indexed_revision: u64,
}
impl LoadedProgram {
    pub fn check(&mut self) -> Result<CheckOutput, QueryError> {
        let mut diagnostics = Vec::new();
        let mut completion = CheckCompletion::Complete;
        let unchanged = self.indexed_revision == self.engine.package_cache_revision();
        // Closure identity covers content; ordinary query receipts additionally
        // revalidate selected definitions and negative lookups outside that closure.
        for package in &mut self.packages {
            package.reused = unchanged
                && package.artifact.as_ref().is_some_and(|a| {
                    a.environment.interface_version == self.engine.environment.interface_version
                        && a.environment.compiler_version
                            == self.engine.environment.compiler_version
                        && a.environment.spec_hash == self.engine.environment.spec_hash
                        && a.environment.builtin_signatures_hash
                            == self.engine.environment.builtin_signatures_hash
                        && a.limits == self.engine.comptime_limits
                })
                && if package.proven {
                    self.engine.prove_package(&package.package);
                    true
                } else {
                    self.engine.package_snapshot_valid(&package.package)
                };
        }
        let mut preflight = BTreeMap::new();
        for (file, context) in &self.files {
            if let Some(package) = self
                .packages
                .iter()
                .find(|p| p.reused && p.package == context.package)
            {
                let index = package.files.iter().position(|(f, _)| f == file).unwrap();
                let errors = package.diagnostics.as_ref().unwrap().preflight[index].clone();
                preflight.insert(*file, errors.clone());
                diagnostics.extend(errors);
                continue;
            }
            match crate::tools::loop_declaration_diagnostics(&mut self.engine, *file, context) {
                Ok(errors) | Err(QueryError::Diagnostics(errors)) => {
                    preflight.insert(*file, errors.clone());
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
            });
        }
        let mut outputs = BTreeMap::new();
        let root = self.entry.0.package.clone();
        let policy = |package: &PackageId| {
            if *package == root {
                Lowering::All
            } else {
                Lowering::CompileTime
            }
        };
        for (file, context) in &self.files {
            if let Some(package) = self.packages.iter().find(|p| {
                p.reused
                    && p.package == context.package
                    && p.artifact
                        .as_ref()
                        .unwrap()
                        .check_complete(policy(&p.package))
            }) {
                let index = package.files.iter().position(|(f, _)| f == file).unwrap();
                let artifact = package.artifact.as_ref().unwrap();
                diagnostics.extend(package.diagnostics.as_ref().unwrap().files[index].clone());
                self.engine.last_phase = artifact.last_phase;
                continue;
            }
            let output = crate::tools::check_file_lowering(
                &mut self.engine,
                *file,
                context,
                policy(&context.package),
            )?;
            outputs.insert(*file, output.clone());
            diagnostics.extend(output.diagnostics);
            if output.completion != CheckCompletion::Complete {
                completion = output.completion;
            }
        }
        // GEN-5 crosses package boundaries. Its program verdict is always
        // selected for this program, including when every dependency body is
        // already checked. Unchanged file checks share that program verdict.
        if self.packages.iter().any(|p| p.reused) {
            match crate::generic_graph::check(&mut self.engine) {
                Ok(errors) | Err(QueryError::Diagnostics(errors)) => diagnostics.extend(errors),
                Err(error) => return Err(error),
            }
        }
        // Dependencies lower only what the program's own package reaches.
        if !crate::queries::has_errors(&diagnostics) && completion == CheckCompletion::Complete {
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
        for package in self.packages.iter().filter(|p| {
            unchanged
                && self.cache.enabled()
                && (!p.reused
                    || !p
                        .artifact
                        .as_ref()
                        .unwrap()
                        .check_complete(policy(&p.package)))
        }) {
            let Some(file_outputs) = package
                .files
                .iter()
                .map(|(f, _)| outputs.get(f))
                .collect::<Option<Vec<_>>>()
            else {
                continue;
            };
            if file_outputs
                .iter()
                .any(|o| o.completion != CheckCompletion::Complete)
            {
                continue;
            }
            let local: Vec<Vec<_>> = file_outputs
                .iter()
                .zip(&package.files)
                .map(|(o, (f, _))| {
                    o.diagnostics
                        .iter()
                        .filter(|d| d.primary.file == *f && d.rule.as_str() != "GEN-5")
                        .cloned()
                        .collect()
                })
                .collect();
            let lowered = [Lowering::All, Lowering::CompileTime]
                .into_iter()
                .find(|&p| self.engine.package_lowered(&package.files, p));
            let Some(snapshot) = self
                .engine
                .package_snapshot(&package.package, &package.closure)
            else {
                continue;
            };
            let mut ordinals = BTreeMap::<_, usize>::new();
            let sites = self
                .files
                .iter()
                .filter(|(_, c)| package.closure.contains(&c.package))
                .map(|(file, c)| {
                    let index = ordinals.entry(c.package.clone()).or_default();
                    let site = (
                        *file,
                        c.package.clone(),
                        *index,
                        self.engine.sources.file(*file).unwrap().text().len() as u32,
                    );
                    *index += 1;
                    site
                })
                .collect();
            self.cache.insert(
                package.key,
                crate::package_cache::Artifact {
                    snapshot,
                    sites,
                    preflight: package
                        .files
                        .iter()
                        .map(|(f, _)| preflight.get(f).cloned().unwrap_or_default())
                        .collect(),
                    files: local,
                    lowered,
                    last_phase: self.engine.last_phase,
                    environment: self.engine.environment.clone(),
                    limits: self.engine.comptime_limits,
                },
            );
        }
        Ok(CheckOutput {
            diagnostics: select_diagnostics(diagnostics, Some(&self.engine.sources)),
            completion,
            sources: self.engine.sources.clone(),
        })
    }
    pub fn check_for_execution(&mut self) -> Result<(), ToolError> {
        let result = self.check().and_then(|output| {
            if crate::queries::has_errors(&output.diagnostics) {
                Err(QueryError::Diagnostics(output.diagnostics))
            } else if output.completion != CheckCompletion::Complete {
                Err(QueryError::NotImplemented)
            } else {
                Ok(())
            }
        });
        result.map_err(|error| self.error(error))
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
        for dependency in value["dependencies"].as_array_mut().unwrap() {
            let target = std::fs::canonicalize(
                p.path
                    .parent()
                    .unwrap()
                    .join(dependency["manifest"].as_str().unwrap()),
            )?;
            dependency["manifest"] = manifests[&target].clone().into();
        }
        std::fs::write(
            dir.join(&manifests[&p.path]),
            serde_json::to_vec_pretty(&value).unwrap(),
        )?;
    }
    Ok(true)
}
/// Load with explicit artifact storage, or `PackageCache::disabled()` for a fresh oracle.
pub(crate) fn load_program_source_with_cache(
    compiler: &Compiler,
    path: &Path,
    replacement: Option<&str>,
    cache: &PackageCache,
) -> Result<LoadedProgram, ToolError> {
    let manifest = if path.extension().is_some_and(|e| e == "json") {
        Some(path.to_path_buf())
    } else {
        let mut sidecar = path.as_os_str().to_os_string();
        sidecar.push(".json");
        let sidecar = PathBuf::from(sidecar);
        match std::fs::metadata(&sidecar) {
            Ok(_) => Some(sidecar),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(e.into()),
        }
    };
    let Some(manifest) = manifest else {
        let (mut engine, file) = compiler.load(path)?;
        if let Some(text) = replacement {
            engine.sources = SourceMap::default();
            let file = engine
                .sources
                .add(path, text.into())
                .ok_or(ToolError::SourceTooLarge)?;
            return Ok(LoadedProgram {
                engine,
                files: vec![(file, default_context())],
                entry: entry_path(&default_context()),
                primary: file,
                packages: vec![],
                cache: cache.clone(),
                indexed_revision: 0,
            });
        }
        return Ok(LoadedProgram {
            engine,
            files: vec![(file, default_context())],
            entry: entry_path(&default_context()),
            primary: file,
            packages: vec![],
            cache: cache.clone(),
            indexed_revision: 0,
        });
    };
    let mut graph = Graph::default();
    let package = graph.visit(&manifest, None)?;
    let root = graph.packages.get(&package).unwrap();
    let entry = root
        .entry
        .clone()
        .ok_or_else(|| invalid("launch manifest requires entry"))?;
    let primary_path = if path.extension().is_some_and(|e| e == "json") {
        root.sources
            .iter()
            .find(|(_, c)| c.modules == entry.0.modules)
            .map(|(p, _)| p.clone())
            .ok_or_else(|| invalid("entry module has no source"))?
    } else {
        std::fs::canonicalize(path)?
    };
    if !root.sources.iter().any(|(p, _)| p == &primary_path) {
        return Err(invalid(
            "sidecar must assign its source to the root package",
        ));
    }
    let selection = |id: &PackageId| {
        ContentHash::of_parts(
            "hirsel.package.selection.v1",
            [
                id.name.as_bytes(),
                id.version.as_bytes(),
                graph.packages[id].manifest.as_slice(),
            ],
        )
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
                crate::tools::read_source(source)?
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
    let limits = hirsel_interp::Limits::default();
    let mut revisions = vec![];
    for id in &dependencies {
        let key = graph.revision(id, &environment, &files, &texts);
        let cached = cache.select(key, limits);
        revisions.push(crate::package_cache::PackageRevision {
            package: (*id).clone(),
            key,
            closure: graph.closure(id),
            files: vec![],
            artifact: cached.as_ref().and_then(|p| p.artifact.clone()),
            lowerings: cached.map(|p| p.lowerings),
            reused: false,
            proven: false,
            diagnostics: None,
        });
    }
    let input = crate::queries::ImageInput {
        environment: QueryEnvironment {
            package_manifest_hash: ContentHash::of_bytes(b"hirsel.package-image.v1"),
            ..environment.clone()
        },
        limits,
        manifests: manifests.clone(),
        exposures: dependencies
            .iter()
            .map(|id| ((*id).clone(), graph.packages[*id].exposures.clone()))
            .collect(),
        files: files
            .iter()
            .zip(&texts)
            .map(|((path, context), text)| {
                let dependency = context.package != package;
                (
                    path.clone(),
                    context.clone(),
                    dependency.then(|| text.clone()),
                )
            })
            .collect(),
        packages: revisions
            .iter()
            .map(|r| crate::queries::ImagePackage {
                package: r.package.clone(),
                closure: r.closure.clone(),
                artifact: r.artifact.clone(),
                lowerings: r.lowerings.clone(),
            })
            .collect(),
    };
    let image = crate::queries::PackageImage::obtain(cache, &input, || compiler.adapters());
    let mut engine = QueryEngine::new(environment, compiler.adapters());
    engine.comptime_limits = limits;
    engine.package_manifests = manifests;
    engine
        .adapters
        .compiler_state
        .as_ref()
        .unwrap()
        .borrow_mut()
        .package_exposures = graph
        .packages
        .iter()
        .map(|(id, p)| (id.clone(), p.exposures.clone()))
        .collect();
    let mut primary = None;
    let mut ids = Vec::new();
    for ((source, context), text) in files.iter().zip(texts) {
        let file = engine
            .sources
            .add(source, text)
            .ok_or(ToolError::SourceTooLarge)?;
        if source == &primary_path {
            primary = Some(file);
        }
        ids.push((file, context.clone()));
    }
    engine.install_image(&image);
    let mut program = LoadedProgram {
        engine,
        files: ids,
        entry,
        primary: primary.ok_or_else(|| invalid("missing primary source"))?,
        packages: vec![],
        cache: cache.clone(),
        indexed_revision: 0,
    };
    // Dependency files come parsed from the image. One that failed there is
    // parsed again here, so its failure stops loading in file order with the
    // same engine state as any other parse failure.
    for (file, context) in &program.files {
        if context.package != package && program.engine.parsed_in_image(*file) {
            continue;
        }
        program
            .engine
            .reparse_file(*file, context)
            .map_err(|e| program.error(e))?;
    }
    for mut revision in revisions {
        revision.proven = image.proven(&revision.package);
        revision.files = program
            .files
            .iter()
            .filter(|(_, c)| c.package == revision.package)
            .cloned()
            .collect();
        program.packages.push(revision);
    }
    let sites: BTreeMap<_, _> = graph
        .packages
        .iter()
        .flat_map(|(id, p)| {
            p.sources
                .iter()
                .enumerate()
                .map(|(index, (source, _))| {
                    let file = program
                        .files
                        .iter()
                        .find(|(f, _)| &program.engine.sources.file(*f).unwrap().path == source)
                        .unwrap()
                        .0;
                    ((id.clone(), index), file)
                })
                .collect::<Vec<_>>()
        })
        .collect();
    program.indexed_revision = program.engine.package_cache_revision();
    // Diagnostic display locations are projected into this program's SourceMap;
    // stored artifacts remain immutable, including after callers retain old output.
    for p in &mut program.packages {
        if let Some(artifact) = &p.artifact {
            let moves: Vec<_> = artifact
                .sites
                .iter()
                .map(|(old, id, index, len)| (*old, sites[&(id.clone(), *index)], *len))
                .collect();
            p.diagnostics = Some(crate::package_cache::PackageDiagnostics {
                preflight: artifact
                    .preflight
                    .iter()
                    .map(|ds| crate::queries::rebase_package_diagnostics(ds, &moves))
                    .collect(),
                files: artifact
                    .files
                    .iter()
                    .map(|ds| crate::queries::rebase_package_diagnostics(ds, &moves))
                    .collect(),
            });
        }
        if let Some(lowerings) = &p.lowerings {
            let source_sites = graph
                .packages
                .iter()
                .filter(|(id, _)| p.closure.contains(*id))
                .flat_map(|(id, package)| {
                    package
                        .sources
                        .iter()
                        .enumerate()
                        .map(|(index, _)| {
                            let file = sites[&(id.clone(), index)];
                            (
                                file,
                                id.clone(),
                                index,
                                program.engine.sources.file(file).unwrap().text().len() as u32,
                            )
                        })
                        .collect::<Vec<_>>()
                })
                .collect();
            program.engine.register_package_publication(
                p.package.clone(),
                p.closure.clone(),
                source_sites,
                lowerings.clone(),
            );
        }
    }
    Ok(program)
}

struct SelectedPackage {
    path: PathBuf,
    manifest: Vec<u8>,
    sources: Vec<(PathBuf, FileContext)>,
    exposures: Vec<DependencyExposure>,
    entry: Option<DefPath>,
    dependencies: Vec<PackageId>,
}
#[derive(Default)]
struct Graph {
    packages: BTreeMap<PackageId, SelectedPackage>,
    active: Vec<PathBuf>,
}
impl Graph {
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
    fn visit(&mut self, path: &Path, expected: Option<&PackageId>) -> Result<PackageId, ToolError> {
        let path = std::fs::canonicalize(path)?;
        if self.active.contains(&path) {
            return Err(invalid("cyclic package dependency"));
        }
        let bytes = std::fs::read(&path)?;
        let value: UniqueJson = serde_json::from_slice(&bytes)
            .map_err(|e| invalid(&format!("{}: {e}", path.display())))?;
        let object = object_value(
            &value.0,
            &["schema", "package", "sources", "dependencies", "entry"],
        )?;
        if object.get("schema").and_then(Value::as_u64) != Some(1) {
            return Err(invalid("package schema must be 1"));
        }
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
            return Ok(package);
        }
        self.active.push(path.clone());
        let base = path.parent().unwrap();
        let sources = array(required(object, "sources")?)?
            .iter()
            .map(|s| {
                let s = object_value(s, &["path", "module"])?;
                let source = std::fs::canonicalize(base.join(string(required(s, "path")?)?))?;
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
        let mut exposures = Vec::new();
        let mut dependencies = Vec::new();
        for d in array(required(object, "dependencies")?)? {
            let d = object_value(d, &["manifest", "package", "expose"])?;
            let expected = package_value(required(d, "package")?)?;
            let selected = self.visit(
                &base.join(string(required(d, "manifest")?)?),
                Some(&expected),
            )?;
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
            for exposure in array(required(d, "expose")?)? {
                let e = object_value(exposure, &["head", "module"])?;
                exposures.push(DependencyExposure {
                    head: identifier(required(e, "head")?)?,
                    package: selected.clone(),
                    modules: modules(required(e, "module")?)?,
                });
            }
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
        self.active.pop();
        self.packages.insert(
            package.clone(),
            SelectedPackage {
                path,
                manifest: bytes,
                sources,
                exposures,
                entry,
                dependencies,
            },
        );
        Ok(package)
    }
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
    let mut symbols = Interner::default();
    let tokens = hirsel_lexer::lex(file, &sources, &mut symbols);
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
