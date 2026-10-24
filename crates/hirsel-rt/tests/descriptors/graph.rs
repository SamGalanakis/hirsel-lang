//! A conservative static call graph of the runtime's source, parsed with `syn`.
//!
//! Nodes are functions: a free function is keyed by its module path and name (`tasks::scheduler`),
//! a method or associated function by its impl's type or trait and name (`Binding::get`). Edges
//! come from every path expression in a body (calls and function references alike, including
//! inside macro arguments) and every method call. A path reaches every function its segments can
//! name through the module, its `use` items and the crate root. A method call `.f()` reaches
//! every method `f` of a type the function names (in its signature, its body or its impl) or
//! gets back from a function or method it calls; receivers reached only through fields are not
//! typed. Closures belong to
//! the function that contains them. Calls through function pointers and trait objects (compiled
//! code, drop thunks, the Io adapter) are not followed.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::Path;
use syn::visit::Visit;

/// What an entry of a non-op locality must never reach.
pub const FORBIDDEN: &[&str] = &["tasks::scheduler", "Binding::get"];

/// Not followed: a stop is the executor's terminal boundary (§4.1 `stop`).
const BOUNDARIES: &[&str] = &["stop::raise"];

/// Calls that release a credit of a block the caller proves is not unique, so they never free
/// it and never reach the release walk's drops.
const NON_LAST_RELEASES: &[(&str, &str)] = &[
    // "Not unique, so the credit never frees it." (`copy::detach_sized`)
    ("copy::detach_sized", "release::release_block"),
];

#[derive(Default)]
pub struct Graph {
    /// Function key to the keys it may call.
    edges: BTreeMap<String, BTreeSet<String>>,
    /// Exported entry symbol to its function key.
    entries: BTreeMap<String, String>,
    symbols: BTreeMap<String, String>,
}

/// One function's unresolved references.
struct Body {
    key: String,
    module: Vec<String>,
    owner: Option<String>,
    paths: Vec<Vec<String>>,
    methods: Vec<String>,
    /// The type names it mentions.
    types: BTreeSet<String>,
}

#[derive(Default)]
struct Collect {
    /// The type names in each function's return type.
    returns: BTreeMap<String, BTreeSet<String>>,
    bodies: Vec<Body>,
    /// Every function key, and every method name that takes a receiver.
    keys: BTreeSet<String>,
    methods: BTreeMap<String, BTreeSet<String>>,
    /// Per module, `use` names to the paths they import, and glob-imported modules.
    uses: BTreeMap<Vec<String>, BTreeMap<String, Vec<String>>>,
    globs: BTreeMap<Vec<String>, Vec<Vec<String>>>,
    entries: BTreeMap<String, String>,
}

fn cfg_test(attrs: &[syn::Attribute]) -> bool {
    attrs.iter().any(|a| {
        a.path().is_ident("cfg")
            && a.parse_args::<syn::Meta>()
                .is_ok_and(|m| m.path().is_ident("test"))
    })
}

fn exported(attrs: &[syn::Attribute]) -> bool {
    attrs.iter().any(|a| {
        a.path().is_ident("no_mangle")
            || (a.path().is_ident("unsafe")
                && a.parse_args::<syn::Path>()
                    .is_ok_and(|p| p.is_ident("no_mangle")))
    })
}

fn type_name(ty: &syn::Type) -> Option<String> {
    match ty {
        syn::Type::Path(p) => p.path.segments.last().map(|s| s.ident.to_string()),
        _ => None,
    }
}

fn join(module: &[String], name: &str) -> String {
    module
        .iter()
        .map(String::as_str)
        .chain([name])
        .collect::<Vec<_>>()
        .join("::")
}

/// The references of one body.
#[derive(Default)]
struct Refs {
    paths: Vec<Vec<String>>,
    methods: Vec<String>,
    types: BTreeSet<String>,
    locals: BTreeSet<String>,
}

fn uppercase(s: &str) -> bool {
    s.starts_with(|c: char| c.is_ascii_uppercase())
}

impl<'a> Visit<'a> for Refs {
    fn visit_pat_ident(&mut self, p: &'a syn::PatIdent) {
        self.locals.insert(p.ident.to_string());
        syn::visit::visit_pat_ident(self, p);
    }
    fn visit_block(&mut self, block: &'a syn::Block) {
        let outer = self.locals.clone();
        syn::visit::visit_block(self, block);
        self.locals = outer;
    }
    fn visit_local(&mut self, local: &'a syn::Local) {
        // A let binding is not in scope in its initializer or its else arm.
        if let Some(init) = &local.init {
            self.visit_expr(&init.expr);
            if let Some((_, diverge)) = &init.diverge {
                self.visit_expr(diverge);
            }
        }
        self.visit_pat(&local.pat);
    }
    fn visit_expr_let(&mut self, e: &'a syn::ExprLet) {
        self.visit_expr(&e.expr);
        self.visit_pat(&e.pat);
    }
    fn visit_expr_if(&mut self, e: &'a syn::ExprIf) {
        let outer = self.locals.clone();
        self.visit_expr(&e.cond);
        self.visit_block(&e.then_branch);
        self.locals = outer;
        if let Some((_, branch)) = &e.else_branch {
            self.visit_expr(branch);
        }
    }
    fn visit_expr_while(&mut self, e: &'a syn::ExprWhile) {
        let outer = self.locals.clone();
        syn::visit::visit_expr_while(self, e);
        self.locals = outer;
    }
    fn visit_expr_for_loop(&mut self, e: &'a syn::ExprForLoop) {
        self.visit_expr(&e.expr);
        let outer = self.locals.clone();
        self.visit_pat(&e.pat);
        self.visit_block(&e.body);
        self.locals = outer;
    }
    fn visit_expr_closure(&mut self, e: &'a syn::ExprClosure) {
        let outer = self.locals.clone();
        syn::visit::visit_expr_closure(self, e);
        self.locals = outer;
    }
    fn visit_arm(&mut self, arm: &'a syn::Arm) {
        let outer = self.locals.clone();
        syn::visit::visit_arm(self, arm);
        self.locals = outer;
    }
    fn visit_path(&mut self, p: &'a syn::Path) {
        for segment in &p.segments {
            let name = segment.ident.to_string();
            if uppercase(&name) {
                self.types.insert(name);
            }
        }
        syn::visit::visit_path(self, p);
    }
    fn visit_expr_path(&mut self, e: &'a syn::ExprPath) {
        if e.path.segments.len() == 1 && self.locals.contains(&e.path.segments[0].ident.to_string())
        {
            return;
        }
        self.paths.push(
            e.path
                .segments
                .iter()
                .map(|s| s.ident.to_string())
                .collect(),
        );
        syn::visit::visit_expr_path(self, e);
    }
    fn visit_expr_method_call(&mut self, e: &'a syn::ExprMethodCall) {
        self.methods.push(e.method.to_string());
        syn::visit::visit_expr_method_call(self, e);
    }
    fn visit_macro(&mut self, m: &'a syn::Macro) {
        use syn::punctuated::Punctuated;
        if let Ok(args) =
            m.parse_body_with(Punctuated::<syn::Expr, syn::Token![,]>::parse_terminated)
        {
            for arg in &args {
                self.visit_expr(arg);
            }
        } else if let Ok(block) = m.parse_body_with(syn::Block::parse_within) {
            for stmt in &block {
                self.visit_stmt(stmt);
            }
        }
    }
    // Nested items are collected as items of their own.
    fn visit_item(&mut self, _: &'a syn::Item) {}
}

impl Collect {
    fn body(
        &mut self,
        key: String,
        module: &[String],
        owner: Option<String>,
        sig: &syn::Signature,
        block: &syn::Block,
    ) {
        let mut refs = Refs::default();
        refs.visit_signature(sig);
        refs.visit_block(block);
        refs.types.extend(owner.clone());
        let mut returns = Refs::default();
        returns.visit_return_type(&sig.output);
        self.returns.insert(key.clone(), returns.types);
        self.keys.insert(key.clone());
        self.bodies.push(Body {
            key,
            module: module.to_vec(),
            owner,
            paths: refs.paths,
            methods: refs.methods,
            types: refs.types,
        });
    }

    fn uses(&mut self, module: &[String], prefix: Vec<String>, tree: &syn::UseTree) {
        match tree {
            syn::UseTree::Path(p) => {
                let mut prefix = prefix;
                prefix.push(p.ident.to_string());
                self.uses(module, prefix, &p.tree);
            }
            syn::UseTree::Name(n) => {
                let mut full = prefix;
                let name = n.ident.to_string();
                if name != "self" {
                    full.push(name.clone());
                }
                let alias = full.last().cloned().unwrap_or(name);
                self.uses
                    .entry(module.to_vec())
                    .or_default()
                    .insert(alias, full);
            }
            syn::UseTree::Rename(r) => {
                let mut full = prefix;
                full.push(r.ident.to_string());
                self.uses
                    .entry(module.to_vec())
                    .or_default()
                    .insert(r.rename.to_string(), full);
            }
            syn::UseTree::Glob(_) => self.globs.entry(module.to_vec()).or_default().push(prefix),
            syn::UseTree::Group(g) => {
                for tree in &g.items {
                    self.uses(module, prefix.clone(), tree);
                }
            }
        }
    }

    fn items(&mut self, module: &[String], items: &[syn::Item]) {
        for item in items {
            match item {
                syn::Item::Fn(f) if !cfg_test(&f.attrs) => {
                    let name = f.sig.ident.to_string();
                    let key = join(module, &name);
                    if exported(&f.attrs) {
                        self.entries.insert(name, key.clone());
                    }
                    self.body(key, module, None, &f.sig, &f.block);
                    self.items(module, &nested(&f.block));
                }
                syn::Item::Impl(i) if !cfg_test(&i.attrs) => {
                    let Some(owner) = type_name(&i.self_ty) else {
                        continue;
                    };
                    for item in &i.items {
                        if let syn::ImplItem::Fn(f) = item {
                            if cfg_test(&f.attrs) {
                                continue;
                            }
                            let name = f.sig.ident.to_string();
                            if f.sig.receiver().is_some() {
                                self.methods
                                    .entry(name.clone())
                                    .or_default()
                                    .insert(format!("{owner}::{name}"));
                            }
                            self.body(
                                format!("{owner}::{name}"),
                                module,
                                Some(owner.clone()),
                                &f.sig,
                                &f.block,
                            );
                        }
                    }
                }
                syn::Item::Trait(t) if !cfg_test(&t.attrs) => {
                    let owner = t.ident.to_string();
                    for item in &t.items {
                        if let syn::TraitItem::Fn(f) = item {
                            let name = f.sig.ident.to_string();
                            if f.sig.receiver().is_some() {
                                self.methods
                                    .entry(name.clone())
                                    .or_default()
                                    .insert(format!("{owner}::{name}"));
                            }
                            if let Some(block) = &f.default {
                                self.body(
                                    format!("{owner}::{name}"),
                                    module,
                                    Some(owner.clone()),
                                    &f.sig,
                                    block,
                                );
                            }
                        }
                    }
                }
                syn::Item::Mod(m) if !cfg_test(&m.attrs) => {
                    if let Some((_, items)) = &m.content {
                        let mut inner = module.to_vec();
                        inner.push(m.ident.to_string());
                        self.items(&inner, items);
                    }
                }
                syn::Item::Use(u) if !cfg_test(&u.attrs) => self.uses(module, Vec::new(), &u.tree),
                _ => {}
            }
        }
    }

    /// The function keys `path`, written in `module` inside `owner`'s impl, may name.
    fn resolve(&self, module: &[String], owner: Option<&str>, path: &[String]) -> Vec<String> {
        let Some((name, prefix)) = path.split_last() else {
            return Vec::new();
        };
        if let Some(ty) = prefix
            .last()
            .filter(|s| s.starts_with(|c: char| c.is_ascii_uppercase()))
        {
            let ty = if ty == "Self" {
                owner.unwrap_or(ty)
            } else {
                ty
            };
            return vec![format!("{ty}::{name}")];
        }
        let mut bases: Vec<Vec<String>> = Vec::new();
        match prefix.first().map(String::as_str) {
            Some("crate") => bases.push(prefix[1..].to_vec()),
            Some("self") => bases.push([module, &prefix[1..]].concat()),
            Some("super") => bases.push(expand(prefix, module)),
            Some(first) => {
                bases.push([module, prefix].concat());
                bases.push(prefix.to_vec());
                if let Some(full) = self.uses.get(module).and_then(|u| u.get(first)) {
                    let full = expand(full, module);
                    bases.push([&full[..], &prefix[1..]].concat());
                }
            }
            None => {
                bases.push(module.to_vec());
                if let Some(full) = self.uses.get(module).and_then(|u| u.get(name)) {
                    let full = expand(full, module);
                    if let Some((last, base)) =
                        full.split_last().filter(|(_, base)| !base.is_empty())
                    {
                        // An imported function, or an imported type's associated function.
                        return self
                            .resolve(module, owner, &[base, &[last.clone()]].concat())
                            .into_iter()
                            .chain([join(base, last)])
                            .collect();
                    }
                }
                for glob in self.globs.get(module).into_iter().flatten() {
                    bases.push(expand(glob, module));
                }
            }
        }
        bases
            .into_iter()
            .map(|base| join(&base, name))
            .filter(|key| self.keys.contains(key))
            .collect()
    }
}

/// A `use` path relative to `module`, from the crate root.
fn expand(path: &[String], module: &[String]) -> Vec<String> {
    match path.first().map(String::as_str) {
        Some("crate") => path[1..].to_vec(),
        Some("self") => [module, &path[1..]].concat(),
        Some("super") => {
            let parents = path
                .iter()
                .take_while(|part| part.as_str() == "super")
                .count();
            [
                &module[..module.len().saturating_sub(parents)],
                &path[parents..],
            ]
            .concat()
        }
        _ => path.to_vec(),
    }
}

/// Items declared inside a function body.
fn nested(block: &syn::Block) -> Vec<syn::Item> {
    block
        .stmts
        .iter()
        .filter_map(|s| match s {
            syn::Stmt::Item(item @ syn::Item::Fn(_)) => Some(item.clone()),
            _ => None,
        })
        .collect()
}

fn module_of(root: &Path, file: &Path) -> Vec<String> {
    let relative = file.strip_prefix(root).unwrap().with_extension("");
    let mut parts: Vec<String> = relative
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect();
    if matches!(parts.last().map(String::as_str), Some("lib" | "mod")) {
        parts.pop();
    }
    parts
}

fn files(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
    let mut entries: Vec<_> = std::fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("{}: {e}", dir.display()))
        .map(|e| e.unwrap().path())
        .collect();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            files(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

impl Graph {
    /// Direct guard coverage matters: a transitive guard in one branch does
    /// not protect the entry's other branches or its earlier shared reads.
    pub fn directly_calls(&self, start: &str, target: &str) -> bool {
        self.edges
            .get(start)
            .is_some_and(|edges| edges.contains(target))
    }

    pub fn parse(root: &Path) -> Graph {
        let mut paths = Vec::new();
        files(root, &mut paths);
        let mut collect = Collect::default();
        for path in &paths {
            let text = std::fs::read_to_string(path).unwrap();
            let file = syn::parse_file(&text).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
            if cfg_test(&file.attrs) {
                continue;
            }
            collect.items(&module_of(root, path), &file.items);
        }
        let mut graph = Graph::default();
        for body in &collect.bodies {
            let mut targets = BTreeSet::new();
            for path in &body.paths {
                targets.extend(collect.resolve(&body.module, body.owner.as_deref(), path));
            }
            let mut types = body.types.clone();
            loop {
                for target in &targets {
                    types.extend(collect.returns.get(target).into_iter().flatten().cloned());
                }
                let before = targets.len();
                for method in &body.methods {
                    targets.extend(
                        collect
                            .methods
                            .get(method)
                            .into_iter()
                            .flatten()
                            .filter(|key| types.contains(key.split("::").next().unwrap()))
                            .cloned(),
                    );
                }
                if targets.len() == before {
                    break;
                }
            }
            targets.remove(&body.key);
            for (caller, callee) in NON_LAST_RELEASES {
                if body.key == *caller {
                    targets.remove(*callee);
                }
            }
            graph
                .edges
                .entry(body.key.clone())
                .or_default()
                .extend(targets);
        }
        for (symbol, key) in collect.entries {
            graph.symbols.insert(key.clone(), symbol.clone());
            graph.entries.insert(symbol, key);
        }
        graph
    }

    /// Exported `hirsel_rt_v2_*` entries and their function keys.
    pub fn entries(&self) -> impl Iterator<Item = (&str, &str)> {
        self.entries
            .iter()
            .filter(|(s, _)| s.starts_with("hirsel_rt_v2_"))
            .map(|(s, k)| (s.as_str(), k.as_str()))
    }

    pub fn entry_symbol(&self, key: &str) -> Option<&str> {
        self.symbols.get(key).map(String::as_str)
    }

    /// The shortest call path from `start` to a function `bad` holds for. An exported entry
    /// other than `start` is checked on its own and not followed.
    pub fn reaches(&self, start: &str, bad: impl Fn(&str) -> bool) -> Option<Vec<String>> {
        self.reaches_until(start, bad, |_| false)
    }

    /// A caller-specific boundary does not alter other audits: in particular,
    /// physical budget services remain traversed by the whole-Run borrow check.
    pub fn reaches_until(
        &self,
        start: &str,
        bad: impl Fn(&str) -> bool,
        boundary: impl Fn(&str) -> bool,
    ) -> Option<Vec<String>> {
        let mut parent: BTreeMap<&str, &str> = BTreeMap::new();
        let mut queue = VecDeque::from([start]);
        while let Some(key) = queue.pop_front() {
            for next in self.edges.get(key).into_iter().flatten() {
                let next = next.as_str();
                if next == start || parent.contains_key(next) {
                    continue;
                }
                parent.insert(next, key);
                if bad(next) {
                    let mut path = vec![next.to_owned()];
                    let mut at = next;
                    while let Some(&p) = parent.get(at) {
                        path.push(p.to_owned());
                        at = p;
                        if at == start {
                            break;
                        }
                    }
                    path.reverse();
                    return Some(path);
                }
                if !BOUNDARIES.contains(&next)
                    && !boundary(next)
                    && !self.symbols.contains_key(next)
                {
                    queue.push_back(next);
                }
            }
        }
        None
    }
}
