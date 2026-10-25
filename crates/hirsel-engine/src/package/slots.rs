//! PROG-1 implementation slots: a schema-2 manifest declares named slots, each
//! selecting edge fills them, and every package/version has one effective map.
//!
//! Prior art: dune virtual libraries allow one implementation per executable
//! and report a second with both requiring chains (src/dune_rules/lib.ml:152,
//! 1046) and a default-implementation cycle by its members (lib.ml:2047); Cargo
//! names both shortest dependency paths of a duplicate `links` (src/compiler/
//! links.rs:34-56, src/util/graph.rs:141) and normalizes a renamed dependency to
//! its package (src/workspace/dependency.rs:222,244). Unlike Cargo features,
//! slot maps never union: unequal maps refuse.
use super::*;

/// `{dependency, module, name}`: a declaration written in one manifest frame,
/// whose `dependency` is a catalog ID of that frame or `self`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct DeclarationRef {
    dependency: String,
    modules: Vec<String>,
    name: String,
}

/// A slot's actual: one exact implementation declaration, or the whole
/// effective slot of a selected dependency (or of this frame).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Binding {
    Implementation(DeclarationRef),
    Parameter { dependency: String, name: String },
}

/// A declared slot; without a default its actual is required.
pub(super) struct Slot {
    signature: DeclarationRef,
    default: Option<Binding>,
}

/// One edge selecting a package. Its bindings are written in `parent`'s frame;
/// the root selection has no parent and no bindings.
pub(super) struct Selection {
    pub(super) parent: Option<PackageId>,
    pub(super) edge: Option<String>,
    pub(super) bindings: BTreeMap<String, Binding>,
}

/// A package's effective slot: its signature and the terminal implementation
/// declaration that fills it. Configuration never enters nominal identity.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImplementationSlot {
    pub signature: DefPath,
    pub implementation: DefPath,
}

/// Every selected package's effective slots, keyed by slot name.
pub type PackageSlots = BTreeMap<PackageId, BTreeMap<String, ImplementationSlot>>;

pub(super) fn declaration_ref(v: &Value) -> Result<DeclarationRef, ToolError> {
    let o = object_value(v, &["dependency", "module", "name"])?;
    Ok(DeclarationRef {
        dependency: identifier(required(o, "dependency")?)?,
        modules: modules(required(o, "module")?)?,
        name: identifier(required(o, "name")?)?,
    })
}

pub(super) fn binding(v: &Value) -> Result<Binding, ToolError> {
    let o = v
        .as_object()
        .ok_or_else(|| invalid("expected binding object"))?;
    match (o.len(), o.get("implementation"), o.get("parameter")) {
        (1, Some(target), None) => Ok(Binding::Implementation(declaration_ref(target)?)),
        (1, None, Some(target)) => {
            let p = object_value(target, &["dependency", "name"])?;
            Ok(Binding::Parameter {
                dependency: identifier(required(p, "dependency")?)?,
                name: identifier(required(p, "name")?)?,
            })
        }
        _ => Err(invalid(
            "a binding is exactly one of {\"implementation\": ..} or {\"parameter\": ..}",
        )),
    }
}

pub(super) fn bindings(v: &Value) -> Result<BTreeMap<String, Binding>, ToolError> {
    v.as_object()
        .ok_or_else(|| invalid("bindings must be an object"))?
        .iter()
        .map(|(slot, b)| Ok((identifier(&Value::String(slot.clone()))?, binding(b)?)))
        .collect()
}

pub(super) fn parameters(v: &Value) -> Result<BTreeMap<String, Slot>, ToolError> {
    v.as_object()
        .ok_or_else(|| invalid("parameters must be an object"))?
        .iter()
        .map(|(slot, s)| {
            let s = object_value(s, &["signature", "default"])?;
            Ok((
                identifier(&Value::String(slot.clone()))?,
                Slot {
                    signature: declaration_ref(required(s, "signature")?)?,
                    default: s.get("default").map(binding).transpose()?,
                },
            ))
        })
        .collect()
}

/// A frame's references name its own catalog, selected edges and slots, even
/// inside a default an override suppresses.
pub(super) fn check_frame(
    package: &PackageId,
    catalog: &BTreeMap<String, (PathBuf, PackageId)>,
    uses: &BTreeMap<String, PackageId>,
    parameters: &BTreeMap<String, Slot>,
    written: impl IntoIterator<Item = Binding>,
) -> Result<(), ToolError> {
    let frame = spelling(package);
    let check_ref = |r: &DeclarationRef| {
        if r.dependency == "self" || catalog.contains_key(&r.dependency) {
            Ok(())
        } else {
            Err(invalid(&format!(
                "{frame} names {:?}, which is not in its dependency_catalog",
                r.dependency
            )))
        }
    };
    let check = |b: &Binding| match b {
        Binding::Implementation(r) => check_ref(r),
        Binding::Parameter { dependency, name } if dependency == "self" => {
            if parameters.contains_key(name) {
                Ok(())
            } else {
                Err(invalid(&format!(
                    "{frame} forwards its undeclared slot {name}"
                )))
            }
        }
        Binding::Parameter { dependency, .. } => {
            if uses.contains_key(dependency) {
                Ok(())
            } else {
                Err(invalid(&format!(
                    "{frame} forwards a slot of {dependency:?}, which it does not select"
                )))
            }
        }
    };
    for slot in parameters.values() {
        check_ref(&slot.signature)?;
        if let Some(default) = &slot.default {
            check(default)?;
        }
    }
    for b in written {
        check(&b)?;
    }
    Ok(())
}

fn spelling(id: &PackageId) -> String {
    format!("{}@{}", id.name, id.version)
}

/// A chain of selecting edges from the root, as `(edge label, package)`.
type ChainPath = Vec<(Option<String>, PackageId)>;

fn render(path: &ChainPath) -> String {
    let mut out = String::new();
    for (i, (edge, package)) in path.iter().enumerate() {
        if i > 0 {
            out.push_str(" -> ");
        }
        out.push_str(&spelling(package));
        if let Some(edge) = edge.as_ref().filter(|e| **e != package.name) {
            out.push_str(&format!(" (as {edge})"));
        }
    }
    out
}

fn render_map(map: &BTreeMap<String, DefPath>) -> String {
    let slots: Vec<_> = map.iter().map(|(s, d)| format!("{s} = {d}")).collect();
    format!("{{{}}}", slots.join(", "))
}

/// Order selecting paths shortest first, then by spelling, independent of
/// manifest and edge order (as Cargo's breadth-first `path_to_top`).
fn path_key(path: &ChainPath) -> (usize, &ChainPath) {
    (path.len(), path)
}

impl Graph<'_> {
    /// Each selected package's canonical selecting path: its shortest chain of
    /// edges from the root, ties broken by spelling.
    fn selecting_paths(&self) -> BTreeMap<PackageId, ChainPath> {
        let mut best = BTreeMap::new();
        for (package, selections) in &self.selections {
            if selections.iter().any(|s| s.parent.is_none()) {
                best.insert(package.clone(), vec![(None, package.clone())]);
            }
        }
        let mut frontier: BTreeSet<PackageId> = best.keys().cloned().collect();
        while !frontier.is_empty() {
            let mut next: BTreeMap<PackageId, ChainPath> = BTreeMap::new();
            for (package, selections) in &self.selections {
                if best.contains_key(package) {
                    continue;
                }
                for selection in selections {
                    let Some(parent) = selection.parent.as_ref().filter(|p| frontier.contains(*p))
                    else {
                        continue;
                    };
                    let mut path: ChainPath = best[parent].clone();
                    path.push((selection.edge.clone(), package.clone()));
                    match next.get(package) {
                        Some(old) if path_key(old) <= path_key(&path) => {}
                        _ => {
                            next.insert(package.clone(), path);
                        }
                    }
                }
            }
            frontier = next.keys().cloned().collect();
            best.extend(next);
        }
        best
    }

    /// Expand every selected package's slots to terminal implementation
    /// identities, selecting the catalog targets a used reference names, then
    /// refuse a package/version selected with two unequal maps.
    pub(super) fn expand_slots(&mut self) -> Result<(), ToolError> {
        loop {
            let (maps, activate, error) = {
                let paths = self.selecting_paths();
                let mut expansion = Expansion::new(self, &paths);
                let maps = expansion.run();
                (maps, expansion.activate, expansion.error)
            };
            if !activate.is_empty() {
                for (writer, edge) in activate {
                    self.activate(&writer, &edge)?;
                }
                continue;
            }
            if let Some(error) = error {
                return Err(invalid(&error));
            }
            let mut slots = PackageSlots::new();
            for (package, mut selections) in maps {
                selections.sort_by(|a, b| path_key(&a.0).cmp(&path_key(&b.0)));
                let (first_path, first) = &selections[0];
                if let Some((path, map)) = selections.iter().find(|(_, map)| map != first) {
                    return Err(invalid(&format!(
                        "{} is selected with two unequal implementation slot maps: {} selects \
                         {}, and {} selects {}",
                        spelling(&package),
                        render(first_path),
                        render_map(first),
                        render(path),
                        render_map(map)
                    )));
                }
                let declared = &self.packages[&package].parameters;
                if declared.is_empty() {
                    continue;
                }
                let signatures = &signatures(self, &package);
                slots.insert(
                    package,
                    first
                        .iter()
                        .map(|(slot, implementation)| {
                            (
                                slot.clone(),
                                ImplementationSlot {
                                    signature: signatures[slot].clone(),
                                    implementation: implementation.clone(),
                                },
                            )
                        })
                        .collect(),
                );
            }
            self.slots = slots;
            return Ok(());
        }
    }

    /// Select a catalog target a used reference names, as an edge of the
    /// writer's frame with that target's own defaults and no exposures.
    fn activate(&mut self, writer: &PackageId, edge: &str) -> Result<(), ToolError> {
        let (manifest, expected) = self.packages[writer].catalog[edge].clone();
        let selected = self.visit_from(
            &manifest,
            Some(&expected),
            Selection {
                parent: Some(writer.clone()),
                edge: Some(edge.into()),
                bindings: BTreeMap::new(),
            },
        )?;
        let root = self
            .selections
            .iter()
            .any(|(p, s)| p == writer && s.iter().any(|s| s.parent.is_none()));
        if !root && self.packages[&selected].unsafe_ffi.is_some() {
            return Err(invalid(&format!(
                "only the root launch manifest may select the unsafe ffi package {}",
                spelling(&selected)
            )));
        }
        let dependencies = &mut self.packages.get_mut(writer).unwrap().dependencies;
        if !dependencies.contains(&selected) {
            dependencies.push(selected);
        }
        Ok(())
    }
}

/// The signature each declared slot of `package` names, in its own frame.
fn signatures(graph: &Graph, package: &PackageId) -> BTreeMap<String, DefPath> {
    let selected = &graph.packages[package];
    selected
        .parameters
        .iter()
        .map(|(slot, s)| (slot.clone(), terminal(graph, package, &s.signature)))
        .collect()
}

/// The nominal declaration a reference names: catalog aliases of one package
/// normalize to that package (as a Cargo rename normalizes to its package).
fn terminal(graph: &Graph, frame: &PackageId, r: &DeclarationRef) -> DefPath {
    let package = if r.dependency == "self" {
        frame.clone()
    } else {
        graph.packages[frame].catalog[&r.dependency].1.clone()
    };
    DefPath(hirsel_syntax::ItemPath {
        package,
        modules: r.modules.clone(),
        name: r.name.clone(),
    })
}

/// One expansion round over the current selected graph.
struct Expansion<'g, 'a> {
    graph: &'g Graph<'a>,
    paths: &'g BTreeMap<PackageId, ChainPath>,
    /// The canonical (shortest-path) selection's value of each `(package, slot)`.
    effective: BTreeMap<(PackageId, String), Result<DefPath, String>>,
    stack: Vec<(PackageId, String)>,
    /// Catalog targets a used reference names that the writer has not selected.
    activate: BTreeSet<(PackageId, String)>,
    /// The first refusal in package, path and slot order.
    error: Option<String>,
}
type SelectionMaps = BTreeMap<PackageId, Vec<(ChainPath, BTreeMap<String, DefPath>)>>;

impl<'g, 'a> Expansion<'g, 'a> {
    fn new(graph: &'g Graph<'a>, paths: &'g BTreeMap<PackageId, ChainPath>) -> Self {
        Self {
            graph,
            paths,
            effective: BTreeMap::new(),
            stack: Vec::new(),
            activate: BTreeSet::new(),
            error: None,
        }
    }

    fn fail(&mut self, error: String) {
        self.error.get_or_insert(error);
    }

    /// Every selection's complete map, keyed by its selecting path.
    fn run(&mut self) -> SelectionMaps {
        let mut maps = SelectionMaps::new();
        for (package, selections) in &self.graph.selections {
            let declared = &self.graph.packages[package].parameters;
            for slot in declared.values() {
                self.reference(package, &slot.signature);
            }
            let mut ordered: Vec<_> = selections
                .iter()
                .map(|selection| (self.selection_path(package, selection), selection))
                .collect();
            ordered.sort_by(|a, b| path_key(&a.0).cmp(&path_key(&b.0)));
            for (path, selection) in ordered {
                if let Some(slot) = selection
                    .bindings
                    .keys()
                    .find(|s| !declared.contains_key(*s))
                {
                    self.fail(format!(
                        "{} binds {slot}, which {} does not declare as a slot",
                        render(&path),
                        spelling(package)
                    ));
                    continue;
                }
                // The canonical selection's slots are the effective ones, so
                // a cycle is reported from the slot it was entered by.
                let canonical = self.paths.get(package) == Some(&path);
                let mut map = BTreeMap::new();
                for slot in declared.keys() {
                    let value = if canonical {
                        self.effective(package, slot)
                    } else {
                        self.through(package, selection, &path, slot)
                    };
                    match value {
                        Ok(target) => {
                            map.insert(slot.clone(), target);
                        }
                        Err(error) => self.fail(error),
                    }
                }
                maps.entry(package.clone()).or_default().push((path, map));
            }
        }
        maps
    }

    fn selection_path(&self, package: &PackageId, selection: &Selection) -> ChainPath {
        match &selection.parent {
            None => vec![(None, package.clone())],
            Some(parent) => {
                let mut path = self.paths.get(parent).cloned().unwrap_or_default();
                path.push((selection.edge.clone(), package.clone()));
                path
            }
        }
    }

    /// A slot's value through one selection: the edge's actual in the writer's
    /// frame, else the default in the provider's frame. An overridden default
    /// is never evaluated, so its provider is never opened.
    fn through(
        &mut self,
        package: &PackageId,
        selection: &Selection,
        path: &ChainPath,
        slot: &str,
    ) -> Result<DefPath, String> {
        if let Some(actual) = selection.bindings.get(slot) {
            let writer = selection.parent.as_ref().unwrap();
            return self.binding(writer, actual);
        }
        match &self.graph.packages[package].parameters[slot].default {
            Some(default) => self.binding(package, default),
            None => Err(format!(
                "{}'s slot {slot} is required, and {} does not fill it",
                spelling(package),
                render(path)
            )),
        }
    }

    fn binding(&mut self, frame: &PackageId, binding: &Binding) -> Result<DefPath, String> {
        match binding {
            Binding::Implementation(r) => Ok(self.reference(frame, r)),
            Binding::Parameter { dependency, name } => {
                let package = if dependency == "self" {
                    frame.clone()
                } else {
                    self.graph.packages[frame].uses[dependency].clone()
                };
                self.effective(&package, name)
            }
        }
    }

    /// A reference's terminal declaration, selecting its catalog target as an
    /// edge of the writer's frame when the frame has not selected it.
    fn reference(&mut self, frame: &PackageId, r: &DeclarationRef) -> DefPath {
        let target = terminal(self.graph, frame, r);
        if r.dependency != "self" && !self.graph.packages[frame].uses.contains_key(&r.dependency) {
            let selected = self
                .graph
                .selections
                .get(&target.0.package)
                .is_some_and(|s| {
                    s.iter().any(|s| {
                        s.parent.as_ref() == Some(frame) && s.edge.as_deref() == Some(&r.dependency)
                    })
                });
            if !selected {
                self.activate.insert((frame.clone(), r.dependency.clone()));
            }
        }
        target
    }

    /// A package's effective slot: its canonical selection's value. A slot
    /// reached again while it is being evaluated is a forwarding cycle.
    fn effective(&mut self, package: &PackageId, slot: &str) -> Result<DefPath, String> {
        let key = (package.clone(), slot.to_string());
        if let Some(value) = self.effective.get(&key) {
            return value.clone();
        }
        if let Some(start) = self.stack.iter().position(|k| *k == key) {
            let cycle: Vec<_> = self.stack[start..]
                .iter()
                .chain(std::iter::once(&key))
                .map(|(p, s)| format!("{}.{s}", spelling(p)))
                .collect();
            return Err(format!(
                "implementation slot forwarding cycle: {}",
                cycle.join(" -> ")
            ));
        }
        let Some(selected) = self.graph.packages.get(package) else {
            return Err(format!("{} is not selected", spelling(package)));
        };
        if !selected.parameters.contains_key(slot) {
            return Err(format!(
                "a binding forwards {}.{slot}, which is not a declared slot",
                spelling(package)
            ));
        }
        let selections = &self.graph.selections[package];
        let Some((path, selection)) = selections
            .iter()
            .map(|s| (self.selection_path(package, s), s))
            .min_by(|a, b| path_key(&a.0).cmp(&path_key(&b.0)))
        else {
            return Err(format!("{} is not selected", spelling(package)));
        };
        self.stack.push(key.clone());
        let value = self.through(package, selection, &path, slot);
        self.stack.pop();
        self.effective.insert(key, value.clone());
        value
    }
}

/// The canonical bytes of one package's effective map: slots in identifier
/// order, each with its terminal implementation path. Provenance (explicit or
/// omitted default, catalog alias, forwarding) is not part of it.
pub(super) fn canonical(map: &BTreeMap<String, ImplementationSlot>) -> Vec<u8> {
    let mut out = Vec::new();
    for (slot, s) in map {
        for part in [slot.clone(), s.implementation.to_string()] {
            out.extend((part.len() as u64).to_le_bytes());
            out.extend(part.as_bytes());
        }
    }
    out
}

/// Every slot's implementation implements the slot's signature, and its map
/// matches that signature through the GEN-6 matcher. A pipeline without
/// signatures or the matcher cannot decide and leaves this to checking.
pub(super) fn check_conformance(
    engine: &mut QueryEngine,
    slots: &PackageSlots,
) -> Result<(), ToolError> {
    let Some(checker) = engine.adapters.checker.clone() else {
        return Ok(());
    };
    let query = |engine: &mut QueryEngine, error: QueryError| ToolError::Query {
        error,
        phase: None,
        sources: engine.sources.clone(),
    };
    for (package, map) in slots {
        for (slot, s) in map {
            let owner = format!("{}'s slot {slot}", spelling(package));
            let family = match engine.signature(DefId::intern(&s.signature)) {
                Err(QueryError::NotImplemented) => return Ok(()),
                result => result.map_err(|e| query(engine, e))?,
            };
            if !matches!(family.kind, hirsel_types::SignatureKind::Family(_)) {
                return Err(invalid(&format!(
                    "{owner} names the signature {}, which is not a signature",
                    s.signature
                )));
            }
            let actual = engine
                .signature(DefId::intern(&s.implementation))
                .map_err(|e| query(engine, e))?;
            let hirsel_types::SignatureKind::Implementation(actuals) = &actual.kind else {
                return Err(invalid(&format!(
                    "{owner} names {}, which is not an implementation",
                    s.implementation
                )));
            };
            if actuals.family != DefId::intern(&s.signature) {
                return Err(invalid(&format!(
                    "{owner} needs an implementation of {}; {} implements {}",
                    s.signature,
                    s.implementation,
                    actuals.family.path()
                )));
            }
            let (types, memo) = (engine.types.clone(), engine.memo.clone());
            match checker.match_implementation(&types, &memo, &s.signature, actuals, engine) {
                Ok(None) | Err(QueryError::NotImplemented) => {}
                Ok(Some(refusal)) => {
                    return Err(invalid(&format!(
                        "{owner}: {} does not match {} at {refusal}",
                        s.implementation, s.signature
                    )));
                }
                Err(error) => return Err(query(engine, error)),
            }
        }
    }
    Ok(())
}

/// Every slot's signature and implementation name an indexed declaration the
/// slotted package can see.
pub(super) fn check_targets(engine: &QueryEngine, slots: &PackageSlots) -> Result<(), ToolError> {
    let declarations = engine.declarations.frozen();
    for (package, map) in slots {
        let index = declarations.index(package.clone());
        let viewer = FileContext {
            package: package.clone(),
            modules: vec![],
        };
        for (slot, s) in map {
            for (role, target) in [
                ("signature", &s.signature),
                ("implementation", &s.implementation),
            ] {
                if index.entry(target).is_none() {
                    return Err(invalid(&format!(
                        "{}'s slot {slot} names the {role} {target}, which is not a declaration",
                        spelling(package)
                    )));
                }
                if index.access(&viewer, target).is_err() {
                    return Err(invalid(&format!(
                        "{}'s slot {slot} names the {role} {target}, which {} cannot see",
                        spelling(package),
                        spelling(package)
                    )));
                }
            }
        }
    }
    Ok(())
}
