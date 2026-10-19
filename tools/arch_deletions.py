#!/usr/bin/env python3
"""The compiler-architecture deletion list (compiler-arch design §2): what survives.

    tools/arch_deletions.py            # survivors; exit 1 if any (the cutover needs none)
    tools/arch_deletions.py --present  # entries that match nothing; exit 1 if any

Each entry is checked mechanically: a path that must be gone, a word or text
that must not occur in crates/, a struct field or a method that must be gone.
Items the design names by line range or behaviour, which no pattern can find,
are listed as MANUAL for the integrator to confirm by reading. `--present`
checks the list itself against a tree before the cutover: every mechanical
entry must find something there, so that zero survivors after it means the
item was deleted rather than misspelled.
"""

import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SELF = 'tools/arch_deletions.py'

ENGINE = 'crates/hirsel-engine/src'

# (lane, item, kind, spec). Kinds: path; word (whole word, crates/ and tools/);
# text (literal, optionally under a path); field (struct, field); method (type,
# method); manual (a note).
ENTRIES = [
    ('item-syntax', 'queries/positions.rs', 'path', f'{ENGINE}/queries/positions.rs'),
    ('item-syntax', 'rebase_package_snapshot', 'word', 'rebase_package_snapshot'),
    ('item-syntax', 'tir_spans remapping', 'word', 'tir_spans'),
    ('item-syntax', 'the movements code in index (queries.rs:646-693, :812)', 'manual', ''),
    ('item-syntax', 'absolute spans in adapt.rs:48 and emission.rs:178 site_code', 'manual', ''),
    ('item-syntax', 'file-wide syntax arenas (one SyntaxTree per file)', 'manual', ''),
    ('arch-engine', 'ValidationWalk', 'word', 'ValidationWalk'),
    ('arch-engine', 'with_validation_walk', 'word', 'with_validation_walk'),
    ('arch-engine', 'the visited set in valid_node', 'manual', ''),
    ('arch-engine', 'source_revision', 'word', 'source_revision'),
    ('arch-engine', 'PackageProof', 'word', 'PackageProof'),
    ('arch-engine', 'QueryEngine.reads', 'field', ('QueryEngine', 'reads')),
    ('arch-engine', 'Cached', 'text', ('struct Cached', ENGINE)),
    ('arch-engine', 'key() header hashing', 'manual', ''),
    ('arch-engine', 'layered.rs', 'path', f'{ENGINE}/layered.rs'),
    ('arch-engine', 'Layered', 'word', 'Layered'),
    ('arch-engine', 'package_cache.rs (except the image loader)', 'word', 'PackageCache'),
    ('arch-engine', 'queries/package_image.rs', 'path', f'{ENGINE}/queries/package_image.rs'),
    ('arch-engine', 'PackageSnapshot', 'word', 'PackageSnapshot'),
    ('arch-engine', 'adopt_package_snapshot', 'word', 'adopt_package_snapshot'),
    ('arch-engine', 'publish_lowering', 'word', 'publish_lowering'),
    ('arch-engine', 'package_snapshot_valid', 'word', 'package_snapshot_valid'),
    ('arch-engine', 'HIRSEL_PACKAGE_CACHE', 'word', 'HIRSEL_PACKAGE_CACHE'),
    ('arch-engine', 'the reused bookkeeping in LoadedProgram', 'field', ('PackageRevision', 'reused')),
    ('arch-engine', 'the proven bookkeeping in LoadedProgram', 'field', ('PackageRevision', 'proven')),
    ('arch-engine', 'compiler_state', 'word', 'compiler_state'),
    ('arch-engine', 'the Rc adapter fields', 'manual', ''),
    ('arch-engine', 'the single-thread invariant in stack.rs', 'word', 'COMPILER_THREAD'),
    ('arch-engine', '(*modes).clone()', 'text', ('(*modes).clone()', 'crates')),
    ('arch-engine', 'the by-value AnalyzedBody.typed', 'manual', ''),
    ('arch-snapshot', 'shared_input', 'word', 'shared_input'),
    ('arch-snapshot', 'SHARED_INPUTS', 'word', 'SHARED_INPUTS'),
    ('arch-snapshot', 'PASSED_PER_DEFINITION', 'word', 'PASSED_PER_DEFINITION'),
    ('arch-snapshot', 'ProgramInput', 'word', 'ProgramInput'),
    ('arch-snapshot', 'invalidate_signature_input', 'word', 'invalidate_signature_input'),
    ('arch-snapshot', 'the deep clones in signature_input.rs', 'manual', ''),
    ('arch-snapshot', 'SignatureOverlay', 'word', 'SignatureOverlay'),
    ('arch-snapshot', "hirsel-validate's owned Signatures input and matches", 'manual', ''),
    ('arch-snapshot', 'publish_declarations', 'word', 'publish_declarations'),
    ('arch-snapshot', 'CompilerState', 'word', 'CompilerState'),
    ('arch-snapshot', 'set_declarations', 'word', 'set_declarations'),
    ('arch-snapshot', 'ModuleInput', 'word', 'ModuleInput'),
    ('arch-snapshot', 'runtime_types_with_finalizers', 'word', 'runtime_types_with_finalizers'),
    ('arch-snapshot', 'the source_revision bumps in generation.rs', 'text',
     ('source_revision', f'{ENGINE}/queries/generation.rs')),
    ('arch-snapshot', 'ExecutionReads', 'word', 'ExecutionReads'),
    ('arch-snapshot', 'inherit_execution', 'word', 'inherit_execution'),
    ('arch-snapshot', 'executions_valid', 'word', 'executions_valid'),
    ('arch-snapshot', 'the unrecorded runtime.serialization table (queries.rs:1216-1224)', 'manual', ''),
    ('arch-snapshot', 'comptime_stack.len() depth checks', 'text', ('comptime_stack.len()', ENGINE)),
    ('arch-snapshot', 'InstanceBodies::key returning Vec<String>', 'manual', ''),
    ('arch-types', 'TypeStore', 'word', 'TypeStore'),
    ('arch-types', 'TypeStoreMark', 'word', 'TypeStoreMark'),
    ('arch-types', 'truncate_to', 'word', 'truncate_to'),
    ('arch-types', 'fresh_id', 'word', 'fresh_id'),
    ('arch-types', 'provenance and the encode/decode sessions (store.rs)', 'path',
     'crates/hirsel-types/src/store.rs'),
    ('arch-types', 'Importer', 'word', 'Importer'),
    ('arch-types', 'import_type', 'word', 'import_type'),
    ('arch-types', 'substitute_type', 'word', 'substitute_type'),
    ('arch-types', 'substitute_type_in_place', 'word', 'substitute_type_in_place'),
    ('arch-types', 'the store-based shape_equal', 'word', 'shape_equal'),
    ('arch-types', 'the store-based without_edges', 'word', 'without_edges'),
    ('arch-types', 'the store-based semantic_equal_adjusted', 'word', 'semantic_equal_adjusted'),
    ('arch-types', 'the store-based compatible* (properties.rs:966-1204)', 'manual', ''),
    ('arch-types', 'nominal height', 'text', ('fn height(store: &TypeStore', 'crates/hirsel-types')),
    ('arch-types', 'nominal embedding', 'text', ('fn embedding(older: &TypeStore', 'crates/hirsel-types')),
    ("arch-types", "checker-fixes' Shape memo", 'manual', ''),
    ('arch-types', 'Signature.types', 'field', ('Signature', 'types')),
    ('arch-types', 'the store in InstanceEdge', 'field', ('InstanceEdge', 'types')),
    ('arch-types', 'the TypeStore inside InstanceBodies entries', 'manual', ''),
    ('arch-types', 'the types.clone() sites', 'text', ('types.clone()', 'crates')),
    ('arch-types', 'CopiedTypeRows', 'word', 'CopiedTypeRows'),
    ('arch-types', 'EqualityRows', 'word', 'EqualityRows'),
    ('arch-types', 'the per-engine Interner', 'text', ('pub struct Interner', 'crates/hirsel-intern')),
    ('arch-native', 'UnitHeader.input', 'field', ('UnitHeader', 'input')),
    ('arch-native', 'UnitHeader.layouts', 'field', ('UnitHeader', 'layouts')),
    ('arch-native', 'RawReceipt.input', 'field', ('RawReceipt', 'input')),
    ('arch-native', 'RawReceipt.layouts', 'field', ('RawReceipt', 'layouts')),
    ('arch-native', 'RawReceipt.manifest', 'field', ('RawReceipt', 'manifest')),
    ('arch-native', 'LayoutTable::digest()', 'method', ('LayoutTable', 'digest')),
    ('arch-native', 'the per-unit manifest digests', 'manual', ''),
    ('arch-native', 'the descriptor-index immediates (construction.rs:31-35, data.rs:71-84)', 'manual', ''),
    ('arch-native', 'per-unit callback thunks (module.rs:299-312)', 'manual', ''),
    ('arch-native', 'object-local codec programs in both backends', 'manual', ''),
    ('arch-native', 'program-enumerated failure codes in unit code', 'manual', ''),
    ('arch-native', 'per-unit DWARF and source paths in unit code', 'manual', ''),
    ('arch-native', 'the RC barrier around Finalizers::new (stages.rs:336)', 'manual', ''),
    ('arch-native', "import_native_input's second export, SyntaxFile compares and V rerun", 'manual', ''),
    ('rc-state', 'the three BTreeMap<Vec<Step>, Tri>', 'text', ('BTreeMap<Vec<Step>, Tri>', 'crates/hirsel-rc')),
    ('rc-state', 'the double validate_rc', 'manual', ''),
    ('arch-image', 'Precheck::read', 'text', ('Precheck::read', 'crates')),
    ('arch-image', 'HIRPCHK2', 'word', 'HIRPCHK2'),
    ('arch-image', 'the Mutex decode map', 'text', ('decoded: Mutex<', ENGINE)),
    ('arch-image', 'the bincode Header, std re-parse, std signature_query from source', 'manual', ''),
]


def grep(args, paths):
    out = subprocess.run(['git', 'grep', '-n', *args, '--', *paths, f':!{SELF}'],
                         cwd=ROOT, capture_output=True, text=True)
    return out.stdout.splitlines()


def struct_blocks(name):
    """(file, body) of every `struct <name> {` in crates/."""
    pattern = re.compile(r'\bstruct ' + re.escape(name) + r'\b[^{;]*\{')
    for hit in grep(['-l', '-P', r'\bstruct ' + re.escape(name) + r'\b'], ['crates']):
        text = (ROOT / hit).read_text()
        for match in pattern.finditer(text):
            depth, i = 1, match.end()
            while depth and i < len(text):
                depth += {'{': 1, '}': -1}.get(text[i], 0)
                i += 1
            yield hit, text[match.end():i]


def impl_blocks(name):
    pattern = re.compile(r'\bimpl(?:<[^>]*>)? ' + re.escape(name) + r'\s*\{')
    for hit in grep(['-l', '-P', r'\bimpl(<[^>]*>)? ' + re.escape(name) + r'\b'], ['crates']):
        text = (ROOT / hit).read_text()
        for match in pattern.finditer(text):
            depth, i = 1, match.end()
            while depth and i < len(text):
                depth += {'{': 1, '}': -1}.get(text[i], 0)
                i += 1
            yield hit, text[match.end():i]


def survivors(kind, spec):
    """Where an entry still holds, as `file[:line]` strings."""
    if kind == 'path':
        return [spec] if (ROOT / spec).exists() else []
    if kind == 'word':
        return [h.split(':', 2)[0] + ':' + h.split(':', 2)[1] for h in grep(['-w', '-F', spec], ['crates', 'tools'])]
    if kind == 'text':
        text, path = spec
        return [h.split(':', 2)[0] + ':' + h.split(':', 2)[1] for h in grep(['-F', text], [path])]
    if kind == 'field':
        struct, field = spec
        field_re = re.compile(r'^\s*(?:pub(?:\([^)]*\))?\s+)?' + re.escape(field) + r'\s*:', re.M)
        return [f'{file}: {struct}.{field}' for file, body in struct_blocks(struct) if field_re.search(body)]
    if kind == 'method':
        type_, method = spec
        method_re = re.compile(r'\bfn ' + re.escape(method) + r'\b')
        return [f'{file}: {type_}::{method}' for file, body in impl_blocks(type_) if method_re.search(body)]
    raise ValueError(kind)


def main():
    present = sys.argv[1:] == ['--present']
    if sys.argv[1:] not in ([], ['--present']):
        print(__doc__, file=sys.stderr)
        return 2
    failed = 0
    manual = []
    for lane, item, kind, spec in ENTRIES:
        if kind == 'manual':
            manual.append((lane, item))
            continue
        found = survivors(kind, spec)
        if present and not found:
            failed += 1
            print(f'NOT FOUND  {lane}: {item} ({kind} {spec})')
        elif not present and found:
            failed += 1
            shown = ', '.join(found[:3]) + (f' (+{len(found) - 3} more)' if len(found) > 3 else '')
            print(f'SURVIVES   {lane}: {item}: {shown}')
    for lane, item in manual:
        print(f'MANUAL     {lane}: {item}')
    checked = sum(kind != 'manual' for _, _, kind, _ in ENTRIES)
    noun = 'missing from this tree' if present else 'surviving'
    print(f'arch_deletions: {failed} of {checked} mechanical entries {noun}; {len(manual)} manual')
    return 1 if failed else 0


if __name__ == '__main__':
    sys.exit(main())
