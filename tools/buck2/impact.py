"""What a change can affect: the table tools/gate computes its floor and its
suggestion from (gating.Selection). The lane chooses what runs beyond the
floor; this is its starting point (`tools/gate --suggest`).

A changed path is one of:

- documentation (`*.md`, `LICENSE`, `docs/`) that no build or test reads: a
  fast consistency check of its links and whitespace, nothing else;
- the gate's own Python tooling and the native benchmark harness (TOOL_PATHS):
  the tooling tests, bench/native/tests among them, and `sync.py --check`;
- the Lean RC model (PROOFS): its audit and vector replay on the host;
- a build-wide input (BUILD_WIDE): every test and every campaign;
- anything else: the Buck2 tests that read it, directly or through any
  dependency (the owners' reverse dependencies in graph.py's static reading of
  the root BUCK), and a crate's library source
  the campaigns in CRATE_CAMPAIGNS. Campaign data selects by case: a corpus
  case its twins (changed_programs), a std module the cases whose import
  closure includes it (Std, std_dependents), a package fixture the cases that
  reach it (package_dependents); data every case reads its whole campaign
  (campaign_data).

The generated root BUCK selects by target: the targets whose generated rule
changed, and every test that depends on them. Its header changing is
build-wide.
"""

from __future__ import annotations

from dataclasses import dataclass, field
import ast
import json
import re

CONFORMANCE = 'conformance'
AGREEMENT = 'agreement'
NATIVE_SMOKE = 'native smoke'
CAMPAIGNS = (CONFORMANCE, AGREEMENT, NATIVE_SMOKE)
ALL = frozenset(CAMPAIGNS)
FRONT = frozenset({CONFORMANCE, AGREEMENT})

# The campaigns an edit to a crate's library source can change the verdict of.
# conformance: the crate is in the pipeline hirsel-conformance checks and runs
# the corpus through (front end, formatter, interpreter, engine and driver).
# agreement: it is in the source checker, the IR checker or the interpreter
# whose normal and checked runs the agreement campaign compares.
# native smoke: it is in the backend, the runtime or what they are built on.
# Every workspace member has a row (tools/tests/gate.py).
CRATE_CAMPAIGNS = {
    # The shared contract crates every stage is built on.
    'hirsel-graph': ALL,
    # The memory guard every binary and test harness allocates through.
    'hirsel-guard': ALL,
    'hirsel-span': ALL,
    'hirsel-intern': ALL,
    'hirsel-diag': ALL,
    'hirsel-syntax': ALL,
    'hirsel-types': ALL,
    'hirsel-tir': ALL,
    'hirsel-check-protocol': ALL,
    # The front end and the IR validator.
    'hirsel-lexer': FRONT,
    'hirsel-parser': FRONT,
    'hirsel-resolve': FRONT,
    'hirsel-signature': FRONT,
    'hirsel-check': FRONT,
    'hirsel-modes': FRONT,
    'hirsel-lower': FRONT,
    'hirsel-validate': FRONT,
    # The interpreter and what it shares with the runtime.
    'hirsel-interp': FRONT,
    'hirsel-kernels': ALL,
    'hirsel-run-config': ALL,
    # The rest of the pipeline the corpus runs through.
    'hirsel-fmt': frozenset({CONFORMANCE}),
    'hirsel-proof-export': frozenset({CONFORMANCE}),
    'hirsel-codec': ALL,
    'hirsel-engine': ALL,
    'hirsel-driver': ALL,
    'hirsel-conformance': ALL,
    'hirsel-cli-args': ALL,
    # The backend, the runtime and LLVM.
    'hirsel-core-contract': frozenset({NATIVE_SMOKE}),
    'hirsel-lir': frozenset({NATIVE_SMOKE}),
    'hirsel-layout': frozenset({NATIVE_SMOKE}),
    'hirsel-rc': frozenset({NATIVE_SMOKE}),
    'hirsel-heapcheck': frozenset({NATIVE_SMOKE}),
    'hirsel-llvm': frozenset({NATIVE_SMOKE}),
    'hirsel-cranelift': frozenset({NATIVE_SMOKE}),
    'hirsel-rt': frozenset({NATIVE_SMOKE}),
    # Tool and test crates: only their own tests.
    'hirsel-cli': frozenset(),
    'hirsel-native-build': frozenset(),
    'hirsel-pipeline-tests': frozenset(),
    # Host providers no corpus program runs through: only their own tests.
    'hirsel-object-storage': frozenset(),
    'hirsel-code-store': frozenset(),
}

# The crates each campaign runs through: every crate in their normal
# dependency closure must select it (gating.campaign_table_errors). Agreement
# compares the front end and the IR validator, checked and normal
# interpretation; conformance also formats and exports proofs.
AGREEMENT_ROOTS = ('hirsel-lexer', 'hirsel-parser', 'hirsel-resolve', 'hirsel-signature', 'hirsel-check',
                   'hirsel-modes', 'hirsel-lower', 'hirsel-validate', 'hirsel-interp')
CAMPAIGN_ROOTS = {
    CONFORMANCE: (*AGREEMENT_ROOTS, 'hirsel-fmt', 'hirsel-proof-export'),
    AGREEMENT: AGREEMENT_ROOTS,
    NATIVE_SMOKE: ('hirsel-llvm', 'hirsel-cranelift', 'hirsel-rt'),
}

# A command's implementation cannot change another command's campaign.
# Library sources and unlisted paths retain their crate-wide classification.
SOURCE_CAMPAIGNS = {
    'crates/hirsel-conformance/src/main.rs': frozenset({CONFORMANCE}),
    'crates/hirsel-conformance/src/bin/agree.rs': frozenset({AGREEMENT}),
    'crates/hirsel-conformance/src/bin/native.rs': frozenset({NATIVE_SMOKE}),
}

# Count budgets apply to generated code and compiler checks.
PERF_COUNTS = 'root//:gate-perf-counts'
PERF_PATHS = ('crates/hirsel-llvm/**', 'crates/hirsel-rt/**', 'crates/hirsel-rc/**',
              'crates/hirsel-lir/**', 'stdlib/**', 'bench/native/programs/**', 'bench/micro/**',
              'tools/perf-counts', 'tools/perf-counts.tsv')


def perf_counts(paths):
    return any(matches(path, PERF_PATHS) for path in paths)


# These producers must exercise both native backends with one program selection.
NATIVE_PARITY_CRATES = frozenset({
    'hirsel-llvm', 'hirsel-cranelift', 'hirsel-lir', 'hirsel-rc', 'hirsel-rt',
})


def native_programs(paths, crates, root):
    """Changed runnable fixtures, with smoke cases when only a producer changed.

    Explicit sources select the same cases in LLVM's matrix and Cranelift parity.
    A backend implementation has no source dependency graph: without changed
    fixtures its bounded control set is the existing native smoke list.
    """
    if not (set(crates) & NATIVE_PARITY_CRATES):
        return []
    programs = sorted({path.removesuffix('.json') for path in paths
                       if path.startswith(('conformance/', 'native-tests/'))
                       and path.endswith(('.hir', '.hir.json'))
                       and (root / path.removesuffix('.json')).is_file()
                       and (path.startswith('native-tests/') or
                            any(line.startswith(('// expect: run', '// expect: fail', '// expect: blocked', '// expect: test'))
                                for line in (root / path.removesuffix('.json')).read_text().splitlines()))})
    if programs:
        return programs
    return [line for line in (root / 'native-tests/regressions/smoke.txt').read_text().splitlines()
            if line and not line.startswith('#')]


# A crate's paths that are test code or test data, not its library source:
# their edits select only the tests that read them.
CRATE_TEST_DIRS = ('tests', 'benches', 'examples', 'proptest-regressions')

# The gate's own tooling, with tests in tools/tests, and the native benchmark
# harness, with tests in bench/native/tests. None is a build or test input
# (sync.py --check proves that the generated build files do not change),
# except what the generated BUCK reads (bench/native/programs/**): that is
# selected through its owners.
TOOL_PATHS = (
    'tools/gate', 'tools/land', 'tools/capped', 'tools/main-stall-alarm', 'tools/*.sh', 'tools/*.py', 'tools/tests/*',
    *(f'tools/buck2/{name}.py' for name in (
        'action_sizes_from_log', 'agree', 'clippy', 'conformance', 'fmt', 'full_gate_counts',
        'event_logs', 'gating', 'graph', 'impact', 'merging', 'pool_usage', 'sync', 'test_costs', 'test_sizes_from_usage')),
    'tools/known-failures.tsv',
    'tools/buck2/test-sizes.json',
    'tools/buck2/test_sizes.bzl',
    'bench/native/**',
    'bench/multicore/**',
)

# Inputs of every action or of the toolchains: the Cargo workspace and lock,
# Buck2's configuration and rules, the third-party build and the bootstrap.
BUILD_WIDE = (
    'Cargo.toml', 'rust-toolchain.toml', '.buckconfig', '.cargo/**', 'third-party/**',
    'tools/buck2/**',
)
# Under tools/ but read by particular tests: selected through their owners.
TOOL_INPUTS = ('tools/buck2/test-costs/**', 'tools/native-toolchain/**')

# The Lean RC model and the TLA+ ordered-mode model: no Buck2 target reads them; their own
# checks run on the host (proof_checks), as do the protocol vectors the Lean model replays.
PROOFS = ('proofs/**',)
PROOF_VECTORS = 'native-tests/model/'
ORDERED_MODEL = ('proofs/ordered/**',)

# Documentation, and repository metadata no build reads.
DOCS = ('**.md', 'LICENSE', 'docs/**', '.gitignore')


@dataclass
class Classified:
    """A change's paths by what they can affect."""
    docs: list[str] = field(default_factory=list)
    tools: list[str] = field(default_factory=list)
    proofs: list[str] = field(default_factory=list)
    build_wide: list[str] = field(default_factory=list)
    # Paths whose Buck2 owners and their reverse dependencies are selected.
    owned: list[str] = field(default_factory=list)
    # The root BUCK changed: True (header) or the names of changed targets.
    buck: bool | set[str] = False
    crates: set[str] = field(default_factory=set)
    campaigns: set[str] = field(default_factory=set)
    unknown_crates: set[str] = field(default_factory=set)
    # Reservation changes select the resized binaries, without their rdeps.
    test_requests: set[str] = field(default_factory=set)

    @property
    def everything(self) -> bool:
        return bool(self.build_wide) or self.buck is True or bool(self.unknown_crates)

    @property
    def needs_buck2(self) -> bool:
        return self.everything or bool(self.owned) or bool(self.buck) or bool(self.test_requests)


def pattern(glob: str) -> re.Pattern:
    """A path glob: `*` stays within one directory, `**` crosses directories."""
    parts = re.split(r'(\*\*|\*)', glob)
    return re.compile(''.join('.*' if part == '**' else '[^/]*' if part == '*' else re.escape(part)
                              for part in parts) + '$')


def matches(path: str, patterns) -> bool:
    return any(pattern(glob).match(path) for glob in patterns)


def code_read(path: str, buck_text: str) -> bool:
    """Whether the generated BUCK names `path` as a source or data file, literally
    or under one of its `<dir>/**` globs."""
    if f'"{path}"' in buck_text:
        return True
    return any(path.startswith(prefix + '/') for prefix in buck_globs(buck_text))


_GLOBS: dict[int, set[str]] = {}


def buck_globs(buck_text: str) -> set[str]:
    key = hash(buck_text)
    if key not in _GLOBS:
        _GLOBS[key] = set(re.findall(r'"([^"*]+)/\*\*"', buck_text))
    return _GLOBS[key]


def classify(paths, members, buck_text: str) -> Classified:
    """Sort changed repository paths by what they can affect. `members` are the
    workspace crates by directory name; `buck_text` is the root BUCK."""
    result = Classified()
    for path in sorted(set(paths)):
        # Build-graph edits must exercise the static graph reader as well as
        # retaining their usual build-wide or per-target selection.
        graph_tooling = path.startswith('tools/buck2/') or path.rsplit('/', 1)[-1] in ('BUCK', 'defs.bzl')
        if graph_tooling:
            result.tools.append(path)
        if path == 'BUCK':
            continue  # by target, from the BUCK diff (buck_targets)
        if matches(path, DOCS) and not code_read(path, buck_text):
            result.docs.append(path)
        elif matches(path, TOOL_PATHS) and not code_read(path, buck_text):
            if not graph_tooling:
                result.tools.append(path)
        elif matches(path, PROOFS):
            result.proofs.append(path)
        elif matches(path, TOOL_INPUTS):
            result.owned.append(path)
        elif matches(path, BUILD_WIDE):
            result.build_wide.append(path)
        elif path.startswith('crates/'):
            parts = path.split('/')
            crate = parts[1]
            if crate not in members:
                result.unknown_crates.add(crate)
                continue
            result.owned.append(path)
            if len(parts) > 2 and parts[2] not in CRATE_TEST_DIRS and parts[2] != 'README.md':
                result.crates.add(crate)
                # A member this copy of the table does not know yet (tools/land runs
                # main's copy on a lane's tree that adds a crate) runs every campaign,
                # and is still the change's crate, so gate and land agree on it.
                result.campaigns |= SOURCE_CAMPAIGNS.get(path, CRATE_CAMPAIGNS.get(crate, ALL))
        else:
            result.owned.append(path)
    return result


def buck_rules(text: str) -> tuple[str, dict[str, str]]:
    """The generated BUCK's header (everything before its first rule) and each
    rule's text by target name. sync.py writes every rule as `<rule>(` on its
    own line, its attributes indented, and `)` alone at the end."""
    lines = text.splitlines(keepends=True)
    header, rules, current = [], {}, None
    for line in lines:
        if current is None and re.match(r'\w+\($', line.rstrip('\n')):
            current = [line]
        elif current is not None:
            current.append(line)
            if line == ')\n':
                rule = ''.join(current)
                name = re.search(r'^    name = "([^"]+)",$', rule, re.M)
                rules[name[1] if name else rule] = rule
                current = None
        elif not rules:
            header.append(line)
    return ''.join(header), rules


def buck_targets(old: str, new: str) -> bool | set[str]:
    """What a BUCK edit selects: True when its header changed, otherwise the
    names of the targets whose rule is new or changed."""
    old_header, old_rules = buck_rules(old)
    new_header, new_rules = buck_rules(new)
    if old_header != new_header:
        return True
    return {name for name, rule in new_rules.items() if old_rules.get(name) != rule}


def changed_test_requests(old: str, new: str) -> set[str]:
    """Compare generated request values; evidence-only edits run tool checks.

    Parse literals rather than executing repository code. Refuse a changed
    file shape so a rule change cannot silently be treated as sizing data.
    """
    def requests(text):
        tree = ast.parse(text)
        if (len(tree.body) != 1 or not isinstance(tree.body[0], ast.Assign)
                or len(tree.body[0].targets) != 1
                or not isinstance(tree.body[0].targets[0], ast.Name)
                or tree.body[0].targets[0].id != 'TEST_REQUESTS'):
            raise ValueError('test_sizes.bzl must contain only the generated TEST_REQUESTS assignment')
        rows = ast.literal_eval(tree.body[0].value)
        if not isinstance(rows, dict) or any(
                not isinstance(name, str) or not isinstance(row, list) or len(row) != 3
                or type(row[0]) is not int or not 1 <= row[0] <= 4
                or type(row[1]) is not int or row[1] <= 0 or not isinstance(row[2], str)
                for name, row in rows.items()):
            raise ValueError('invalid TEST_REQUESTS')
        return rows
    before, after = requests(old), requests(new)
    return {name for name in before.keys() | after.keys() if before.get(name) != after.get(name)}


# ---------------------------------------------------------------- the corpus

# The outcome word a corpus case's name ends with. Cases that differ only in it
# are twins: a rejected case and the accepted case that isolates its rule
# (conformance/README.md).
OUTCOMES = ('ok', 'bad', 'run', 'fail', 'reject', 'accept')
PASSING = 'conformance/passing.txt'


def corpus_program(path: str) -> str | None:
    """The corpus case a changed path is, a case or its sidecar manifest."""
    path = path.removesuffix('.json') if path.endswith('.hir.json') else path
    return path if path.startswith('conformance/') and path.endswith('.hir') else None


def case_stem(path: str) -> str:
    stem = path.removesuffix('.hir')
    head, _, word = stem.rpartition('-')
    return head if head and word in OUTCOMES else stem


def twins(program: str, root) -> list[str]:
    """The existing cases beside `program` with its stem, itself included."""
    directory = root / program.rpartition('/')[0]
    if not directory.is_dir():
        return []
    stem = case_stem(program)
    return sorted(p.relative_to(root).as_posix() for p in directory.glob('*.hir')
                  if case_stem(p.relative_to(root).as_posix()) == stem)


def ratchet_programs(diff: str) -> set[str]:
    """The cases whose `passing.txt` line a `git diff -U0` adds or removes."""
    return {line[1:].strip() for line in diff.splitlines()
            if line[:1] in '+-' and not line.startswith(('+++', '---'))
            and corpus_program(line[1:].strip())}


def changed_programs(paths, root, ratchet: set[str] = frozenset()) -> list[str]:
    """Every changed or added corpus case that still exists, with its twins: the
    cases the floor of every gate runs. A deleted case's twins run too."""
    programs = {program for path in paths if (program := corpus_program(path))} | set(ratchet)
    return sorted({twin for program in programs for twin in twins(program, root)})


# ---------------------------------------------------------------- std's import closure

STD = 'stdlib/std@1'
STD_MANIFEST = f'{STD}/hirsel.json'
# A module path as source spells it: segments joined by `.` (expressions) or
# `/` (type context and versioned paths), whitespace allowed (LANG-4).
CHAIN = re.compile(r'[A-Za-z_][A-Za-z0-9_]*(?:\s*[./]\s*[A-Za-z_][A-Za-z0-9_]*)+')
SEGMENT = re.compile(r'[A-Za-z_][A-Za-z0-9_]*')


# A top-level definition's name, after its attributes and modifiers.
DEFINITION = re.compile(r'^(?:@[\w.]+(?:\([^)\n]*\))?\s+)*(?:pub\s+)?(?:[a-z_]+\s+)*?(?:fn|type|const)\s+([A-Za-z_]\w*)', re.M)
# Core's signature table: a core function `fn list.push(...)` is core's member.
CORE_SIGNATURES = 'crates/hirsel-engine/src/registry/section22.txt'
CORE_MEMBER = re.compile(r'\bfn ([a-z_0-9]+)\.([A-Za-z_]\w*)')


class Std:
    """std's modules, each one's definitions, and core's members under the
    module heads core and std share (`string.byte_len` is core's,
    `string.find` std's: LANG-4)."""

    def __init__(self, root, read_text=None):
        read_text = read_text or (lambda path: (root / path).read_text(errors='replace'))
        self.manifest = manifest = json.loads(read_text(STD_MANIFEST))
        self.files = {f'{STD}/{source["path"]}': tuple(source['module']) for source in manifest['sources']}
        self.modules = set(self.files.values())
        self.members: dict[tuple[str, ...], set[str]] = {module: set() for module in self.modules}
        self.texts = {}
        for path, module in self.files.items():
            self.texts[path] = read_text(path)
            self.members[module] |= set(DEFINITION.findall(self.texts[path]))
        self.core: dict[str, set[str]] = {}
        for head, member in CORE_MEMBER.findall(read_text(CORE_SIGNATURES)):
            self.core.setdefault(head, set()).add(member)

    def mentioned(self, text: str, aliases=None) -> set[tuple[str, ...]]:
        """The modules a source names: a path with one of them as a proper prefix
        (`time.now`, `internal.format.pad`, `json/Value`) or through an exposed
        alias head, unless what follows is core's member under a shared head.
        It over-approximates: a local named like a module counts."""
        found = set()
        aliases = aliases or {}
        for chain in CHAIN.findall(text):
            segments = tuple(SEGMENT.findall(chain))
            if segments[0] in aliases:
                target = aliases[segments[0]]
                found |= {m for m in self.modules if m[:len(target)] == target}
            for length in range(1, len(segments)):
                module, member = segments[:length], segments[length]
                if module in self.modules and (member in self.members[module]
                                               or (*module, member) in self.modules
                                               or length > 1
                                               or member not in self.core.get(module[0], ())):
                    found.add(module)
        return found

    def reach(self) -> dict[tuple[str, ...], set[tuple[str, ...]]]:
        """Each module's import closure within std, itself included."""
        direct = {module: {module} for module in self.modules}
        for path, module in self.files.items():
            direct[module] |= self.mentioned(self.texts[path])
        reach = {}
        for module in self.modules:
            seen, frontier = set(), [module]
            while frontier:
                current = frontier.pop()
                if current not in seen:
                    seen.add(current)
                    frontier.extend(direct[current] - seen)
            reach[module] = seen
        return reach


def changed_std_modules(paths, std: Std, previous: Std | None = None) -> set[tuple[str, ...]] | None:
    """The std modules a change edits, or None when it changes std as a whole
    (a manifest field outside sources, or a file that is no module source).
    Compare source entries by path, so order and JSON formatting do not matter;
    a re-pointed source changes both its old and its new module.
    """
    changed = set()
    if STD_MANIFEST in paths:
        if previous is None:
            return None
        before, after = previous.manifest, std.manifest
        if ({k: v for k, v in before.items() if k != 'sources'} !=
                {k: v for k, v in after.items() if k != 'sources'}):
            return None
        old = {source['path']: source for source in before['sources']}
        new = {source['path']: source for source in after['sources']}
        for path in old.keys() | new.keys():
            if old.get(path) != new.get(path):
                changed |= {tuple(sources[path]['module']) for sources in (old, new) if path in sources}
    for path in paths:
        if path.startswith(f'{STD}/') and path != STD_MANIFEST:
            modules = {version.files[path] for version in (std, previous)
                       if version is not None and path in version.files}
            if not modules:
                return None
            changed |= modules
    return changed


def package_std_reach(manifest, root, std: Std, reach, seen=None) -> set[tuple[str, ...]]:
    """The std modules a package's sources reach, through its own std
    dependency and through every package it depends on."""
    seen = set() if seen is None else seen
    manifest = manifest.resolve()
    if manifest in seen:
        return set()
    seen.add(manifest)
    data = json.loads(manifest.read_text())
    std_manifest = (root / STD_MANIFEST).resolve()
    found = set()
    texts = [(manifest.parent / source['path']).read_text(errors='replace')
             for source in data.get('sources', []) if (manifest.parent / source['path']).is_file()]
    for dependency in data.get('dependencies', []):
        target = (manifest.parent / dependency['manifest']).resolve()
        if target == std_manifest:
            aliases = {expose['head']: tuple(expose['module']) for expose in dependency.get('expose', [])}
            for text in texts:
                for module in std.mentioned(text, aliases):
                    found |= reach[module]
        elif target.is_file():
            found |= package_std_reach(target, root, std, reach, seen)
    return found


def std_dependents(changed, root, std: Std | None = None) -> list[str]:
    """The corpus cases whose import closure includes a changed std module
    (`changed` None: every case that selects std). Only a case's sidecar
    selects std (PROG-1), so a case without one depends on none."""
    std = std or Std(root)
    reach = std.reach()
    programs = []
    for manifest in sorted((root / 'conformance').rglob('*.hir.json')):
        program = manifest.with_suffix('')
        if not program.is_file():
            continue
        modules = package_std_reach(manifest, root, std, reach)
        if modules and (changed is None or modules & changed):
            programs.append(program.relative_to(root).as_posix())
    return programs


def std_test_inputs(changed, root, std: Std) -> list[str]:
    """Inputs of package and example tests reaching changed std modules.

    Follow manifests even without a corpus caller. Returning their sources
    too lets the build graph find tests with literal inputs as well as globs.
    """
    reach = std.reach()
    inputs = set()
    manifests = sorted({manifest for directory in ('packages', 'examples')
                        for manifest in (root / directory).rglob('hirsel.json')})
    for manifest in manifests:
        modules = package_std_reach(manifest, root, std, reach)
        if modules and (changed is None or modules & changed):
            data = json.loads(manifest.read_text())
            inputs.add(manifest.relative_to(root).as_posix())
            inputs.update((manifest.parent / source['path']).resolve().relative_to(root.resolve()).as_posix()
                          for source in data.get('sources', []))
    return sorted(inputs)


def package_files(manifest, root, seen=None) -> set:
    """The sources and manifests a package selects, through every dependency
    but std (std_dependents follows std by module)."""
    seen = set() if seen is None else seen
    manifest = manifest.resolve()
    if manifest in seen or manifest == (root / STD_MANIFEST).resolve() or not manifest.is_file():
        return set()
    seen.add(manifest)
    data = json.loads(manifest.read_text())
    files = {manifest} | {(manifest.parent / source['path']).resolve() for source in data.get('sources', [])}
    for dependency in data.get('dependencies', []):
        files |= package_files(manifest.parent / dependency['manifest'], root, seen)
    return files


def package_dependents(paths, root) -> list[str]:
    """The corpus cases whose sidecar's packages select a changed file outside
    the corpus and std: a source or manifest of a package fixture, or a file
    added beside one."""
    changed = [(root / path).resolve() for path in paths
               if not path.startswith(('conformance/', f'{STD}/'))]
    if not changed:
        return []
    programs = []
    for manifest in sorted((root / 'conformance').rglob('*.hir.json')):
        files = package_files(manifest, root)
        directories = {file.parent for file in files if file.name == 'hirsel.json'}
        if any(path in files or path.parent in directories for path in changed):
            programs.append(manifest.with_suffix('').relative_to(root).as_posix())
    return programs


def campaign_data(path: str) -> frozenset:
    """The whole campaigns a changed data file outside the per-case rules can
    affect: the spec's rule table every case header names, a corpus file that
    is no case (such as a list the runner reads), and the native fixtures."""
    if path.startswith('docs/spec/'):
        return FRONT
    if path.startswith(('native-tests/', 'bench/native/programs/')):
        return frozenset({NATIVE_SMOKE})
    if (path.startswith('conformance/') and not corpus_program(path)
            and path != PASSING and not path.endswith('.md')):
        return frozenset({CONFORMANCE})
    return frozenset()


def proof_checks(paths) -> list[tuple[str, list[str]]]:
    """The models' own checks a change selects: the Lean audit when the Lean model
    changes, its vector replay when the model or a vector changes, and the TLC
    runs when the ordered-mode model changes."""
    model = any(matches(path, PROOFS) and not matches(path, ORDERED_MODEL) for path in paths)
    vectors = model or any(path.startswith(PROOF_VECTORS) for path in paths)
    ordered = any(matches(path, ORDERED_MODEL) for path in paths)
    return ([('proofs/rc/audit.sh', ['proofs/rc/audit.sh'])] if model else []) + \
        ([('proofs/rc/check-vectors.sh', ['proofs/rc/check-vectors.sh'])] if vectors else []) + \
        ([('proofs/ordered/check.sh', ['proofs/ordered/check.sh'])] if ordered else [])
