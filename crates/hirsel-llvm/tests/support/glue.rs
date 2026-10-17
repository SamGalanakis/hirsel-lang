//! The drop-glue corpus: block layouts from the layout derivation, one emitted entry per root that
//! releases a block of it through compiled glue, and typed object graphs of those layouts
//! written out as C data. The C harness (`tests/native/glue_harness.c`) builds each graph
//! twice and releases one through the runtime's step-program walk and one through the
//! emitted entry.
//!
//! The roots follow J2's schema corpus and the `rc-trace` object shape: records, an ordinal
//! variant of every arity (`Obj`), null-pointer niches (`Chain`, `Tree`, a list of niches),
//! lists, nested static repeats, a handle, a callable, and as children the layouts glue
//! leaves to the runtime: a resource with a finalizer, an inline resource and a
//! structure-of-arrays list.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use hirsel_lir::layout::evaluate_size;
use hirsel_lir::layout_types::{DropStep, LayoutRecord, ScanStep, SizeSpec, TagForm};
use hirsel_lir::rt_program;
use hirsel_llvm::symbol_name;

use super::programs::Program;
use super::*;

/// The fixed tree's depth: 31 nodes, all in one slab.
pub const TREE_DEPTH: u32 = 4;
/// Links of the fixed chain: more than the glue's budget of nested frames.
pub const CHAIN_LINKS: u32 = 300;

fn nominal(
    owners: &mut Owners,
    name: &str,
    resource: bool,
    soa: bool,
    shape: impl FnOnce(&mut Types) -> TypeId,
) -> DefPath {
    let mut t = Types::default();
    let private_shape = shape(&mut t);
    owners.nominals.insert(
        path(name),
        OwnerRepresentation::Defined(RepresentationRecord {
            definition: path(name),
            representation_revision: hash(name),
            generics: Vec::new(),
            types: t.store,
            private_shape,
            layout_promises: hirsel_types::Layout {
                soa,
                ..hirsel_types::Layout::default()
            },
            resource,
            finalizer: resource.then(|| (path(&format!("{name}_drop")), hirsel_tir::BodyId(0))),
            unsafe_ffi_package: false,
        }),
    );
    path(name)
}

/// One corpus root: a block layout, and the emitted entry that releases a block of it when
/// glue covers its payload.
pub struct Root {
    pub name: &'static str,
    pub payload: LayoutId,
    pub entry: Option<String>,
}

pub struct Corpus {
    pub program: Program,
    pub roots: Vec<Root>,
}

impl Root {
    /// The C wrapper `call_<entry>` of the ABI v1 signature calling this root's entry
    /// (`hirsel_llvm::c_call`), when glue covers the root.
    pub fn c_call(&self, program: &Program) -> Option<String> {
        let entry = self.entry.as_ref()?;
        Some(program.c_call(&format!("drop_{}", self.name), &format!("call_{entry}")))
    }
}

impl Corpus {
    pub fn root(&self, name: &str) -> &Root {
        self.roots.iter().find(|r| r.name == name).unwrap()
    }
}

/// Whether compiled glue releases a block of payload `layout` itself, restated from the
/// rule (no finalizer, inline resource or column reachable without crossing a block).
fn glued(table: &LayoutTable, layout: LayoutId) -> bool {
    fn plain(table: &LayoutTable, steps: &[DropStep]) -> bool {
        steps.iter().all(|step| match step {
            DropStep::Finalize | DropStep::ColumnsReverse(_) => false,
            DropStep::ActiveVariant { variants, .. } => variants.iter().all(|v| plain(table, &v.1)),
            DropStep::Child { layout, .. }
            | DropStep::ElementsReverse {
                element: layout, ..
            }
            | DropStep::RepeatReverse {
                element: layout, ..
            } => glued(table, *layout),
            _ => true,
        })
    }
    let record = &table.layouts[layout.0 as usize];
    record.finalizer.is_none() && plain(table, &record.drop)
}

pub fn corpus(target: &TargetSpec) -> Corpus {
    use PassingMode::{Borrow, Take};
    use Primitive::{F64, I64, U32, U64};
    let mut schema = Schema::new();
    let owners = &mut schema.owners;
    owners.nominals.insert(
        path("Socket"),
        OwnerRepresentation::Handle {
            revision: hash("Socket"),
            resource: true,
        },
    );
    let opaque_leaf = nominal(owners, "OpaqueLeaf", true, false, |t| t.p(U64));
    let socket = path("Socket");
    let res = nominal(owners, "Res", true, false, |t| {
        let (u, f) = (t.p(U64), t.p(F64));
        let (a, s, b) = (t.boxed(u), t.named(&socket, Vec::new()), t.boxed(f));
        t.record(&[a, s, b])
    });
    let particle = nominal(owners, "Particle", false, true, |t| {
        let (id, name, f, u) = (t.p(U32), t.p(Primitive::String), t.p(F64), t.p(U64));
        let (bf, bu) = (t.boxed(f), t.boxed(u));
        let two = t.inline(bu, 2);
        t.record(&[id, name, f, bf, two])
    });
    let chain = path("Chain");
    nominal(owners, "Chain", false, false, |t| {
        let (i, next) = (t.p(I64), t.named(&chain, Vec::new()));
        let next = t.boxed(next);
        t.variant(&[&[], &[i, next]])
    });
    let tree = path("Tree");
    nominal(owners, "Tree", false, false, |t| {
        let node = t.named(&tree, Vec::new());
        let child = t.boxed(node);
        t.variant(&[&[], &[child, child]])
    });
    let rose = path("Rose");
    nominal(owners, "Rose", false, false, |t| {
        let (u, r) = (t.p(U64), t.named(&rose, Vec::new()));
        let kids = t.list(r);
        t.record(&[u, kids])
    });
    // The rc-trace object shape: a block of 0 to 3 counted fields, or one resource.
    let obj = path("Obj");
    nominal(owners, "Obj", false, false, |t| {
        let (o, r) = (t.named(&obj, Vec::new()), t.named(&res, Vec::new()));
        let (o, r) = (t.boxed(o), t.boxed(r));
        t.variant(&[&[], &[o], &[o, o], &[o, o, o], &[r]])
    });
    let t = &mut schema.types;
    let opaque_leaf = t.named(&opaque_leaf, Vec::new());
    schema.named.push(("opaque_leaf", opaque_leaf));
    let (u64_, i64_) = (t.p(U64), t.p(I64));
    let leaf = t.boxed(u64_);
    let r = t.named(&res, Vec::new());
    let box_res = t.boxed(r);
    let three = t.inline(leaf, 3);
    let opt = t.variant(&[&[], &[leaf, box_res], &[three]]);
    let string = t.p(Primitive::String);
    let rec = t.record(&[leaf, string, box_res, three, opt]);
    let fun = t.function(&[(Borrow, i64_)], i64_, hirsel_types::Multiplicity::Many, 0);
    let inner = t.record(&[r, leaf]);
    let box_inner = t.boxed(inner);
    let wide = t.inline(leaf, 6);
    let wide = t.inline(wide, 5);
    let niche = t.variant(&[&[leaf, leaf], &[]]);
    let (chain, tree, rose, obj, particle, socket) = (
        t.named(&chain, Vec::new()),
        t.named(&tree, Vec::new()),
        t.named(&rose, Vec::new()),
        t.named(&obj, Vec::new()),
        t.named(&particle, Vec::new()),
        t.named(&socket, Vec::new()),
    );
    let particles = t.list(particle);
    let held = t.record(&[socket, leaf, fun]);
    let outer = t.record(&[box_inner, particles, leaf]);
    let roots: Vec<(&'static str, TypeId)> = vec![
        ("leaf", leaf),
        ("rec", t.boxed(rec)),
        ("recs", t.list(rec)),
        ("wide", t.list(wide)),
        ("chain", t.boxed(chain)),
        ("tree", t.boxed(tree)),
        ("niches", t.list(niche)),
        ("rose", t.boxed(rose)),
        ("obj", t.boxed(obj)),
        ("held", t.boxed(held)),
        ("outer", t.boxed(outer)),
        ("res", box_res),
    ];
    for &(name, ty) in &roots {
        schema.name(name, ty);
        schema.signature(name, &[(Take, ty), (Borrow, i64_)], i64_);
    }
    let table = schema.build(target);
    let manifest = manifest();
    let mut units = Vec::new();
    let mut out = Vec::new();
    for &(name, _) in &roots {
        let payload = programs::payload(&table, name);
        let entry = glued(&table.table, payload).then(|| {
            // drop_<name>(x, _) = 0 after releasing the taken block x.
            let unit_name = format!("drop_{name}");
            let mut u = Unit::new(&table, &unit_name, name);
            let x = u.param(table.layout(name));
            u.builder.unit.values[x.index()].holding = Holding::Owned;
            u.param(table.scalar(ScalarKind::I64));
            u.release(ENTRY, x, payload);
            let zero = u.int(ENTRY, ScalarKind::I64, 0);
            u.builder.ret(ENTRY, Some(zero));
            units.push(seal(&table, &manifest, u.finish()));
            symbol_name(&path(&unit_name), Some(&instance(&unit_name)))
        });
        out.push(Root {
            name,
            payload,
            entry,
        });
    }
    let finalizers = table
        .table
        .layouts
        .iter()
        .enumerate()
        .filter_map(|(i, r)| {
            r.finalizer
                .as_ref()
                .map(|_| (LayoutId(i as u32), instance("resource-probe")))
        })
        .collect();
    Corpus {
        program: Program {
            table,
            manifest,
            units,
            statics: Vec::new(),
            finalizers,
        },
        roots: out,
    }
}

/// The actual benchmark's linked program contains just its recursive tree, so its
/// descriptor registry has no corpus-only C finalizers or unrelated entries.
pub fn binarytrees_corpus(target: &TargetSpec) -> Corpus {
    let mut schema = Schema::new();
    let tree_path = path("Tree");
    nominal(&mut schema.owners, "Tree", false, false, |t| {
        let tree = t.named(&tree_path, Vec::new());
        let child = t.boxed(tree);
        t.variant(&[&[], &[child, child]])
    });
    let tree = schema.types.named(&tree_path, Vec::new());
    let tree = schema.types.boxed(tree);
    let i64_ = schema.types.p(Primitive::I64);
    schema.name("tree", tree);
    schema.signature(
        "tree",
        &[(PassingMode::Take, tree), (PassingMode::Borrow, i64_)],
        i64_,
    );
    let table = schema.build(target);
    let manifest = manifest();
    let payload = programs::payload(&table, "tree");
    let mut unit = Unit::new(&table, "drop_tree", "tree");
    let root = unit.param(table.layout("tree"));
    unit.builder.unit.values[root.index()].holding = Holding::Owned;
    unit.param(table.scalar(ScalarKind::I64));
    unit.release(ENTRY, root, payload);
    let zero = unit.int(ENTRY, ScalarKind::I64, 0);
    unit.builder.ret(ENTRY, Some(zero));
    let units = vec![seal(&table, &manifest, unit.finish())];
    Corpus {
        program: Program {
            table,
            manifest,
            units,
            statics: Vec::new(),
            finalizers: BTreeMap::new(),
        },
        roots: vec![Root {
            name: "tree",
            payload,
            entry: Some(symbol_name(
                &path("drop_tree"),
                Some(&instance("drop_tree")),
            )),
        }],
    }
}

/// How a graph picks variants and lengths.
#[derive(Clone, Copy, PartialEq)]
pub enum Shape {
    /// Seeded choices, with shared and externally held blocks, down to a block depth.
    Random { seed: u64, depth: u32 },
    /// The variant with the most fields at every level down to a block depth: a full tree,
    /// or a chain of that many links.
    Full { depth: u32 },
}

/// One block of a graph: its payload bytes with the block references left to `relocs`.
pub struct BlockSpec {
    /// The payload layout, or `None` for an untyped leaf block with a logging finalizer (a
    /// slice's owner, a callable's environment).
    pub layout: Option<LayoutId>,
    pub align: u32,
    pub bytes: Vec<u8>,
    /// Payload offset of a block reference, and the block it names.
    pub relocs: Vec<(u64, usize)>,
    /// Credits the harness holds besides the graph's own references.
    pub held: u32,
    complete: bool,
}

/// A typed object graph; block 0 is the root.
pub struct Graph {
    pub blocks: Vec<BlockSpec>,
}

struct Generator<'t> {
    table: &'t LayoutTable,
    shape: Shape,
    rng: u64,
    handles: u64,
    blocks: Vec<BlockSpec>,
}

impl Generator<'_> {
    fn next(&mut self) -> u64 {
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 7;
        self.rng ^= self.rng << 17;
        self.rng
    }

    fn limit(&self) -> u32 {
        match self.shape {
            Shape::Random { depth, .. } | Shape::Full { depth } => depth,
        }
    }

    fn record(&self, layout: LayoutId) -> LayoutRecord {
        self.table.layouts[layout.0 as usize].clone()
    }

    fn size(&self, layout: LayoutId) -> u64 {
        match self.table.layouts[layout.0 as usize].size {
            SizeSpec::Fixed(size) => size,
            SizeSpec::Dynamic(_) => panic!("a dynamic extent inline"),
        }
    }

    fn write(&mut self, block: usize, at: u64, bytes: &[u8]) {
        self.blocks[block].bytes[at as usize..at as usize + bytes.len()].copy_from_slice(bytes);
    }

    fn leaf(&mut self) -> usize {
        let bytes = self.next().to_le_bytes().to_vec();
        self.blocks.push(BlockSpec {
            layout: None,
            align: 8,
            bytes,
            relocs: Vec::new(),
            held: 0,
            complete: true,
        });
        self.blocks.len() - 1
    }

    /// Writes a value of `layout` at `at` in `block`, `depth` blocks below the root.
    fn value(&mut self, layout: LayoutId, block: usize, at: u64, depth: u32) {
        let record = self.record(layout);
        if let Some(tag) = &record.tag {
            let by_fields = |most: bool| {
                let key = |v: &&hirsel_lir::layout_types::VariantLayout| v.fields.len();
                if most {
                    tag.variants.iter().max_by_key(key)
                } else {
                    tag.variants.iter().min_by_key(key)
                }
            };
            let variant = if depth >= self.limit() {
                by_fields(false)
            } else if matches!(self.shape, Shape::Full { .. }) {
                by_fields(true)
            } else {
                let pick = self.next() as usize % tag.variants.len();
                tag.variants.get(pick)
            }
            .expect("a variant")
            .clone();
            // A niche's word is its dataful variant's block reference, written below.
            if tag.form == TagForm::Ordinal {
                self.write(block, at + tag.offset, &variant.ordinal.to_le_bytes());
            }
            for field in &variant.fields {
                self.value(field.layout, block, at + field.offset, depth);
            }
            return;
        }
        match record.kind {
            LayoutKind::Block { payload } | LayoutKind::Box { payload } => {
                let child = self.block(payload, depth + 1);
                self.blocks[block].relocs.push((at, child));
            }
            // The owner or environment is whatever block the value was made over.
            LayoutKind::Slice { .. } | LayoutKind::Callable { .. } => {
                let offset = match record.drop[..] {
                    [DropStep::Release { offset }] | [DropStep::Environment { offset }] => offset,
                    _ => panic!("an unexpected holder program {:?}", record.drop),
                };
                let leaf = self.leaf();
                self.blocks[block].relocs.push((at + offset, leaf));
            }
            LayoutKind::Handle => {
                self.handles += 1;
                let id = 1000 + self.handles;
                self.write(block, at, &id.to_le_bytes());
            }
            LayoutKind::Inline { element, count } => {
                let stride = self.size(layout) / count.max(1);
                for index in 0..count {
                    self.value(element, block, at + index * stride, depth);
                }
            }
            LayoutKind::Aggregate | LayoutKind::Resource => {
                for field in &record.fields {
                    self.value(field.layout, block, at + field.offset, depth);
                }
            }
            LayoutKind::Unit => {}
            LayoutKind::Scalar(_) => {
                let bytes = self.next().to_le_bytes();
                let size = self.size(layout) as usize;
                self.write(block, at, &bytes[..size.min(8)]);
            }
            ref other => panic!("no generator for an inline {other:?}"),
        }
    }

    /// A block of payload `layout`, `depth` blocks below the root: a new one, or sometimes
    /// a finished one of the same layout, which then has two holders.
    fn block(&mut self, layout: LayoutId, depth: u32) -> usize {
        let record = self.record(layout);
        let random = matches!(self.shape, Shape::Random { .. });
        if random && record.finalizer.is_none() && self.next() % 6 == 0 {
            let shared = self
                .blocks
                .iter()
                .position(|b| b.complete && b.layout == Some(layout));
            if let Some(shared) = shared {
                return shared;
            }
        }
        let index = self.blocks.len();
        self.blocks.push(BlockSpec {
            layout: Some(layout),
            align: record.align,
            bytes: Vec::new(),
            relocs: Vec::new(),
            held: 0,
            complete: false,
        });
        let count = if depth >= self.limit() {
            0
        } else {
            self.next() % 4
        };
        match (&record.size, &record.kind) {
            (SizeSpec::Fixed(size), _) => {
                self.blocks[index].bytes = vec![0; *size as usize];
                self.value(layout, index, 0, depth);
            }
            (SizeSpec::Dynamic(_), LayoutKind::Sequence { element }) => {
                let mut head = [0u8; 64];
                head[..8].copy_from_slice(&count.to_le_bytes());
                let extent = evaluate_size(&record.size, &head).expect("an extent");
                self.blocks[index].bytes = vec![0; extent.max(8) as usize];
                self.write(index, 0, &count.to_le_bytes());
                let placed = record.scan.iter().find_map(|step| match step {
                    ScanStep::Sequence { offset, stride, .. } => Some((*offset, *stride)),
                    _ => None,
                });
                if let Some((offset, stride)) = placed {
                    for i in 0..count {
                        self.value(*element, index, offset + i * stride, depth);
                    }
                }
            }
            (SizeSpec::Dynamic(_), LayoutKind::SoaColumns { .. }) => {
                let Some(DropStep::ColumnsReverse(columns)) = record.drop.first() else {
                    panic!("a structure-of-arrays payload without columns to drop")
                };
                let quantum = columns.capacity_quantum;
                let capacity = count.next_multiple_of(quantum) + quantum;
                let mut head = [0u8; 64];
                let words = [
                    (columns.count_offset, count),
                    (columns.capacity_offset, capacity),
                ];
                for (offset, word) in words {
                    head[offset as usize..offset as usize + 8].copy_from_slice(&word.to_le_bytes());
                }
                let extent = evaluate_size(&record.size, &head).expect("an extent");
                self.blocks[index].bytes = vec![0; extent as usize];
                for (offset, word) in words {
                    self.write(index, offset, &word.to_le_bytes());
                }
                for i in 0..count {
                    for column in &columns.columns {
                        let at = columns.data_offset + capacity * column.prefix + i * column.stride;
                        self.value(column.layout, index, at, depth);
                    }
                }
            }
            (_, other) => panic!("no generator for a dynamic {other:?}"),
        }
        if random && index != 0 && self.next() % 10 == 0 {
            self.blocks[index].held = 1;
        }
        self.blocks[index].complete = true;
        index
    }
}

pub fn graph(table: &LayoutTable, root: LayoutId, shape: Shape) -> Graph {
    let rng = match shape {
        Shape::Random { seed, .. } => seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1,
        Shape::Full { .. } => 1,
    };
    let mut generator = Generator {
        table,
        shape,
        rng,
        handles: 0,
        blocks: Vec::new(),
    };
    generator.block(root, 0);
    Graph {
        blocks: generator.blocks,
    }
}

/// One harness scenario: a graph released through `entry`.
pub struct Scenario {
    pub name: String,
    pub entry: String,
    pub graph: Graph,
}

/// The scenarios of a corpus: per glued root, seeded graphs; the full tree; the long chain.
pub fn scenarios(corpus: &Corpus, seeds: u64) -> Vec<Scenario> {
    let table = &corpus.program.table.table;
    let mut out = Vec::new();
    let mut add = |root: &Root, name: String, shape: Shape| {
        if let Some(entry) = &root.entry {
            out.push(Scenario {
                name,
                entry: entry.clone(),
                graph: graph(table, root.payload, shape),
            });
        }
    };
    add(
        corpus.root("tree"),
        "tree full".into(),
        Shape::Full { depth: TREE_DEPTH },
    );
    add(
        corpus.root("chain"),
        "chain full".into(),
        Shape::Full { depth: CHAIN_LINKS },
    );
    for root in &corpus.roots {
        for seed in 1..=seeds {
            let depth = 2 + (seed % 4) as u32;
            let name = format!("{} {seed}", root.name);
            add(root, name, Shape::Random { seed, depth });
        }
    }
    out
}

/// The generated C header: a descriptor per layout (as `hirsel_lir::rt_program` lowers it,
/// every finalizer the harness's logger), each scenario's blocks, and the chain's node shape
/// for the stack probe.
pub fn cases_header(corpus: &Corpus, scenarios: &[Scenario]) -> String {
    let table = &corpus.program.table.table;
    let mut used: Vec<LayoutId> = scenarios
        .iter()
        .flat_map(|s| s.graph.blocks.iter().filter_map(|b| b.layout))
        .collect();
    used.push(corpus.program.table.layout("opaque_leaf"));
    used.sort();
    used.dedup();
    let descriptors = rt_program::descriptors(table, &used).expect("every layout lowers");
    let mut c = String::new();
    for d in &descriptors {
        let digest = hirsel_lir::snapshot::digest(
            "runtime-layout-descriptor",
            table.layout(d.layout).unwrap(),
        );
        let hex: String = digest.0.iter().map(|b| format!("{b:02x}")).collect();
        let _ = writeln!(
            c,
            "extern const hirsel_rt_v2_layout L{} __asm__(\"hirsel.layout.{hex}\");",
            d.layout.0
        );
    }
    let mut drop_symbols = std::collections::BTreeSet::new();
    for (id, instance) in &corpus.program.finalizers {
        let finalizer = table.layout(*id).unwrap().finalizer.as_ref().unwrap();
        drop_symbols.insert(symbol_name(&finalizer.definition, Some(instance)));
    }
    for symbol in drop_symbols {
        let _ = writeln!(
            c,
            "uint8_t {symbol}(ctx_t *ctx, void *value) {{ return fin_log(ctx, value, 7); }}"
        );
    }
    for root in &corpus.roots {
        if let Some(call) = root.c_call(&corpus.program) {
            c.push_str(&call);
        }
    }
    for (i, scenario) in scenarios.iter().enumerate() {
        for (j, block) in scenario.graph.blocks.iter().enumerate() {
            if !block.bytes.is_empty() {
                let bytes: Vec<String> = block.bytes.iter().map(u8::to_string).collect();
                let _ = writeln!(
                    c,
                    "static const uint8_t B{i}_{j}[] = {{{}}};",
                    bytes.join(",")
                );
            }
            if !block.relocs.is_empty() {
                let relocs: Vec<String> = block
                    .relocs
                    .iter()
                    .map(|(offset, target)| format!("{{{offset}, {target}}}"))
                    .collect();
                let _ = writeln!(
                    c,
                    "static const struct reloc R{i}_{j}[] = {{{}}};",
                    relocs.join(",")
                );
            }
        }
        let _ = writeln!(c, "static const struct spec G{i}[] = {{");
        for (j, block) in scenario.graph.blocks.iter().enumerate() {
            let layout = block.layout.map_or(
                format!("&L{}", corpus.program.table.layout("opaque_leaf").0),
                |l| format!("&L{}", l.0),
            );
            let bytes = if block.bytes.is_empty() {
                "NULL".into()
            } else {
                format!("B{i}_{j}")
            };
            let relocs = if block.relocs.is_empty() {
                "NULL".into()
            } else {
                format!("R{i}_{j}")
            };
            let _ = writeln!(
                c,
                "  {{{layout}, {}, {}, {}, {bytes}, {relocs}, {}}},",
                block.bytes.len(),
                block.align,
                block.held,
                block.relocs.len()
            );
        }
        c.push_str("};\n");
    }
    c.push_str("static const struct scenario scenarios[] = {\n");
    for (i, scenario) in scenarios.iter().enumerate() {
        let _ = writeln!(
            c,
            "  {{\"{}\", call_{}, G{i}, {}}},",
            scenario.name,
            scenario.entry,
            scenario.graph.blocks.len()
        );
    }
    c.push_str("};\n");
    // One Cons node of the chain: its payload, and where its link lies.
    let chain = corpus.root("chain");
    let node = graph(table, chain.payload, Shape::Full { depth: 1 });
    let cons = &node.blocks[0];
    let _ = writeln!(
        c,
        "#define CHAIN_LAYOUT (&L{})\n#define CHAIN_SIZE {}\n#define CHAIN_ALIGN {}\n\
         #define CHAIN_NEXT {}\n#define CHAIN_ENTRY call_{}",
        chain.payload.0,
        cons.bytes.len(),
        cons.align,
        cons.relocs[0].0,
        chain.entry.as_ref().expect("glue covers the chain")
    );
    c
}
