"""The root BUCK's targets, their inputs and their dependencies, read without
Buck2: sync.py generates the file from Cargo, and every rule is a call of a
tools/buck2/defs.bzl macro whose targets this module expands the same way.
tools/gate --suggest and its targeted set query it instead of starting a
Buck2 daemon, so a suggestion never waits behind the worktree's daemon.

Only the per-push graph is modelled: `full` stages (including full-only script
tests) exist only under `-c hirsel.full=true`, and third-party targets are
build-wide inputs (impact.BUILD_WIDE) that select everything anyway. Against `buck2 uquery` on
the repository's BUCK it gave the same tests for every crate, for all tests,
and for the owners' reverse dependencies of 330 sampled files; a new macro
must be taught here (an unknown one is refused).
"""

from __future__ import annotations

import ast
from dataclasses import dataclass, field
import re

# Packages under the root whose files no root glob reaches.
SUBPACKAGES = ('third-party/', 'tools/buck2/')


@dataclass(frozen=True)
class Glob:
    include: tuple[str, ...]
    exclude: tuple[str, ...] = ()

    def matches(self, path: str) -> bool:
        if path.startswith(SUBPACKAGES):
            return False
        return (any(pattern(glob).fullmatch(path) for glob in self.include)
                and not any(pattern(glob).fullmatch(path) for glob in self.exclude))


PATTERNS: dict[str, re.Pattern] = {}


def pattern(glob: str) -> re.Pattern:
    """Buck2's glob: `**` is any number of directories, `*` stays in one."""
    if glob not in PATTERNS:
        out, i = '', 0
        while i < len(glob):
            if glob.startswith('**/', i):
                out, i = out + '(?:[^/]+/)*', i + 3
            elif glob.startswith('**', i):
                out, i = out + '.*', i + 2
            elif glob[i] == '*':
                out, i = out + '[^/]*', i + 1
            else:
                out, i = out + re.escape(glob[i]), i + 1
        PATTERNS[glob] = re.compile(out)
    return PATTERNS[glob]


@dataclass
class Target:
    name: str
    test: bool = False          # a `_rust_test` outside the gate stages
    doctest: bool = False       # a library that tests its documentation examples
    inputs: list = field(default_factory=list)   # paths and Globs
    deps: set = field(default_factory=set)

    def reads(self, path: str, exists=True) -> bool:
        """Whether this target reads `path`; a glob matches only an existing file."""
        return any(item.matches(path) and exists if isinstance(item, Glob) else item == path for item in self.inputs)


def evaluate(node):
    """A rule argument's value: literals, lists, dicts, `+` and glob()."""
    if isinstance(node, ast.Constant):
        return node.value
    if isinstance(node, ast.List):
        return [evaluate(item) for item in node.elts]
    if isinstance(node, ast.Dict):
        return {evaluate(key): evaluate(value) for key, value in zip(node.keys, node.values)}
    if isinstance(node, ast.BinOp) and isinstance(node.op, ast.Add):
        return evaluate(node.left) + evaluate(node.right)
    if isinstance(node, ast.Call) and getattr(node.func, 'id', None) == 'glob':
        arguments = {keyword.arg: evaluate(keyword.value) for keyword in node.keywords}
        include = evaluate(node.args[0]) if node.args else arguments['include']
        return [Glob(tuple(include), tuple(arguments.get('exclude', ())))]
    raise ValueError(f'BUCK: unsupported expression {ast.dump(node)[:80]}')


def label(value):
    """A root target's name from `//:x` or `:x`; None for another package's."""
    if isinstance(value, str):
        if value.startswith(('//:', 'root//:')):
            return value.partition(':')[2]
        if value.startswith(':'):
            return value[1:]
    return None


def source(value):
    """A source attribute: a path, or a file label of another package."""
    if isinstance(value, str) and value.startswith(('//', 'root//')):
        package, _, name = value.partition('//')[2].partition(':')
        return f'{package}/{name}' if package else name
    return value


def labels(*values):
    names = set()
    for value in values:
        items = value.values() if isinstance(value, dict) else value if isinstance(value, list) else [value]
        for item in items:
            if isinstance(item, list):
                names |= labels(item)
            elif (name := label(item)) is not None:
                names.add(name)
    return names


def sources(*values):
    items = []
    for value in values:
        for item in value if isinstance(value, list) else [value]:
            if item is not None and label(item) is None:
                items.append(source(item))
    return items


def targets(text: str) -> dict[str, Target]:
    """Every per-push target the root BUCK's macro calls declare."""
    found = {}

    def add(target):
        found[target.name] = target

    for statement in ast.parse(text).body:
        call = statement.value if isinstance(statement, ast.Expr) else None
        if not isinstance(call, ast.Call) or not isinstance(call.func, ast.Name) or call.func.id == 'load':
            continue
        macro, a = call.func.id, {keyword.arg: evaluate(keyword.value) for keyword in call.keywords}
        name = a['name']
        if macro in ('hirsel_library', 'hirsel_binary'):
            add(Target(name, doctest=bool(a.get('doctests')), inputs=sources(a.get('srcs', []), a.get('crate_root')),
                       deps=labels(a.get('deps', []), a.get('build_script'))))
        elif macro == 'hirsel_build_script':
            add(Target(name, inputs=sources(a['srcs'], a['crate_root'], a.get('inputs', [])), deps=labels(a['deps'])))
        elif macro == 'hirsel_runtime_archives':
            add(Target(name, inputs=sources(a['srcs'], a['crate_root']),
                       deps=labels(a['deps'], *a.get('feature_deps', {}).values())))
        elif macro == 'hirsel_test':
            harness = name + '-harness'
            add(Target(harness, inputs=sources(a.get('srcs', []), a.get('crate_root')),
                       deps=labels(a.get('deps', []), a.get('build_script'))))
            shards = a.get('shards', 1)
            for shard in range(shards):
                add(Target(name if shards == 1 else f'{name}-shard-{shard + 1}', test=True,
                           inputs=sources(a.get('data', []), a.get('costs')),
                           deps={harness} | labels(a.get('bin_exes', {}), a.get('runtime_archives'), a.get('precheck'))))
        elif macro == 'hirsel_command_test':
            if not a.get('full') and not a.get('selected'):
                add(Target(name, inputs=sources(a.get('data', [])),
                           deps=labels(a['binary'], a.get('runtime_archives'), a.get('precheck'),
                                       a.get('native_selection'))))
        elif macro == 'hirsel_precheck':
            add(Target(name, inputs=sources(a['srcs'], a['manifest']), deps=labels(a['hirsel'])))
        elif macro == 'hirsel_toolchain_bundle':
            add(Target(name, deps=labels(a['hirsel'], a['precheck'])))
        elif macro == 'hirsel_native_selection':
            add(Target(name, inputs=sources(a.get('data', [])), deps=labels(a['collector'])))
        elif macro == 'hirsel_script_test':
            if not a.get('full', True):
                add(Target(name, test=True, inputs=sources(a['script'], a.get('data', [])),
                           deps=labels(a.get('bin_exes', {}), a.get('runtime_archives'), a.get('precheck'))))
        elif macro == 'hirsel_link_hosts_test':
            continue  # A full-gate stage.
        else:
            raise ValueError(f'BUCK: unknown macro {macro} (teach tools/buck2/graph.py its targets)')
    return found


class Graph:
    """Queries over the root BUCK's targets, in the gate's terms."""

    def __init__(self, text: str, root=None):
        self.root = root
        self.targets = targets(text)
        self.rdeps_of = {}
        for target in self.targets.values():
            for dep in target.deps:
                self.rdeps_of.setdefault(dep, set()).add(target.name)

    def tests(self, names) -> set[str]:
        """The tests among `names`: `_rust_test`s outside the gate stages and
        libraries that test their documentation examples."""
        return {name for name in names if (t := self.targets.get(name)) and (t.test or t.doctest)}

    def all_tests(self) -> set[str]:
        return self.tests(self.targets)

    def owners(self, paths) -> set[str]:
        exists = {path: self.root is None or (self.root / path).is_file() for path in paths}
        return {name for name, target in self.targets.items() if any(target.reads(path, exists[path]) for path in paths)}

    def rdeps(self, names) -> set[str]:
        seen, frontier = set(), list(names)
        while frontier:
            name = frontier.pop()
            if name not in seen:
                seen.add(name)
                frontier.extend(self.rdeps_of.get(name, ()))
        return seen

    def crate_tests(self, crates) -> set[str]:
        """Every test target of these crates: unit, integration (all shards),
        binary unit tests and doctests."""
        names = '|'.join(re.escape(crate) for crate in sorted(crates))
        tests = re.compile(rf'({names})-(unittest|test-.+|bin-.+-unittest)')
        return {name for name, t in self.targets.items()
                if (t.test and tests.fullmatch(name)) or (t.doctest and name in crates)}

    def macro_targets(self, names) -> set[str]:
        """The targets each generated rule call declares."""
        rule = re.compile('(' + '|'.join(re.escape(name) for name in sorted(names)) + r')(-harness|-shard-[0-9]+)?')
        return {name for name in self.targets if rule.fullmatch(name)}
