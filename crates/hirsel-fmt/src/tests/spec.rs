//! Hand-built ASTs for the 19 executable file items of the spec's overview and closure examples.
//! Schematic parse { .. } and three top-level let sketches are excluded by #25.
use super::*;
impl Fixture {
    fn call(&mut self, name: &str, args: Vec<ExprId>) -> ExprId {
        let callee = self.value(name);
        self.e(ExprKind::Call {
            callee,
            arguments: args
                .into_iter()
                .map(|value| Argument { label: None, value })
                .collect(),
        })
    }
    fn invoke(&mut self, name: &str, args: &[&str]) -> ExprId {
        let args = args.iter().map(|s| self.value(s)).collect();
        self.call(name, args)
    }
    fn question(&mut self, value: ExprId, error: Option<&str>) -> ExprId {
        let map_error = error.map(|s| self.path(s));
        self.e(ExprKind::Try { value, map_error })
    }
    fn unary(&mut self, op: UnaryOp, s: &str) -> ExprId {
        let value = self.value(s);
        self.e(ExprKind::Unary { op, value })
    }
    fn bin(&mut self, op: BinaryOp, left: ExprId, right: ExprId) -> ExprId {
        self.e(ExprKind::Binary { op, left, right })
    }
    fn binding(
        &mut self,
        name: &str,
        value: ExprId,
        mutable: bool,
        ty: Option<TypeExprId>,
    ) -> StmtId {
        let s = self.s(name);
        let pattern = self.p(PatternKind::Binding(s));
        self.st(StmtKind::Let {
            mutable,
            pattern,
            ty,
            value,
        })
    }
    fn app(&mut self, name: &str, args: Vec<TypeExprId>) -> TypeExprId {
        let path = self.path(name);
        self.t(TypeExprKind::Named {
            path,
            arguments: args.into_iter().map(TypeArgument::Type).collect(),
        })
    }
    fn parameter(&mut self, name: &str, ty: TypeExprId, passing: PassingMode) -> Parameter {
        Parameter {
            label: None,
            name: Some(self.s(name)),
            passing,
            ty,
        }
    }
    fn params(&mut self, params: &[(&str, &str)]) -> Vec<Parameter> {
        params
            .iter()
            .map(|(n, t)| {
                let t = self.named(t);
                self.parameter(n, t, PassingMode::Borrow)
            })
            .collect()
    }
    fn sig(&mut self, params: &[(&str, &str)], result: Option<TypeExprId>) -> FunctionSyntax {
        FunctionSyntax {
            parameters: self.params(params),
            result,
            ..Default::default()
        }
    }
    fn function(
        &mut self,
        name: &str,
        signature: FunctionSyntax,
        body: Option<ExprId>,
        public: bool,
    ) {
        self.item(
            name,
            ItemKind::Function {
                signature,
                body,
                abi: None,
            },
        );
        if public {
            self.file.tree.items.last_mut().unwrap().visibility = Visibility::Public;
        }
    }
    fn generic(&mut self, name: &str, bounds: Vec<Bound>) -> GenericParam {
        GenericParam {
            name: self.s(name),
            kind: GenericKind::Type(bounds),
        }
    }
    fn record(&mut self, name: &str, flavor: TypeFlavor, fields: &[(&str, &str)]) {
        let fields = fields
            .iter()
            .map(|(n, t)| {
                let ty = self.named(t);
                FieldSyntax {
                    growable: false,

                    name: self.s(n),
                    ty,
                    origin: self.origin(),
                }
            })
            .collect();
        self.item(
            name,
            ItemKind::Type {
                flavor,
                generics: vec![],
                bounds: None,
                authority: None,
                body: TypeBody::Record(fields),
                drop: None,
            },
        );
    }
}
/// The spec's overview and closure examples, each fenced `hirsel` block
/// followed by a blank line.
fn pre_text() -> String {
    let mut result = String::new();
    for part in [
        include_str!("../../../../docs/spec/index.md"),
        include_str!("../../../../docs/spec/closures.md"),
    ] {
        let mut rest = part;
        while let Some((_, after)) = rest.split_once("\n```hirsel\n") {
            let (block, next) = after.split_once("\n```").unwrap();
            result.push_str(block);
            result.push_str("\n\n\n");
            rest = next;
        }
    }
    result
}
fn source(name: &str) -> String {
    let text = pre_text();
    let needle = if [
        "Item",
        "Order",
        "AtomicFile",
        "Percent",
        "Header",
        "Tree",
        "Log",
    ]
    .contains(&name)
    {
        format!("type {name}")
    } else {
        format!("fn {name}(")
    };
    // Generic functions have brackets between name and parameters.
    let pos = text
        .find(&needle)
        .or_else(|| text.find(&format!("fn {name}[")))
        .unwrap();
    let mut start = text[..pos].rfind('\n').map_or(0, |i| i + 1);
    if name == "saxpy" {
        start = text[..start - 1].rfind('\n').map_or(0, |i| i + 1);
    }
    let tail = &text[start..];
    let mut end = tail.len();
    let mut offset = 0;
    for (i, line) in tail.split_inclusive('\n').enumerate() {
        if i > usize::from(name == "saxpy")
            && (line.trim().is_empty()
                || [
                    "pub ",
                    "flat ",
                    "resource ",
                    "distinct ",
                    "type ",
                    "@",
                    "let ",
                    "//",
                ]
                .iter()
                .any(|prefix| line.starts_with(prefix)))
        {
            end = offset;
            break;
        }
        offset += line.len();
    }
    format!("{}\n", tail[..end].trim_end())
}
fn build(name: &str) -> Fixture {
    let mut f = Fixture::new(&source(name));
    match name {
        "main" => {
            let unit = f.t(TypeExprKind::Tuple(vec![]));
            let err = f.named("CliError");
            let result = f.app("Result", vec![unit, err]);
            let signature = f.sig(&[("rt", "Io"), ("args", "Args")], Some(result));
            let args = f.value("args");
            let zero = f.int("0");
            let root = f.call("arg.path", vec![args, zero]);
            let root = f.question(root, Some("Usage"));
            let root = f.binding("root", root, false, None);
            let dir = f.invoke("io.dir", &["rt", "root"]);
            let dir = f.question(dir, Some("Files"));
            let dir = f.binding("dir", dir, false, None);
            let n = f.invoke("normalize_tree", &["dir"]);
            let n = f.question(n, Some("Files"));
            let n = f.binding("n", n, false, None);
            let stdout = f.invoke("io.stdout", &["rt"]);
            let string = f.invoke("u64.to_string", &["n"]);
            let print = f.call("out.print", vec![stdout, string]);
            let print = f.question(print, Some("Output"));
            let print = f.st(StmtKind::Expr(print));
            let unit = f.e(ExprKind::Tuple(vec![]));
            let ok = f.call("Ok", vec![unit]);
            let body = f.block(vec![root, dir, n, print], Some(ok));
            f.function(name, signature, Some(body), true);
        }
        "normalize_tree" => {
            let u = f.named("u64");
            let err = f.named("FileError");
            let result = f.app("Result", vec![u, err]);
            let signature = f.sig(&[("dir", "Dir")], Some(result));
            let zero = f.int("0");
            let changed = f.binding("changed", zero, true, Some(u));
            let walk = f.invoke("file.walk", &["dir"]);
            let walk = f.question(walk, None);
            let walk = f.binding("walk", walk, false, None);
            let step = f.value("step");
            let path = f.question(step, None);
            let path = f.binding("path", path, false, None);
            let old = f.invoke("file.read_bytes", &["dir", "path"]);
            let old = f.question(old, None);
            let old = f.binding("old", old, false, None);
            let ov = f.value("old");
            let crlf = f.e(ExprKind::Literal(Literal::Bytes("b\"\\r\\n\"".into())));
            let lf = f.e(ExprKind::Literal(Literal::Bytes("b\"\\n\"".into())));
            let new = f.call("bytes.replace", vec![ov, crlf, lf]);
            let new = f.binding("new", new, false, None);
            let nv = f.value("new");
            let ov = f.value("old");
            let condition = f.bin(BinaryOp::Ne, nv, ov);
            let dir = f.value("dir");
            let pv = f.value("path");
            let taken = f.unary(UnaryOp::Take, "new");
            let write = f.call("file.write_bytes", vec![dir, pv, taken]);
            let write = f.question(write, None);
            let write = f.st(StmtKind::Expr(write));
            let place = f.value("changed");
            let one = f.int("1");
            let add = f.st(StmtKind::Assign {
                place,
                op: AssignOp(Some(BinaryOp::Add)),
                value: one,
            });
            let then_branch = f.block(vec![write, add], None);
            let if_ = f.e(ExprKind::If {
                condition,
                then_branch,
                else_branch: None,
            });
            let body = f.block(vec![path, old, new], Some(if_));
            let step = f.s("step");
            let pattern = f.p(PatternKind::Binding(step));
            let src = f.unary(UnaryOp::Take, "walk");
            let for_ = f.st(StmtKind::For {
                pattern,
                source: src,
                body,
            });
            let ok = f.invoke("Ok", &["changed"]);
            let body = f.block(vec![changed, walk, for_], Some(ok));
            f.function(name, signature, Some(body), true);
        }
        "test_normalize" => {
            let signature = f.sig(&[("t", "Test")], None);
            let string = f.e(ExprKind::Literal(Literal::String("\"a.txt\"".into())));
            let path = f.call("path.of", vec![string]);
            let bytes = f.e(ExprKind::Literal(Literal::Bytes("b\"x\\r\\ny\"".into())));
            let tuple = f.e(ExprKind::Tuple(vec![path, bytes]));
            let seq = f.e(ExprKind::Sequence(vec![tuple]));
            let t = f.value("t");
            let dir = f.call("test.dir", vec![t, seq]);
            let dir = f.binding("dir", dir, false, None);
            let normalized = f.invoke("normalize_tree", &["dir"]);
            let one = f.int("1");
            let ok = f.call("Ok", vec![one]);
            let eq = f.bin(BinaryOp::Eq, normalized, ok);
            let t = f.value("t");
            let check = f.call("test.check", vec![t, eq]);
            let body = f.block(vec![dir], Some(check));
            f.function(name, signature, Some(body), true);
        }
        "reserve" => {
            let r = f.named("Reservation");
            let e = f.named("StockError");
            let e = f.app("TxFailure", vec![e]);
            let result = f.app("Result", vec![r, e]);
            let signature = f.sig(&[("db", "Db"), ("sku", "Sku"), ("n", "u32")], Some(result));
            let get = f.invoke("stock.get", &["tx", "sku"]);
            let get = f.question(get, None);
            let have = f.binding("have", get, false, None);
            let hv = f.value("have");
            let n = f.value("n");
            let condition = f.bin(BinaryOp::Lt, hv, n);
            let out = f.invoke("OutOfStock", &["sku"]);
            let err = f.call("Error", vec![out]);
            let ret = f.e(ExprKind::Return(Some(err)));
            let then_branch = f.block(vec![], Some(ret));
            let if_ = f.e(ExprKind::If {
                condition,
                then_branch,
                else_branch: None,
            });
            let if_ = f.st(StmtKind::Expr(if_));
            let hv = f.value("have");
            let n = f.value("n");
            let sub = f.bin(BinaryOp::Sub, hv, n);
            let tx = f.value("tx");
            let sku = f.value("sku");
            let put = f.call("stock.put", vec![tx, sku, sub]);
            let put = f.question(put, None);
            let put = f.st(StmtKind::Expr(put));
            let sku = f.value("sku");
            let n = f.value("n");
            let callee = f.value("Reservation");
            let sl = f.s("sku");
            let count = f.s("count");
            let reservation = f.e(ExprKind::Call {
                callee,
                arguments: vec![
                    Argument {
                        label: Some(sl),
                        value: sku,
                    },
                    Argument {
                        label: Some(count),
                        value: n,
                    },
                ],
            });
            let tx = f.value("tx");
            let inserted = f.call("reservations.insert", vec![tx, reservation]);
            let inserted = f.question(inserted, None);
            let ok = f.call("Ok", vec![inserted]);
            let body = f.block(vec![have, if_, put], Some(ok));
            let closure_sig = f.sig(&[("tx", "Tx")], None);
            let closure = f.e(ExprKind::Closure {
                signature: closure_sig,
                body,
            });
            let db = f.value("db");
            let transaction = f.call("store.transact", vec![db, closure]);
            let body = f.block(vec![], Some(transaction));
            f.function(name, signature, Some(body), true);
        }
        "sizes" => {
            let u = f.named("u64");
            let tuple = f.t(TypeExprKind::Tuple(vec![u, u]));
            let err = f.named("FileError");
            let result = f.app("Result", vec![tuple, err]);
            let signature = f.sig(
                &[
                    ("tasks", "Tasks"),
                    ("dir", "Dir"),
                    ("a", "Path"),
                    ("b", "Path"),
                ],
                Some(result),
            );
            let mut tasks = Vec::new();
            for (binding, path) in [("ha", "a"), ("hb", "b")] {
                let size = f.invoke("file.size", &["dir", path]);
                let body = f.block(vec![], Some(size));
                let closure = f.e(ExprKind::Closure {
                    signature: FunctionSyntax::default(),
                    body,
                });
                let s = f.value("s");
                let task = f.call("task.async", vec![s, closure]);
                tasks.push(f.binding(binding, task, false, None));
            }
            let mut awaited = Vec::new();
            for h in ["ha", "hb"] {
                let s = f.value("s");
                let h = f.unary(UnaryOp::Take, h);
                let await_ = f.call("task.await", vec![s, h]);
                awaited.push(f.question(await_, None));
            }
            let tuple = f.e(ExprKind::Tuple(awaited));
            let ok = f.call("Ok", vec![tuple]);
            let body = f.block(tasks, Some(ok));
            let cs = f.sig(&[("s", "Scope")], None);
            let closure = f.e(ExprKind::Closure {
                signature: cs,
                body,
            });
            let tasks = f.value("tasks");
            let scope = f.call("task.scope", vec![tasks, closure]);
            let body = f.block(vec![], Some(scope));
            f.function(name, signature, Some(body), true);
        }
        "get" | "at" | "append" | "map" | "lazy_map" => {
            let t = f.named(if name == "get" { "K" } else { "T" });
            let v = f.named(if name == "get" { "V" } else { "U" });
            let mut signature = FunctionSyntax::default();
            match name {
                "get" => {
                    let map = f.app("Map", vec![t, v]);
                    let option = f.app("Option", vec![v]);
                    let m = f.path("m");
                    let result = f.t(TypeExprKind::Qualified {
                        base: option,
                        modifiers: vec![],
                        brand: None,
                        relation: Some(RelationSyntax {
                            places: vec![RelationPlace::Path(m)],
                            origin: f.origin(),
                        }),
                    });
                    signature.generics =
                        vec![f.generic("K", vec![Bound::Key]), f.generic("V", vec![])];
                    signature.parameters = vec![
                        f.parameter("m", map, PassingMode::Borrow),
                        f.parameter("k", t, PassingMode::Borrow),
                    ];
                    signature.result = Some(result);
                }
                "at" => {
                    let array = f.app("Array", vec![t]);
                    let u = f.named("u64");
                    let xs = f.path("xs");
                    let result = f.t(TypeExprKind::Qualified {
                        base: t,
                        modifiers: vec![],
                        brand: None,
                        relation: Some(RelationSyntax {
                            places: vec![RelationPlace::Path(xs)],
                            origin: f.origin(),
                        }),
                    });
                    signature.generics = vec![f.generic("T", vec![])];
                    signature.parameters = vec![
                        f.parameter("xs", array, PassingMode::Borrow),
                        f.parameter("i", u, PassingMode::Borrow),
                    ];
                    signature.result = Some(result);
                    let i = f.value("i");
                    let len = f.invoke("array.len", &["xs"]);
                    let expression = f.bin(BinaryOp::Lt, i, len);
                    signature.contracts.push(ContractSyntax {
                        kind: ContractKind::Requires,
                        expression,
                    });
                }
                "append" => {
                    let list = f.app("List", vec![t]);
                    signature.generics = vec![f.generic("T", vec![])];
                    signature.parameters = vec![
                        f.parameter("xs", list, PassingMode::Mut),
                        f.parameter("x", t, PassingMode::Take),
                    ];
                    let len = f.invoke("list.len", &["xs"]);
                    let old = f.invoke("list.len", &["xs"]);
                    let old = f.e(ExprKind::Before(old));
                    let one = f.int("1");
                    let plus = f.bin(BinaryOp::Add, old, one);
                    let expression = f.bin(BinaryOp::Eq, len, plus);
                    signature.contracts.push(ContractSyntax {
                        kind: ContractKind::Ensures,
                        expression,
                    });
                }
                _ => {
                    let container = if name == "map" { "List" } else { "Iter" };
                    let input = f.app(container, vec![t]);
                    let result = f.app(container, vec![v]);
                    let fun = f.t(TypeExprKind::Function(FunctionSyntax {
                        parameters: vec![Parameter {
                            label: None,
                            name: None,
                            passing: PassingMode::Borrow,
                            ty: t,
                        }],
                        result: Some(v),
                        ..Default::default()
                    }));
                    signature.generics = vec![f.generic("T", vec![]), f.generic("U", vec![])];
                    let mode = if name == "map" {
                        PassingMode::Borrow
                    } else {
                        PassingMode::Take
                    };
                    signature.parameters = vec![
                        f.parameter(if name == "map" { "xs" } else { "it" }, input, mode),
                        f.parameter("f", fun, mode),
                    ];
                    signature.result = Some(result);
                    if name == "lazy_map" {
                        signature.uses = Some(vec![f.path("it"), f.path("f")]);
                    }
                }
            }
            f.function(name, signature, None, true);
        }
        "saxpy" => {
            let float = f.named("f32");
            let n = f.named("n");
            let inline = f.app("Inline", vec![float, n]);
            let signature = FunctionSyntax {
                generics: vec![GenericParam {
                    name: f.s("n"),
                    kind: GenericKind::Size,
                }],
                parameters: vec![
                    f.parameter("a", float, PassingMode::Borrow),
                    f.parameter("x", inline, PassingMode::Borrow),
                    f.parameter("y", inline, PassingMode::Mut),
                ],
                ..Default::default()
            };
            let i = f.value("i");
            let y = f.value("y");
            let place = f.e(ExprKind::Index { base: y, index: i });
            let x = f.value("x");
            let x = f.e(ExprKind::Index { base: x, index: i });
            let a = f.value("a");
            let mul = f.bin(BinaryOp::Mul, a, x);
            let sum = f.bin(BinaryOp::Add, mul, place);
            let assign = f.st(StmtKind::Assign {
                place,
                op: AssignOp(None),
                value: sum,
            });
            let body = f.block(vec![assign], None);
            let zero = f.int("0");
            let n = f.value("n");
            let source = f.bin(BinaryOp::Range, zero, n);
            let i = f.s("i");
            let pattern = f.p(PatternKind::Binding(i));
            let for_ = f.st(StmtKind::For {
                pattern,
                source,
                body,
            });
            let body = f.block(vec![for_], None);
            f.function(name, signature, Some(body), true);
            let attr = Attribute {
                name: f.s("noalloc"),
                arguments: vec![],
                origin: f.origin(),
            };
            f.file.tree.items.last_mut().unwrap().attributes.push(attr);
        }
        "Item" => f.record(name, TypeFlavor::Flat, &[("sku", "String"), ("qty", "u32")]),
        "Order" => {
            let u = f.named("u64");
            let string = f.named("String");
            let item = f.named("Item");
            let list = f.app("List", vec![item]);
            let fields = [("id", u), ("customer", string), ("items", list)]
                .into_iter()
                .map(|(n, ty)| FieldSyntax {
                    growable: false,

                    name: f.s(n),
                    ty,
                    origin: f.origin(),
                })
                .collect();
            f.item(
                name,
                ItemKind::Type {
                    flavor: TypeFlavor::Flat,
                    generics: vec![],
                    bounds: None,
                    authority: None,
                    body: TypeBody::Record(fields),
                    drop: None,
                },
            );
        }
        "AtomicFile" => {
            f.record(
                name,
                TypeFlavor::Resource,
                &[("dir", "Dir"), ("tmp", "Path"), ("dst", "Path")],
            );
            let remove = f.invoke("file.remove", &["f.dir", "f.tmp"]);
            let pattern = f.p(PatternKind::Wildcard);
            let let_ = f.st(StmtKind::Let {
                mutable: false,
                pattern,
                ty: None,
                value: remove,
            });
            let body = f.block(vec![let_], None);
            let drop = DropSyntax {
                noalloc: false,

                binding: f.s("f"),
                body,
            };
            if let ItemKind::Type { drop: d, .. } = &mut f.file.tree.items[0].kind {
                *d = Some(drop);
            }
        }
        "save" => {
            let unit = f.t(TypeExprKind::Tuple(vec![]));
            let err = f.named("FileError");
            let result = f.app("Result", vec![unit, err]);
            let signature = f.sig(
                &[("dir", "Dir"), ("p", "Path"), ("data", "Bytes")],
                Some(result),
            );
            let created = f.invoke("file.create_atomic", &["dir", "p"]);
            let created = f.question(created, None);
            let binding = f.binding("f", created, true, None);
            let dir = f.value("dir");
            let mut_ = f.unary(UnaryOp::Mut, "f");
            let data = f.value("data");
            let write = f.call("file.write_all", vec![dir, mut_, data]);
            let write = f.question(write, None);
            let write = f.st(StmtKind::Expr(write));
            let dir = f.value("dir");
            let take = f.unary(UnaryOp::Take, "f");
            let commit = f.call("file.commit", vec![dir, take]);
            let body = f.block(vec![binding, write], Some(commit));
            f.function(name, signature, Some(body), true);
        }
        "Percent" => {
            let u = f.named("u8");
            let field = FieldSyntax {
                growable: false,

                name: f.s("v"),
                ty: u,
                origin: f.origin(),
            };
            let v = f.value("v");
            let hundred = f.int("100");
            let fact = f.bin(BinaryOp::Le, v, hundred);
            f.item(
                name,
                ItemKind::Type {
                    flavor: TypeFlavor::Distinct,
                    generics: vec![],
                    bounds: None,
                    authority: None,
                    body: TypeBody::Distinct {
                        field,
                        fact: Some(fact),
                    },
                    drop: None,
                },
            );
        }
        "Header" => {
            f.record(
                name,
                TypeFlavor::Ordinary,
                &[("magic", "u32"), ("len", "u16"), ("flags", "u16")],
            );
            let c = f.s("c");
            let attr = Attribute {
                name: f.s("layout"),
                arguments: vec![AttributeArgument::Name(c)],
                origin: f.origin(),
            };
            f.file.tree.items[0].attributes.push(attr);
        }
        "Tree" => {
            let tree = f.named("Tree");
            let boxed = f.t(TypeExprKind::Box(tree));
            let u = f.named("u64");
            let variants = vec![
                VariantSyntax {
                    name: f.s("Leaf"),
                    fields: vec![],
                },
                VariantSyntax {
                    name: f.s("Node"),
                    fields: vec![
                        (Some(f.s("left")), boxed),
                        (Some(f.s("key")), u),
                        (Some(f.s("right")), boxed),
                    ],
                },
            ];
            f.item(
                name,
                ItemKind::Type {
                    flavor: TypeFlavor::Ordinary,
                    generics: vec![],
                    bounds: None,
                    authority: None,
                    body: TypeBody::Variants(variants),
                    drop: None,
                },
            );
        }
        "Log" => {
            let dir = f.named("Dir");
            let file = f.named("File");
            let path = f.path("dir");
            let branded = f.t(TypeExprKind::Qualified {
                base: file,
                modifiers: vec![],
                brand: Some(BrandSyntax {
                    sibling: true,
                    path,
                }),
                relation: None,
            });
            let fields = vec![
                FieldSyntax {
                    growable: false,

                    name: f.s("dir"),
                    ty: dir,
                    origin: f.origin(),
                },
                FieldSyntax {
                    growable: false,

                    name: f.s("file"),
                    ty: branded,
                    origin: f.origin(),
                },
            ];
            f.item(
                name,
                ItemKind::Type {
                    flavor: TypeFlavor::Ordinary,
                    generics: vec![],
                    bounds: None,
                    authority: None,
                    body: TypeBody::Record(fields),
                    drop: None,
                },
            );
        }
        _ => panic!("missing hand-built fixture {name}"),
    }
    f
}
const NAMES: [&str; 19] = [
    "main",
    "normalize_tree",
    "test_normalize",
    "reserve",
    "sizes",
    "get",
    "saxpy",
    "Item",
    "Order",
    "AtomicFile",
    "save",
    "at",
    "append",
    "Percent",
    "Header",
    "Tree",
    "Log",
    "map",
    "lazy_map",
];
#[test]
fn every_executable_spec_overview_item_has_a_hand_built_formatter_golden() {
    let mut actual = String::new();
    let mut original = String::new();
    let mut unchanged = 0;
    for name in NAMES {
        original.push_str(&format!("// example: {name}\n{}\n", source(name)));
        let f = build(name);
        let result = format(&f.file, &f.sources);
        assert!(!result.unchanged, "fallback for {name}: {}", result.source);
        if result.source == source(name) {
            unchanged += 1;
        }
        actual.push_str(&format!("// example: {name}\n{}\n", result.source));
    }
    assert_eq!(
        original,
        include_str!("../../tests/fixtures/spec-overview-original.hir")
    );
    assert_eq!(
        actual,
        include_str!("../../tests/fixtures/spec-overview.hir")
    );
    eprintln!(
        "spec overview examples: {} formatted, {unchanged} exact, {} layout differences",
        NAMES.len(),
        NAMES.len() - unchanged
    );
}
