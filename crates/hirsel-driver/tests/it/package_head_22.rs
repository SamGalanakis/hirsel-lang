//! Finite PROG-1/LANG-4 oracle. Expected answers use only the graph below,
//! never ManifestIndex, provider lookup, or an engine query.
//!
//! Multi-file and nested/broken-module vectors follow rust-analyzer's
//! hir-def/src/nameres/tests/mod_resolution.rs:1-96 (MIT OR Apache-2.0),
//! revision 3131751eaf1c3b5da6e618acdc2148ad7db65bc7. Explicit manifests
//! replace Rust discovery/reexports; no Rust precedence rules are imported.
//! The bounded leaf/branch enumeration follows proptest's recursive strategy
//! tests (recursive.rs:150-208, revision 70cf37d46b7d83393b6ed94c611f8c52209f585d,
//! MIT OR Apache-2.0); exhaustive small graphs replace random recursion.
use hirsel_driver::load_program;
use hirsel_engine::{DefinitionQueries, QueryError, with_compiler_stack};
use hirsel_syntax::{DefPath, ItemPath, PackageId};
use serde_json::{Value, json};
use std::{collections::BTreeSet, path::Path};

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Package(&'static str, u8);
impl Package {
    fn json(&self) -> Value {
        json!({"name": self.0, "version": self.1.to_string()})
    }
    fn manifest(&self) -> String {
        format!("{}-{}.json", self.0, self.1)
    }
}
const APP: Package = Package("app", 1);
const DEP: Package = Package("dep", 1);
const CORE: Package = Package("core", 1);

#[derive(Clone, Debug)]
struct Source {
    package: Package,
    module: Vec<&'static str>,
    item: &'static str,
    text: String,
}
#[derive(Clone, Debug)]
struct Edge {
    importer: Package,
    target: Package,
    aliases: Vec<(&'static str, Vec<&'static str>)>,
}
#[derive(Clone, Debug)]
struct Probe {
    importer: Package,
    qualified: Option<Package>,
    spelling: &'static str,
    is_type: bool,
}
#[derive(Debug, PartialEq, Eq)]
enum Answer {
    Found(String),
    Missing,
    MissingMember(BTreeSet<String>),
    Ambiguous(BTreeSet<String>),
}
#[derive(Debug)]
struct Graph {
    packages: Vec<Package>,
    sources: Vec<Source>,
    edges: Vec<Edge>,
    probes: Vec<Probe>,
}
fn identity(package: &Package, module: &[&str], item: &str) -> String {
    format!(
        "{}@{}/{}{}",
        package.0,
        package.1,
        module.iter().map(|m| format!("{m}/")).collect::<String>(),
        item
    )
}
impl Graph {
    fn value(&mut self, package: Package, module: &[&'static str], item: &'static str) {
        self.sources.push(Source {
            package,
            module: module.to_vec(),
            item,
            text: format!("pub fn {item}() -> u64 {{ 1u64 }}\n"),
        });
    }
    fn probe(
        &mut self,
        importer: Package,
        qualified: Option<Package>,
        spelling: &'static str,
        is_type: bool,
    ) {
        self.probes.push(Probe {
            importer,
            qualified,
            spelling,
            is_type,
        });
    }
    fn contains(&self, package: &Package, module: &[&str], item: &str) -> bool {
        (*package == CORE && module == ["string"] && item == "byte_len")
            || self
                .sources
                .iter()
                .any(|s| &s.package == package && s.module == module && s.item == item)
    }
    fn answer(&self, probe: &Probe) -> Answer {
        let mut segments: Vec<_> = probe.spelling.split('.').collect();
        let item = segments.pop().unwrap();
        if let Some(package) = &probe.qualified {
            return if self.contains(package, &segments, item) {
                Answer::Found(identity(package, &segments, item))
            } else {
                Answer::Missing
            };
        }
        // Probe functions live in `client`: lexical module, then package root.
        for module in [
            std::iter::once("client")
                .chain(segments.iter().copied())
                .collect(),
            segments.clone(),
        ] {
            if self.contains(&probe.importer, &module, item) {
                return Answer::Found(identity(&probe.importer, &module, item));
            }
        }
        let Some(head) = segments.first() else {
            return Answer::Missing;
        };
        // Exposure sets are owned by an importer. Direct edges supply implicit
        // first segments and explicit aliases; transitive edges supply neither.
        let mut targets = BTreeSet::new();
        for edge in self.edges.iter().filter(|e| e.importer == probe.importer) {
            for source in self.sources.iter().filter(|s| s.package == edge.target) {
                if source.module.first() == Some(head) {
                    targets.insert((edge.target.clone(), vec![*head]));
                }
            }
            for (alias, module) in &edge.aliases {
                if alias == head {
                    targets.insert((edge.target.clone(), module.clone()));
                }
            }
        }
        // Only the string default is needed by this finite model. Non-std
        // selections replace it; matching std modules merge with it.
        if *head == "string" && targets.iter().all(|(p, _)| p.0 == "std") {
            targets.insert((CORE, vec!["string"]));
        }
        if targets.is_empty() {
            return Answer::Missing;
        }
        let paths = || {
            targets
                .iter()
                .map(|(p, m)| {
                    let module: Vec<_> = m.iter().chain(&segments[1..]).copied().collect();
                    identity(p, &module, item)
                })
                .collect()
        };
        let merged = targets.len() == 2
            && targets.contains(&(CORE, vec!["string"]))
            && targets
                .iter()
                .any(|(p, m)| p.0 == "std" && m == &["string"]);
        if targets.len() > 1 && !merged {
            return Answer::Ambiguous(paths());
        }
        for (package, prefix) in &targets {
            let module: Vec<_> = prefix.iter().chain(&segments[1..]).copied().collect();
            if self.contains(package, &module, item) {
                return Answer::Found(identity(package, &module, item));
            }
        }
        Answer::MissingMember(paths())
    }

    /// Two serializations reverse every source/dependency/exposure array without
    /// changing package, module or item identity. Print before loading, so even
    /// setup errors retain the complete attempted multi-file fixture in the log.
    fn write(&self, dir: &Path, case: usize, reversed: bool) {
        let mut files = serde_json::Map::new();
        for package in &self.packages {
            let mut sources = vec![];
            for (i, source) in self
                .sources
                .iter()
                .enumerate()
                .filter(|(_, s)| s.package == *package)
            {
                let name = format!("source-{i}.hir");
                files.insert(name.clone(), json!(source.text));
                sources.push(json!({"path": name, "module": source.module}));
            }
            let mut text = String::new();
            for (i, probe) in self
                .probes
                .iter()
                .enumerate()
                .filter(|(_, p)| p.importer == *package)
            {
                let spelling = if let Some(p) = &probe.qualified {
                    format!("{}@{}/{}", p.0, p.1, probe.spelling)
                } else {
                    probe.spelling.into()
                };
                text.push_str(&if probe.is_type {
                    format!("pub fn probe_{i}(x: {spelling}) {{}}\n")
                } else {
                    format!("pub fn probe_{i}() -> u64 {{ {spelling}({}) }}\n", if probe.spelling == "string.byte_len" && matches!(self.answer(probe), Answer::Found(ref p) if p.starts_with("core@1/")) { "\"sample\"" } else { "" })
                });
            }
            let name = format!("{}-{}-probes.hir", package.0, package.1);
            files.insert(name.clone(), json!(text));
            sources.push(json!({"path": name, "module": ["client"]}));
            let mut dependencies = vec![];
            for edge in self.edges.iter().filter(|e| e.importer == *package) {
                let mut expose: Vec<_> = edge
                    .aliases
                    .iter()
                    .map(|(head, module)| json!({"head": head, "module": module}))
                    .collect();
                if reversed {
                    expose.reverse();
                }
                dependencies.push(json!({"manifest": edge.target.manifest(), "package": edge.target.json(), "expose": expose}));
            }
            if reversed {
                sources.reverse();
                dependencies.reverse();
            }
            let mut manifest = json!({"schema": 1, "package": package.json(), "sources": sources, "dependencies": dependencies});
            if *package == APP {
                manifest["entry"] = json!({"module": ["client"], "name": format!("probe_{}", self.probes.iter().position(|p| p.importer == APP).unwrap())});
            }
            files.insert(package.manifest(), json!(format!("{manifest}\n")));
        }
        let expected: Vec<_> = self
            .probes
            .iter()
            .map(|p| format!("{:?}", self.answer(p)))
            .collect();
        eprintln!(
            "PACKAGE_HEAD_22_FIXTURE {}",
            json!({"case":case,"reversed":reversed,"files":files,"expected":expected})
        );
        for (name, text) in files {
            std::fs::write(dir.join(name), text.as_str().unwrap()).unwrap();
        }
    }
}

fn graph(case: usize) -> Graph {
    let topology = case / 4;
    let duplicate = case & 1 != 0;
    let own = case & 2 != 0;
    let mut g = Graph {
        packages: vec![APP, DEP],
        sources: vec![],
        edges: vec![],
        probes: vec![],
    };
    g.value(DEP, &["api", "nested"], "value");
    g.value(DEP, &["api", "nested", "child"], "value");
    g.value(DEP, &["elsewhere"], "other");
    let mut aliases = vec![("renamed", vec!["api", "nested"])];
    if topology != 5 {
        aliases.push(("vendor", vec!["api", "nested"]));
    }
    if duplicate {
        aliases.extend(aliases.clone());
    }
    g.edges.push(Edge {
        importer: APP,
        target: DEP,
        aliases,
    });
    if own {
        g.value(APP, &["vendor"], "value");
        g.value(APP, &["vendor"], "root_value");
        g.value(APP, &["client", "vendor"], "value");
    }
    match topology {
        1 | 2 => {
            let third = if topology == 1 {
                Package("dep", 2)
            } else {
                Package("other", 1)
            };
            g.packages.push(third.clone());
            // Only DEP has `value`; ambiguity must be decided before membership.
            g.value(third.clone(), &["api", "nested"], "other");
            g.edges.push(Edge {
                importer: APP,
                target: third.clone(),
                aliases: vec![("vendor", vec!["api", "nested"])],
            });
            g.probe(APP, Some(third), "api.nested.other", false);
        }
        3 => g.edges[0].aliases.push(("vendor", vec!["elsewhere"])),
        4 | 5 => {
            let third = Package("dep", 2);
            g.packages.push(third.clone());
            g.value(third.clone(), &["api", "nested"], "value");
            g.sources.push(Source {
                package: third.clone(),
                module: vec!["types"],
                item: "Token",
                text: "pub type Token { n: u64 }\n".into(),
            });
            g.sources.push(Source {
                package: DEP,
                module: vec!["bridge"],
                item: "pass",
                text: "pub fn pass(x: dep@2/types.Token) -> dep@2/types.Token { x }\n".into(),
            });
            g.edges.push(Edge {
                importer: DEP,
                target: third.clone(),
                aliases: vec![
                    ("vendor", vec!["api", "nested"]),
                    ("importer_only", vec!["api", "nested"]),
                ],
            });
            g.probe(DEP, None, "vendor.value", false);
            g.probe(DEP, None, "renamed.value", false);
            g.probe(APP, None, "importer_only.value", false);
            g.probe(APP, None, "types.Token", true);
            g.probe(APP, Some(third), "types.Token", true);
        }
        6 => {
            g.edges[0].aliases.push(("string", vec!["api", "nested"]));
            g.probe(APP, None, "string.value", false);
            g.probe(APP, None, "string.byte_len", false);
        }
        7 => {
            let std = Package("std", 1);
            g.packages.push(std.clone());
            g.value(std.clone(), &["string"], "find");
            g.edges.push(Edge {
                importer: APP,
                target: std,
                aliases: vec![],
            });
            g.probe(APP, None, "string.byte_len", false);
            g.probe(APP, None, "string.find", false);
            g.probe(APP, None, "string.absent", false);
        }
        _ => {}
    }
    for spelling in [
        "vendor.value",
        "vendor.root_value",
        "vendor.child.value",
        "renamed.value",
        "api.nested.value",
        "value",
    ] {
        g.probe(APP, None, spelling, false);
    }
    g.probe(APP, Some(DEP), "api.nested.value", false);
    g.probe(APP, Some(Package("dep", 9)), "api.nested.value", false);
    g
}

#[test]
fn package_head_22_finite_graph_and_manifest_permutations() {
    with_compiler_stack(|| {
        for case in 0..32 {
            let graph = graph(case);
            let mut original = None;
            for reversed in [false, true] {
                let scratch = tempfile::tempdir().unwrap();
                graph.write(scratch.path(), case, reversed);
                let mut program = load_program(&scratch.path().join(APP.manifest())).unwrap();
                if case / 4 == 4 || case / 4 == 5 {
                    let bridge = DefPath(ItemPath {
                        package: PackageId {
                            name: "dep".into(),
                            version: "1".into(),
                        },
                        modules: vec!["bridge".into()],
                        name: "pass".into(),
                    });
                    let checked = program.engine.check_definition(&bridge).unwrap();
                    assert!(
                        checked.diagnostics.is_empty(),
                        "case {case}/{reversed}: {:?}",
                        checked.diagnostics
                    );
                }
                let mut observed = vec![];
                for (i, probe) in graph.probes.iter().enumerate() {
                    let path = DefPath(ItemPath {
                        package: PackageId {
                            name: probe.importer.0.into(),
                            version: probe.importer.1.to_string().into(),
                        },
                        modules: vec!["client".into()],
                        name: format!("probe_{i}").into(),
                    });
                    let (dependencies, diagnostics) = match program.engine.resolve_definition(&path)
                    {
                        Ok(output) => (output.value.dependencies.clone(), output.diagnostics),
                        Err(QueryError::Diagnostics(diagnostics)) => (vec![], diagnostics),
                        Err(error) => panic!(
                            "case {case}/{reversed}, {probe:?}: operational/setup failure: {error:?}"
                        ),
                    };
                    let expected = graph.answer(probe);
                    let actual = match &expected {
                        Answer::Found(target) => {
                            assert!(
                                diagnostics.is_empty(),
                                "case {case}/{reversed}, {probe:?}: {:?}",
                                diagnostics
                            );
                            let paths: BTreeSet<_> =
                                dependencies.iter().map(ToString::to_string).collect();
                            assert_eq!(
                                paths,
                                BTreeSet::from([target.clone()]),
                                "case {case}/{reversed}, {probe:?}"
                            );
                            let checked = program.engine.check_definition(&path).unwrap();
                            assert!(
                                checked.diagnostics.is_empty(),
                                "case {case}/{reversed}, {probe:?}: {:?}",
                                checked.diagnostics
                            );
                            format!("found {target}")
                        }
                        error => {
                            let [d] = &diagnostics[..] else {
                                panic!("case {case}/{reversed}, {probe:?}: {:?}", diagnostics)
                            };
                            assert_eq!(d.rule.as_str(), "LANG-4", "{d:?}");
                            let prefix = match error {
                                Answer::Ambiguous(_) => "ambiguous module head",
                                Answer::MissingMember(_) => "unresolved module member",
                                _ => "unresolved name",
                            };
                            assert!(
                                d.message.starts_with(prefix),
                                "case {case}/{reversed}, {probe:?}: {d:?}"
                            );
                            if let Answer::Ambiguous(targets) | Answer::MissingMember(targets) =
                                error
                            {
                                let listed = d
                                    .message
                                    .split_once(if matches!(error, Answer::Ambiguous(_)) {
                                        ": "
                                    } else {
                                        "looked for "
                                    })
                                    .unwrap()
                                    .1
                                    .split(';')
                                    .next()
                                    .unwrap();
                                assert_eq!(
                                    listed
                                        .split(", ")
                                        .map(str::to_owned)
                                        .collect::<BTreeSet<_>>(),
                                    *targets,
                                    "case {case}/{reversed}, {probe:?}"
                                );
                            }
                            d.message.clone()
                        }
                    };
                    observed.push(actual);
                }
                eprintln!(
                    "PACKAGE_HEAD_22_RESULT {}",
                    json!({"case":case,"reversed":reversed,"observed":observed})
                );
                if let Some(previous) = &original {
                    assert_eq!(&observed, previous, "case {case} permutation");
                }
                original = Some(observed);
            }
        }
    });
}
