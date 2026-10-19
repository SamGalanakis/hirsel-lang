//! B12/B13: independently computed path and program-check outcomes, with no
//! recorded host fault fed back into the interpreter as its own oracle.
use hirsel_interp::{
    OperationCall, OperationContext, OperationResult, Operations, Value, library as reference,
};
use hirsel_rt::{io::*, io_real::RealIo, io_sim::SimIo};
use hirsel_span::{SourceMap, Span};
use hirsel_tir::RunConfiguration;
use std::collections::BTreeMap;
use std::path::PathBuf;

struct Reference {
    run: RunConfiguration,
    library: reference::Library,
}
impl Reference {
    fn new(tree: &Tree) -> Self {
        Self::configured(tree, Default::default())
    }
    fn configured(tree: &Tree, config: reference::SimulationConfig) -> Self {
        let run = RunConfiguration {
            entry: hirsel_syntax::DefPath::core("main"),
            hash_seed: 0,
            inputs: vec![],
        };
        let mut library = reference::Library::new(
            &run,
            reference::SimulationConfig {
                files: tree.files.clone(),
                directories: tree.directories.clone(),
                links: tree.links.clone(),
                ..config
            },
        )
        .unwrap();
        assert_eq!(
            library.simulation.grant_root("Io").unwrap(),
            handle("Io", 1)
        );
        Self { run, library }
    }
    fn raw_call(
        &mut self,
        name: &str,
        arguments: Vec<Value>,
        bounded: bool,
    ) -> Result<OperationResult, hirsel_interp::Error> {
        let mut types = hirsel_types::TypeStore::default();
        let unit = types.push(hirsel_types::Type::unit());
        let result_type = if bounded {
            types.push(hirsel_types::Type::with_kind(
                hirsel_types::TypeKind::Named {
                    definition: hirsel_syntax::DefPath::core("Bounded"),
                    arguments: vec![hirsel_types::GenericArgument::Type(unit)].into(),
                },
            ))
        } else {
            unit
        };
        let sources = SourceMap::default();
        let mut output = vec![];
        let mut trace = vec![];
        let path = hirsel_syntax::DefPath::core(name);
        let result = self
            .library
            .call(
                OperationCall {
                    runtime: &Default::default(),
                    path: &path,
                    generics: &[],
                    arguments: &arguments,
                    result_type,
                    types: &types,
                    site: Span::default(),
                },
                &mut OperationContext {
                    configuration: &self.run,
                    sources: &sources,
                    stdout: &mut output,
                    trace: &mut trace,
                    trace_mode: Default::default(),
                    anchor: None,
                },
            )?
            .expect("known core operation");
        Ok(result)
    }
    fn call(&mut self, name: &str, arguments: Vec<Value>) -> Result<Value, hirsel_interp::Error> {
        match self.raw_call(name, arguments, false)? {
            OperationResult::Complete { value, .. } => Ok(value),
            other => panic!("unexpected {name}: {other:?}"),
        }
    }
}
fn handle(kind: &str, id: u64) -> Value {
    Value::Handle {
        kind: kind.into(),
        id,
    }
}

struct Scratch(PathBuf);
impl Scratch {
    fn new() -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "hirsel-io-property-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}

struct Draw(u64);
impl Draw {
    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        self.0 ^ (self.0 >> 29)
    }
    fn pick<'a>(&mut self, choices: &'a [&'a str]) -> &'a str {
        choices[self.next() as usize % choices.len()]
    }
}

/// A small recursive specification: resolve one prefix, then the remaining
/// suffix. Link expansion restarts that prefix with a shared hop budget. It
/// uses neither of the implementations' walkers nor host canonicalize (which
/// follows absolute links and can leave the grant).
fn read_reference(tree: &Tree, root: &str, path: &str) -> Result<Vec<u8>, IoError> {
    fn legal(path: &str) -> bool {
        !path.starts_with('/') && !path.contains(['\\', '\0'])
    }
    if !legal(path) {
        return Err(IoError::Fault(1));
    }
    let mut normalized = vec![];
    for part in path.split('/') {
        match part {
            "" | "." => (),
            ".." => {
                normalized.pop().ok_or(IoError::Fault(1))?;
            }
            _ => normalized.push(part),
        }
    }
    fn resolve(
        tree: &Tree,
        root: &str,
        at: String,
        parts: &[&str],
        hops: &mut usize,
    ) -> Result<String, IoError> {
        let Some((part, tail)) = parts.split_first() else {
            return Ok(at);
        };
        match *part {
            "" | "." => resolve(tree, root, at, tail, hops),
            ".." => {
                if at == root {
                    return Err(IoError::Fault(1));
                }
                resolve(
                    tree,
                    root,
                    at.rsplit_once('/').map_or("", |(p, _)| p).into(),
                    tail,
                    hops,
                )
            }
            name => {
                let child = if at.is_empty() {
                    name.into()
                } else {
                    format!("{at}/{name}")
                };
                if let Some(target) = tree.links.get(&child) {
                    *hops += 1;
                    if *hops > 40 || !legal(target) {
                        return Err(IoError::Fault(1));
                    }
                    let expanded = format!(
                        "{target}{}{}",
                        if tail.is_empty() { "" } else { "/" },
                        tail.join("/")
                    );
                    resolve(
                        tree,
                        root,
                        at,
                        &expanded.split('/').collect::<Vec<_>>(),
                        hops,
                    )
                } else {
                    if !tail.is_empty() && !tree.directories.contains(&child) {
                        return Err(if tree.files.contains_key(&child) {
                            IoError::NotDirectory
                        } else {
                            IoError::NotFound
                        });
                    }
                    resolve(tree, root, child, tail, hops)
                }
            }
        }
    }
    let resolved = resolve(tree, root, root.into(), &normalized, &mut 0)?;
    if tree.directories.contains(&resolved) {
        return Err(IoError::IsDirectory);
    }
    tree.files.get(&resolved).cloned().ok_or(IoError::NotFound)
}

fn confinement(seeds: u64, draws: usize) {
    let mut count = 0;
    for seed in 0..seeds {
        let mut rng = Draw(seed);
        let scratch = Scratch::new();
        let mut tree = Tree::new(BTreeMap::from([
            ("top".into(), vec![seed as u8, 255]),
            ("d0/f".into(), b"zero".to_vec()),
            ("d0/n/f".into(), b"nested".to_vec()),
            ("d1/f".into(), b"one".to_vec()),
        ]))
        .unwrap();
        for i in 0..8 {
            let parent = rng.pick(&["", "d0/", "d0/n/", "d1/"]);
            let target = rng.pick(&[
                "d0",
                "d1/f",
                "../top",
                "../d1",
                "../../top",
                "f",
                "n/f",
                "/top",
                "missing",
                ".",
                "..",
                "cycle",
            ]);
            tree.links.insert(format!("{parent}l{i}"), target.into());
        }
        tree.links.extend([
            ("alias".into(), "d0".into()),
            ("d0/up".into(), "../top".into()),
            ("cycle".into(), "cycle".into()),
        ]);
        for directory in &tree.directories {
            std::fs::create_dir_all(scratch.0.join(directory)).unwrap();
        }
        for (name, bytes) in &tree.files {
            std::fs::write(scratch.0.join(name), bytes).unwrap();
        }
        for (name, target) in &tree.links {
            std::os::unix::fs::symlink(target, scratch.0.join(name)).unwrap();
        }
        let mut real = RealIo::grant(&scratch.0).unwrap();
        let mut reference = Reference::new(&tree);
        let mut tape = vec![];
        for (request, name, arguments, expected) in [
            (
                Request::Directory {
                    io: 1,
                    path: String::new(),
                },
                "io.dir",
                vec![handle("Io", 1), reference::path("")],
                Completion::Handle(Ok(2)),
            ),
            (
                Request::NarrowDir {
                    directory: 2,
                    path: "d0".into(),
                },
                "dir.narrow",
                vec![handle("Dir", 2), reference::path("d0")],
                Completion::Handle(Ok(3)),
            ),
        ] {
            assert_eq!(real.call(request.clone()), Ok(expected.clone()));
            assert_eq!(
                crate::support::io::decode(&expected, reference.call(name, arguments).unwrap()),
                expected
            );
            tape.push(Event {
                request,
                completion: expected,
                fault: None,
            });
        }
        let mut paths: Vec<String> = [
            "top",
            "d0/f",
            "alias/n/../f",
            "alias/../top",
            "d0/up",
            "../top",
            "cycle",
            "d0/f/../f",
            "missing/../top",
            "d0/n/../../top",
            "/top",
            "a\\b",
            "a\0b",
            "",
            ".",
            "d0/",
            "d0/f/",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect();
        paths.extend(tree.links.keys().cloned());
        for _ in 0..draws {
            let mut path = String::new();
            for i in 0..(1 + rng.next() % 6) {
                if i > 0 {
                    path.push('/');
                }
                path.push_str(rng.pick(&[
                    "d0", "d1", "n", "f", "top", "l0", "l1", "l2", "alias", "up", "..", ".", "",
                    "missing",
                ]));
            }
            paths.push(path);
        }
        for (directory, root) in [(2, ""), (3, "d0")] {
            for path in &paths {
                let expected = Completion::Bytes(read_reference(&tree, root, path));
                let request = Request::Read {
                    directory,
                    path: path.clone(),
                };
                assert_eq!(
                    real.call(request.clone()),
                    Ok(expected.clone()),
                    "B12 real seed={seed} root={root:?} path={path:?} links={:?}",
                    tree.links
                );
                let value = reference
                    .call(
                        "file.read_bytes",
                        vec![handle("Dir", directory), reference::path(path)],
                    )
                    .unwrap();
                assert_eq!(
                    crate::support::io::decode(&expected, value),
                    expected,
                    "B12 interpreter seed={seed} root={root:?} path={path:?}"
                );
                tape.push(Event {
                    request,
                    completion: expected,
                    fault: None,
                });
                count += 1;
            }
        }
        // No injected host error: the replay walker must independently derive
        // every expected refusal and every successful byte sequence.
        let mut sim = SimIo::new(tree, tape.clone());
        for event in tape {
            assert_eq!(
                sim.call(event.request.clone()),
                Ok(event.completion),
                "B12 replay seed={seed} {:?}",
                event.request
            );
        }
        sim.finish().unwrap();
    }
    eprintln!("B12: {count} reads, three implementations and an independent reference");
}
#[test]
fn generated_confinement_differential() {
    confinement(4, 32);
}
#[test]
#[ignore = "on-demand expanded confinement differential"]
fn full_gate_generated_confinement_differential() {
    confinement(128, 256);
}
