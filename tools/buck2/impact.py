#!/usr/bin/env python3
"""What a change can affect: the table tools/gate selects its work from.

A changed path is one of:

- documentation (`*.md`, `LICENSE`, `docs/`) that no build or test reads: a
  fast consistency check of its links and whitespace, nothing else;
- the gate's own Python tooling (TOOL_PATHS): its tests, and `sync.py --check`;
- a build-wide input (BUILD_WIDE): every test and every campaign;
- anything else: the Buck2 tests that read it, directly or through any
  dependency (`rdeps(//..., owner(<paths>))`). A file a campaign reads as data
  (the corpus, the stdlib, native fixtures) selects that campaign through its
  owner, and a crate's library source selects the campaigns in
  CRATE_CAMPAIGNS.

The generated root BUCK selects by target: the targets whose generated rule
changed, and every test that depends on them. Its header changing is
build-wide.
"""

from __future__ import annotations

from dataclasses import dataclass, field
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
    'hirsel-codec': frozenset({CONFORMANCE, NATIVE_SMOKE}),
    'hirsel-engine': ALL,
    'hirsel-driver': ALL,
    'hirsel-conformance': ALL,
    # The backend, the runtime and LLVM.
    'hirsel-core-contract': frozenset({NATIVE_SMOKE}),
    'hirsel-lir': frozenset({NATIVE_SMOKE}),
    'hirsel-rc': frozenset({NATIVE_SMOKE}),
    'hirsel-heapcheck': frozenset({NATIVE_SMOKE}),
    'hirsel-llvm': frozenset({NATIVE_SMOKE}),
    'hirsel-rt': frozenset({NATIVE_SMOKE}),
    # Tool and test crates: only their own tests.
    'hirsel-cli': frozenset(),
    'hirsel-native-build': frozenset(),
    'hirsel-pipeline-tests': frozenset(),
}

# A crate's paths that are test code or test data, not its library source:
# their edits select only the tests that read them.
CRATE_TEST_DIRS = ('tests', 'benches', 'examples', 'proptest-regressions')

# The gate's own tooling. Each is a script with tests in tools/tests; none is
# a build or test input (sync.py --check proves that the generated build files
# do not change).
TOOL_PATHS = (
    'tools/gate', 'tools/land', 'tools/main-stall-alarm', 'tools/*.sh', 'tools/*.py', 'tools/tests/*',
    *(f'tools/buck2/{name}.py' for name in (
        'action_sizes_from_log', 'agree', 'clippy', 'conformance', 'fmt', 'full_gate_counts',
        'gating', 'impact', 'pool_usage', 'sync', 'test_costs')),
)

# Inputs of every action or of the toolchains: the Cargo workspace and lock,
# Buck2's configuration and rules, the third-party build and the bootstrap.
BUILD_WIDE = (
    'Cargo.toml', 'rust-toolchain.toml', '.buckconfig', '.cargo/**', 'third-party/**',
    'tools/buck2/**',
)
# Under tools/ but read by particular tests: selected through their owners.
TOOL_INPUTS = ('tools/buck2/test-costs/**', 'tools/native-toolchain/**')

# Documentation, and repository metadata no build reads.
DOCS = ('**.md', 'LICENSE', 'docs/**', '.gitignore')


@dataclass
class Classified:
    """A change's paths by what they can affect."""
    docs: list[str] = field(default_factory=list)
    tools: list[str] = field(default_factory=list)
    build_wide: list[str] = field(default_factory=list)
    # Paths whose Buck2 owners and their reverse dependencies are selected.
    owned: list[str] = field(default_factory=list)
    # The root BUCK changed: True (header) or the names of changed targets.
    buck: bool | set[str] = False
    crates: set[str] = field(default_factory=set)
    campaigns: set[str] = field(default_factory=set)
    unknown_crates: set[str] = field(default_factory=set)

    @property
    def everything(self) -> bool:
        return bool(self.build_wide) or self.buck is True or bool(self.unknown_crates)

    @property
    def needs_buck2(self) -> bool:
        return self.everything or bool(self.owned) or bool(self.buck)


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
        if path == 'BUCK':
            continue  # by target, from the BUCK diff (buck_targets)
        if matches(path, DOCS) and not code_read(path, buck_text):
            result.docs.append(path)
        elif matches(path, TOOL_PATHS):
            result.tools.append(path)
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
            if len(parts) > 2 and parts[2] not in CRATE_TEST_DIRS:
                result.crates.add(crate)
                result.campaigns |= CRATE_CAMPAIGNS[crate]
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
