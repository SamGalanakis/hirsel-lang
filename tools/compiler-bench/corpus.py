"""The compiler benchmark corpus: which programs are compiled, in which modes,
and the generators that write the synthetic and stress programs (Hirsel and
their Rust pairs) into a work directory. Python stdlib only.

Every program is a `Program`: a name, its language, a path the compiler takes
(a source, a manifest or a Cargo project), its source lines and the groups it
belongs to. A pair is two programs of the same name in different languages.
Generators are deterministic: the same arguments write the same bytes.
"""
from dataclasses import dataclass, field
import json
from pathlib import Path
import shutil

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[1]
RUST = HERE / 'rust'

# The six example programs the synthetic family copies, in copy order, and each
# one's Rust port under rust/units/.
UNITS = ('todo-api', 'kvstore', 'logq', 'csvjson', 'mdtoc', 'bench-lite')

# A fixed sample of accepted conformance programs, one per rule family (the
# fifth `-run.hir` of each family's sorted listing on 2026-10-19). A program
# renamed or removed makes the suite report it missing; replace it then, and
# start a new baseline for its cells.
CONFORMANCE = (
    'CAP/CAP-1-arg-snapshot-run.hir',
    'CLO/CLO-1-nested-local-call-result-run.hir',
    'CONC/CONC-2-taken-block-sender-capture-run.hir',
    'CT/CT-2-size-local-run.hir',
    'DROP/DROP-2-destructure-opaque-run.hir',
    'FACT/FACT-3-match-tag-retested-each-pass-run.hir',
    'GEN/GEN-1-capture-data-nested-run.hir',
    'LANG/LANG-11-block-value-borrow-run.hir',
    'MEM/MEM-11-thaw-after-root-dropped-run.hir',
    'MODE/MODE-1-contract6-owned-run.hir',
    'OWN/OWN-3-element-sibling-field-block-argument-run.hir',
    'PROV/PROV-1-copied-prefix-if-run.hir',
    'SEM/SEM-1-bigint-go-bitwise-run.hir',
    'SER/SER-2-duplicate-map-run.hir',
)

# Synthetic program sizes in source lines. Each size above the first is
# measured only when the one before it completed under the cap.
SCALE = (3000, 10000, 30000)

# Stress families: generator name and the parameters each is run at.
STRESS = {
    'nest': (8, 16, 32),          # Wrap[Wrap[...Leaf]] chains, one function per level
    'instances': (32, 128),       # one generic type instantiated at N distinct types
    'wide': (2000,),              # one module of N tiny functions
    'long': (2000,),              # one function of N statements
    'arms': (200,),               # an N-variant enum and an N-arm match
    'nested-match': (16, 32),     # matches nested N deep
    'table': (2000,),             # an N-row literal table
    'inline-constant': (1024, 2048, 4096),  # scalar constant facts, not List iteration
    'record-flow': (4, 8, 16),    # match arms per mut helper over a fixed wide record
    'derive': (64,),              # N record types with derived JSON encoders
    'closures': (200,),           # N closures, each called once
}


@dataclass(frozen=True)
class Program:
    name: str
    language: str        # 'hirsel' or 'rust'
    path: Path           # source, manifest or (Rust) Cargo project directory
    lines: int
    groups: tuple = field(default=())
    # A Hirsel program that runs only as `check` (no entry a native build accepts).
    check_only: bool = False


def lines_of(path: Path) -> int:
    if path.is_dir():
        return sum(lines_of(p) for p in sorted(path.rglob('*.rs')))
    if path.name == 'hirsel.json':
        manifest = json.loads(path.read_text())
        return sum(lines_of(path.parent / s['path']) for s in manifest['sources'])
    return path.read_text().count('\n')


def write(path: Path, text: str):
    path.parent.mkdir(parents=True, exist_ok=True)
    if not path.exists() or path.read_text() != text:
        path.write_text(text)


# ---- Rust projects ----

def cargo_project(directory: Path, name: str, main: str, modules: dict[str, str]) -> Path:
    """A Cargo project at `directory` with the pinned dependency set and lockfile."""
    write(directory / 'Cargo.toml', (RUST / 'Cargo.toml.in').read_text().replace('@NAME@', name))
    lock = (RUST / 'Cargo.lock.in').read_text().replace('@NAME@', name)
    write(directory / 'Cargo.lock', lock)
    write(directory / 'src/main.rs', main)
    for module, text in modules.items():
        write(directory / f'src/{module}.rs', text)
    return directory


def rust_unit(unit: str) -> str:
    return (RUST / 'units' / (unit.replace('-', '_') + '.rs')).read_text()


def rust_example(work: Path, unit: str) -> Path:
    """One example's Rust port as its own binary, calling the unit's main."""
    module = unit.replace('-', '_')
    main = (f'mod {module};\n\nfn main() {{\n'
            f'    let args: Vec<String> = std::env::args().skip(1).collect();\n'
            f'    if let Err(error) = {module}::main(&args) {{\n'
            f'        eprintln!("error: {{error:?}}");\n        std::process::exit(1);\n    }}\n}}\n')
    return cargo_project(work / 'rust' / unit, module.replace('_', '-'), main, {module: rust_unit(unit)})


HELLO_HIRSEL = ('pub type Failure = Grant(GrantError) | Output(OutputError)\n\n'
                'pub fn main(rt: Io) -> Result[(), Failure] {\n'
                '  let output = io.stdout(rt) ? Grant\n'
                '  out.print(output, "hello, world\\n") ? Output\n'
                '  Ok(())\n}\n')
HELLO_RUST = 'fn main() {\n    println!("hello, world");\n}\n'


def std_manifest(sources, name):
    return {'schema': 1, 'package': {'name': name, 'version': '1'}, 'sources': sources,
            'dependencies': [{'manifest': str(ROOT / 'stdlib/std@1/hirsel.json'),
                              'package': {'name': 'std', 'version': '1'}, 'expose': []}],
            'entry': {'module': [], 'name': 'main'}}


def hello(work: Path):
    directory = work / 'hirsel' / 'hello'
    write(directory / 'main.hir', HELLO_HIRSEL)
    write(directory / 'hirsel.json', json.dumps(std_manifest([{'path': 'main.hir', 'module': []}], 'hello')))
    rust = cargo_project(work / 'rust' / 'hello', 'hello', HELLO_RUST, {})
    return directory / 'hirsel.json', rust


# ---- synthetic programs ----

def synthetic(work: Path, target: int):
    """A Hirsel package of about `target` lines: copies of the six examples, each
    its own module, called from a root main by argument count; and its Rust pair
    with the same modules in the same order."""
    name = f'syn{target // 1000}k'
    out = work / 'hirsel' / name
    sources, calls, rust_calls, modules, total, i = [], [], [], {}, 0, 0
    while total < target:
        unit = UNITS[i % len(UNITS)]
        module = f"{unit.replace('-', '_')}_{i}"
        text = (ROOT / 'examples' / unit / 'main.hir').read_text()
        write(out / f'{module}.hir', text)
        sources.append({'path': f'{module}.hir', 'module': [module]})
        calls.append(f'  if k == {i + 1} {{\n    let _ = {module}.main(rt, args)\n  }}\n')
        rust_calls.append(f'    if k == {i + 1} {{\n        let _ = {module}::main(&args);\n    }}\n')
        modules[module] = rust_unit(unit)
        total += text.count('\n')
        i += 1
    root = ('pub type RootError = Grant(GrantError) | Output(OutputError)\n\n'
            'pub fn main(rt: Io, args: Args) -> Result[(), RootError] {\n'
            '  let output = io.stdout(rt) ? Grant\n'
            '  out.print(output, "start\\n") ? Output\n'
            '  let k = arg.len(args)\n' + ''.join(calls) + '  Ok(())\n}\n')
    write(out / 'main.hir', root)
    sources.insert(0, {'path': 'main.hir', 'module': []})
    write(out / 'hirsel.json', json.dumps(std_manifest(sources, 'synthetic')))
    main = (''.join(f'mod {m};\n' for m in modules) + '\nfn main() {\n'
            '    let args: Vec<String> = std::env::args().skip(1).collect();\n'
            '    println!("start");\n    let k = args.len();\n' + ''.join(rust_calls) + '}\n')
    rust = cargo_project(work / 'rust' / name, 'synthetic', main, modules)
    return name, out / 'hirsel.json', rust


# ---- stress families: (Hirsel source, Rust source) for one parameter ----

def wrapped(k, wrap='Wrap[{}]', leaf='Leaf'):
    s = leaf
    for _ in range(k):
        s = wrap.format(s)
    return s


def stress_nest(d):
    h = ['type Leaf {\n  x: u64,\n}\n', 'type Wrap[T] {\n  value: T,\n}\n']
    r = ['#[allow(dead_code)]\nstruct Leaf {\n    x: u64,\n}\n',
         '#[allow(dead_code)]\nstruct Wrap<T> {\n    value: T,\n}\n']
    for k in range(1, d + 1):
        h.append(f'fn f{k}(v: {wrapped(k - 1)}) -> {wrapped(k)} {{\n  Wrap(value: v)\n}}\n')
        r.append(f'fn f{k}(v: {wrapped(k - 1, "Wrap<{}>")}) -> {wrapped(k, "Wrap<{}>")} {{\n'
                 '    Wrap { value: v }\n}\n')
    h.append('pub fn main() {\n  let v0 = Leaf(x: 1)\n'
             + ''.join(f'  let v{k} = f{k}(v{k - 1})\n' for k in range(1, d + 1)) + '}\n')
    r.append('fn main() {\n    let v0 = Leaf { x: 1 };\n'
             + ''.join(f'    let v{k} = f{k}(v{k - 1});\n' for k in range(1, d + 1))
             + f'    std::hint::black_box(&v{d});\n}}\n')
    return '\n'.join(h), '\n'.join(r)


def stress_instances(n):
    h = ['type Cell[T] {\n  value: T,\n  count: u64,\n}\n',
         'fn cell[T](take v: T) -> Cell[T] {\n  Cell(value: v, count: 1)\n}\n']
    r = ['#[allow(dead_code)]\nstruct Cell<T> {\n    value: T,\n    count: u64,\n}\n',
         'fn cell<T>(v: T) -> Cell<T> {\n    Cell { value: v, count: 1 }\n}\n']
    for k in range(n):
        h.append(f'type P{k} {{\n  x: u64,\n}}\n')
        r.append(f'#[allow(dead_code)]\nstruct P{k} {{\n    x: u64,\n}}\n')
    h.append('pub fn main() {\n' + ''.join(f'  let c{k} = cell(P{k}(x: {k}))\n' for k in range(n))
             + '  let mut total = 0u64\n' + ''.join(f'  total += c{k}.count\n' for k in range(n)) + '}\n')
    r.append('fn main() {\n' + ''.join(f'    let c{k} = cell(P{k} {{ x: {k} }});\n' for k in range(n))
             + '    let mut total = 0u64;\n' + ''.join(f'    total += c{k}.count;\n' for k in range(n))
             + '    std::hint::black_box(total);\n}\n')
    return '\n'.join(h), '\n'.join(r)


def stress_wide(n):
    h = [f'fn f{k}(x: u64) -> u64 {{\n  x + {k}\n}}\n' for k in range(n)]
    r = [f'fn f{k}(x: u64) -> u64 {{\n    x.wrapping_add({k})\n}}\n' for k in range(n)]
    h.append('pub fn main() {\n  let mut x = 0u64\n' + ''.join(f'  x = f{k}(x)\n' for k in range(0, n, 50)) + '}\n')
    r.append('fn main() {\n    let mut x = 0u64;\n' + ''.join(f'    x = f{k}(x);\n' for k in range(0, n, 50))
             + '    std::hint::black_box(x);\n}\n')
    return '\n'.join(h), '\n'.join(r)


def stress_long(n):
    h = 'pub fn main() {\n  let mut x = 1u64\n' + ''.join(
        f'  x = x % 1000003u64 + {k}u64\n' for k in range(n)) + '}\n'
    r = 'fn main() {\n    let mut x = 1u64;\n' + ''.join(
        f'    x = x % 1000003 + {k};\n' for k in range(n)) + '    std::hint::black_box(x);\n}\n'
    return h, r


def stress_arms(n):
    variants = ' | '.join(f'V{k}' for k in range(n))
    h = (f'type Op = {variants}\n\n'
         'fn code(op: Op) -> u64 {\n  match op {\n'
         + ''.join(f'    V{k}() => {k * 7 % 101}u64,\n' for k in range(n)) + '  }\n}\n\n'
         'pub fn main() {\n  let _ = code(V3())\n}\n')
    r = ('#[allow(dead_code)]\nenum Op {\n' + ''.join(f'    V{k},\n' for k in range(n)) + '}\n\n'
         'fn code(op: Op) -> u64 {\n    match op {\n'
         + ''.join(f'        Op::V{k} => {k * 7 % 101},\n' for k in range(n)) + '    }\n}\n\n'
         'fn main() {\n    std::hint::black_box(code(Op::V3));\n}\n')
    return h, r


def stress_nested_match(d):
    def h_level(k, indent):
        pad = '  ' * indent
        if k == d:
            return f'{pad}{k}u64\n'
        return (f'{pad}match o{k} {{\n{pad}  Some(v{k}) => {{\n{pad}    let o{k + 1}: Option[u64] = Some(v{k} + 1)\n'
                + h_level(k + 1, indent + 2) + f'{pad}  }},\n{pad}  None() => 0u64,\n{pad}}}\n')

    def r_level(k, indent):
        pad = '    ' * indent
        if k == d:
            return f'{pad}{k}\n'
        return (f'{pad}match o{k} {{\n{pad}    Some(v{k}) => {{\n{pad}        let o{k + 1}: Option<u64> = Some(v{k} + 1);\n'
                + r_level(k + 1, indent + 2) + f'{pad}    }}\n{pad}    None => 0,\n{pad}}}\n')
    h = ('fn deep(o0: Option[u64]) -> u64 {\n' + h_level(0, 1) + '}\n\n'
         'pub fn main() {\n  let _ = deep(Some(1u64))\n}\n')
    r = ('fn deep(o0: Option<u64>) -> u64 {\n' + r_level(0, 1) + '}\n\n'
         'fn main() {\n    std::hint::black_box(deep(Some(1)));\n}\n')
    return h, r


def stress_table(n):
    h = ('const TABLE: List[(String, u64)] = [\n'
         + ''.join(f'  ("key{k}", {k * 31 % 9973}u64),\n' for k in range(n)) + ']\n\n'
         'pub fn main() {\n  let mut total = 0u64\n  for (_, v) in TABLE {\n    total += v\n  }\n}\n')
    r = (f'const TABLE: [(&str, u64); {n}] = [\n'
         + ''.join(f'    ("key{k}", {k * 31 % 9973}),\n' for k in range(n)) + '];\n\n'
         'fn main() {\n    let mut total = 0u64;\n    for (_, v) in TABLE.iter() {\n        total += v;\n    }\n'
         '    std::hint::black_box(total);\n}\n')
    return h, r


def stress_inline_constant(n):
    h = (f'const TABLE: Inline[u64, {n}] = [\n'
         + ''.join(f'  {k * 31 % 9973}u64,\n' for k in range(n)) + ']\n\n'
         'pub fn main() {\n  let _ = TABLE[7]\n}\n')
    r = (f'const TABLE: [u64; {n}] = [\n'
         + ''.join(f'    {k * 31 % 9973},\n' for k in range(n)) + '];\n\n'
         'fn main() {\n    std::hint::black_box(TABLE[7]);\n}\n')
    return h, r


def stress_record_flow(n):
    # Keep record width and helper count fixed: only the size of each function
    # changes. Each helper has the protobuf decoder's match/conditional writes
    # through a mutable whole-message parameter, including non-scalar fields.
    width = 165
    h = ['type Message {\n' + ''.join(f'  f{k}: List[u64],\n' for k in range(width)) + '}\n']
    r = ['struct Message {\n' + ''.join(f'    f{k}: Vec<u64>,\n' for k in range(width)) + '}\n']
    h.append('fn read(wire: u64) -> Result[List[u64], ()] {\n'
             '  if wire == 0 { Ok([7u64]) } else { Error(()) }\n}\n')
    r.append('fn read(wire: u64) -> Result<Vec<u64>, ()> {\n'
             '    if wire == 0 { Ok(vec![7]) } else { Err(()) }\n}\n')
    for helper in range(4):
        h.append(f'fn update{helper}(mut m: Message, field: u64, wire: u64) -> Result[bool, ()] {{\n'
                 '  let found = match field {\n'
                 + ''.join(f'    {k} => {{ m.f{k} = read(wire)?; true }},\n'
                           for k in range(n)) + '    _ => false,\n  }\n  Ok(found)\n}\n')
        r.append(f'fn update{helper}(m: &mut Message, field: u64, wire: u64) -> Result<bool, ()> {{\n'
                 '    let found = match field {\n'
                 + ''.join(f'        {k} => {{ m.f{k} = read(wire)?; true }},\n'
                           for k in range(n)) + '        _ => false,\n    };\n    Ok(found)\n}\n')
    # A check-only command lowers compile-time callees. Force V to inspect the
    # helpers as well as modes; a run-time-only main would measure modes alone.
    h.append('fn exercise() -> Result[bool, ()] {\n  let mut m = Message(\n'
             + ''.join(f'    f{k}: [],\n' for k in range(width)) + '  )\n'
             + ''.join(f'  let _ = update{helper}(mut m, 0, 0)?\n' for helper in range(4))
             + '  Ok(true)\n}\nconst CHECKED: Result[bool, ()] = exercise()\n'
             + 'pub fn main() {\n  let _ = CHECKED\n}\n')
    r.append('fn main() {\n    let mut m = Message {\n'
             + ''.join(f'        f{k}: Vec::new(),\n' for k in range(width)) + '    };\n'
             + ''.join(f'    std::hint::black_box(update{helper}(&mut m, 0, 0));\n' for helper in range(4))
             + '    std::hint::black_box(m);\n}\n')
    return '\n'.join(h), '\n'.join(r)


def stress_derive(n):
    h, r = [], ['use serde::Serialize;\n']
    for k in range(n):
        h.append(f'type R{k} {{\n  id: u64,\n  name: String,\n  ok: bool,\n}}\n\n'
                 f'fn r{k}_value(v: R{k}) -> json.Value uses {{}} = comptime derive.json_value(shape(R{k}))\n')
        r.append(f'#[derive(Serialize)]\nstruct R{k} {{\n    id: u64,\n    name: String,\n    ok: bool,\n}}\n')
    h.append('pub fn main() {\n' + ''.join(
        f'  let _ = r{k}_value(R{k}(id: {k}, name: "n", ok: true))\n' for k in range(n)) + '}\n')
    r.append('fn main() {\n' + ''.join(
        f'    std::hint::black_box(serde_json::to_value(R{k} {{ id: {k}, name: "n".into(), ok: true }}).ok());\n'
        for k in range(n)) + '}\n')
    return '\n'.join(h), '\n'.join(r)


def stress_closures(n):
    h = ('pub fn main() {\n  let base = 3u64\n  let mut total = 0u64\n'
         + ''.join(f'  let c{k} = fn(x: u64) -> u64 {{\n    x * base + {k}u64\n  }}\n  total += c{k}({k}u64)\n'
                   for k in range(n)) + '}\n')
    r = ('fn main() {\n    let base = 3u64;\n    let mut total = 0u64;\n'
         + ''.join(f'    let c{k} = |x: u64| -> u64 {{ x * base + {k} }};\n    total += c{k}({k});\n'
                   for k in range(n)) + '    std::hint::black_box(total);\n}\n')
    return h, r


GENERATORS = {
    'nest': stress_nest, 'instances': stress_instances, 'wide': stress_wide, 'long': stress_long,
    'arms': stress_arms, 'nested-match': stress_nested_match, 'table': stress_table,
    'inline-constant': stress_inline_constant, 'record-flow': stress_record_flow,
    'derive': stress_derive, 'closures': stress_closures,
}
# Stress programs that use std (and so need a manifest sidecar).
STD_STRESS = {'derive'}


def stress(work: Path, family: str, n: int):
    name = f'{family}-{n}'
    hirsel, rust = GENERATORS[family](n)
    # Every generator ends with main; a native entry returns Result[(), E].
    assert hirsel.count('pub fn main() {\n') == 1 and hirsel.endswith('}\n'), family
    hirsel = hirsel.replace('pub fn main() {\n', 'pub fn main() -> Result[(), ()] {\n')[:-2] + '  Ok(())\n}\n'
    source = work / 'hirsel' / 'stress' / f'{name}.hir'
    write(source, hirsel)
    if family in STD_STRESS:
        write(source.with_name(source.name + '.json'), json.dumps(
            std_manifest([{'path': source.name, 'module': []}], name)))
    project = cargo_project(work / 'rust' / 'stress' / name, name, rust, {})
    return name, source, project


# ---- the edit loop ----

# The program the edit cells change, and the module whose file they edit.
EDIT_PROGRAM = 'syn10k'
EDIT_MODULE = 'todo_api_0'
# Probe definitions appended to that module before the edit; each edit replaces
# one marked text. The body edit changes a reply string of a function the
# program calls; the signature edits change a private and a public function's
# parameter and result types (no callers, so only the edited definition and
# whatever depends on its module's declarations is affected).
PROBES = {
    'hirsel': ('\nfn probe_private(x: u64) -> u64 {\n  x\n}\n\npub fn probe_public(x: u64) -> u64 {\n  x\n}\n',
               {'edit-body': ('error_json("not found")', 'error_json("not found!")'),
                'edit-private-sig': ('fn probe_private(x: u64) -> u64', 'fn probe_private(x: u32) -> u32'),
                'edit-public-sig': ('pub fn probe_public(x: u64) -> u64', 'pub fn probe_public(x: u32) -> u32')}),
    'rust': ('\n#[allow(dead_code)]\nfn probe_private(x: u64) -> u64 {\n    x\n}\n\n'
             '#[allow(dead_code)]\npub fn probe_public(x: u64) -> u64 {\n    x\n}\n',
             {'edit-body': ('error_json("not found")', 'error_json("not found!")'),
              'edit-private-sig': ('fn probe_private(x: u64) -> u64', 'fn probe_private(x: u32) -> u32'),
              'edit-public-sig': ('pub fn probe_public(x: u64) -> u64', 'pub fn probe_public(x: u32) -> u32')}),
}


def edits(copy: Path, language: str):
    """In a copy of the edit program: the file the edits change, its text with
    the probes, and each edit's text."""
    path = copy / f'{EDIT_MODULE}.hir' if language == 'hirsel' else copy / 'src' / f'{EDIT_MODULE}.rs'
    probes, changes = PROBES[language]
    base = path.read_text() + probes
    variants = {}
    for edit, (old, new) in changes.items():
        assert base.count(old) == 1, (edit, old)
        variants[edit] = base.replace(old, new)
    return path, base, variants


# ---- the whole corpus ----

def corpus(work: Path, scale=SCALE) -> list[Program]:
    """Write every generated program under `work` and return the corpus, in run
    order: small programs first, so a failing toolchain fails fast."""
    programs = []
    add = programs.append
    hello_hirsel, hello_rust = hello(work)
    add(Program('hello', 'hirsel', hello_hirsel, lines_of(hello_hirsel), ('pairs', 'examples')))
    add(Program('hello', 'rust', hello_rust, lines_of(hello_rust), ('pairs', 'examples')))
    for unit in UNITS:
        manifest = ROOT / 'examples' / unit / 'hirsel.json'
        add(Program(unit, 'hirsel', manifest, lines_of(manifest), ('pairs', 'examples')))
        project = rust_example(work, unit)
        add(Program(unit, 'rust', project, lines_of(project), ('pairs', 'examples')))
    for name, filename in (('tasks', 'hirsel.json'), ('hello-web', 'main.hir')):
        path = ROOT / 'examples' / name / filename
        add(Program(name, 'hirsel', path, lines_of(path), ('examples',)))
        if name == 'tasks':
            project = ROOT / 'bench/apps/tasks/rust'
            app_lines = sum(lines_of(project / 'src' / f) for f in
                            ('main.rs', 'model.rs', 'store.rs', 'unicode.rs', 'view.rs')) + lines_of(project / 'build.rs')
            add(Program(name, 'rust', project, app_lines, ('examples',)))
    for relative in CONFORMANCE:
        path = ROOT / 'conformance' / relative
        add(Program('conformance/' + Path(relative).stem, 'hirsel', path, lines_of(path), ('conformance',), True))
    for path in tour_programs():
        add(Program('tour/' + path.stem, 'hirsel', path, lines_of(path), ('tour',), True))
    std = ROOT / 'stdlib/std@1/hirsel.json'
    add(Program('std@1', 'hirsel', std, lines_of(std), ('std',), True))
    for family, parameters in STRESS.items():
        for n in parameters:
            name, source, project = stress(work, family, n)
            add(Program('stress/' + name, 'hirsel', source, lines_of(source), ('stress', 'pairs')))
            add(Program('stress/' + name, 'rust', project, lines_of(project), ('stress', 'pairs')))
    for target in scale:
        name, manifest, project = synthetic(work, target)
        add(Program(name, 'hirsel', manifest, lines_of(manifest), ('scale', 'pairs')))
        add(Program(name, 'rust', project, lines_of(project), ('scale', 'pairs')))
    return programs


def tour_programs() -> list[Path]:
    """The runnable tour sources docs/tour.md links, in its order."""
    marker = '<!-- tour: '
    found = []
    for line in (ROOT / 'docs/tour.md').read_text().splitlines():
        if line.startswith(marker):
            found.append(ROOT / line[len(marker):].split(' ')[0])
    return found


def clean(work: Path):
    shutil.rmtree(work, ignore_errors=True)
