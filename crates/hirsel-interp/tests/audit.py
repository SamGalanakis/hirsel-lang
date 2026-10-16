#!/usr/bin/env python3
"""Run fixture tests with strict assert_valid, using a read-only validator source.

--validator-root names a repository whose crates/hirsel-validate is used instead
of this one's. No shared crate or worktree is modified.
"""
import argparse
from pathlib import Path
import shutil
import subprocess
import tempfile

repo = Path(__file__).resolve().parents[3]
parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--validator-root', type=Path, default=repo)
args = parser.parse_args()
validator = args.validator_root.resolve() / 'crates/hirsel-validate'
if not (validator / 'Cargo.toml').is_file():
    parser.error(f'no validator crate at {validator}')
files = ['aggregates', 'callbacks', 'execution', 'operation_mutation', 'patterns', 'structural']
with tempfile.TemporaryDirectory(prefix='hirsel-fixture-audit-') as directory:
    target = Path(directory)
    (target / 'src').mkdir()
    dependencies = ['hirsel-interp', 'hirsel-tir', 'hirsel-types', 'hirsel-syntax',
                    'hirsel-span', 'hirsel-diag']
    (target / 'Cargo.toml').write_text(
        '[package]\nname="fixture-audit"\nversion="0.0.0"\nedition="2024"\n'
        '[dependencies]\n' + ''.join(
            f'{name}={{path="{repo / "crates" / name}"}}\n' for name in dependencies)
        + f'hirsel-validate={{path="{validator}"}}\n')
    (target / 'src/lib.rs').write_text(
        'pub use hirsel_tir::*;\npub use hirsel_validate as validate;\n')
    tests = target / 'tests'
    tests.mkdir()
    shutil.copytree(repo / 'crates/hirsel-interp/tests/support', tests / 'support')
    for name in files:
        source = (repo / f'crates/hirsel-interp/tests/it/{name}.rs').read_text()
        source = source.replace('use crate::support;', 'mod support;')
        (tests / f'{name}.rs').write_text(source)
    support = tests / 'support/mod.rs'
    source = support.read_text().replace('        (|| {',
        '        if self.queries.is_empty() { validation::audit(&self.definitions); }\n        (|| {')
    support.write_text(source + '\nmod validation;\n')
    (tests / 'support/validation.rs').write_text(r'''
use super::*;
use fixture_audit::validate::{self, ViolationKind};
use hirsel_interp::aggregate::{RuntimeTypes, Nominal};
fn audit(definitions: &[Definition]) {
    let thread = std::thread::current();
    let name = thread.name().unwrap_or("unknown");
    // These are fixture declarations, not classifications guessed by the VM.
    // Fixture finalizers have distinct *_drop paths, as supplied to RuntimeTypes.
    let mut runtime = RuntimeTypes {
        nominals: definitions.iter().filter_map(|d|
            d.path.0.name.strip_suffix("_drop").map(|n| Nominal {
                definition:path(n), resource:true, distinct:false,
                finalizer:Some((d.path.clone(),BodyId(0))), fields:0,
            })).collect(),
        ..RuntimeTypes::default()
    };
    if name == "resource_copy_is_a_compiler_error" {
        runtime.nominals.push(Nominal {definition:path("A"),resource:true,
            distinct:false,finalizer:None,fields:0});
    }
    let signatures = fixture_signatures(definitions,&runtime);
    for definition in definitions {
        let report = validate::validate(definition,&signatures);
        let intentionally_invalid = name == "operation_updates_only_mut_argument_places"
            || name == "resource_copy_is_a_compiler_error";
        let depth_sentinel = name == "depth_failure_occurs_after_entry_and_cleans_transferred_arguments"
            && definition.path == path("callee");
        if intentionally_invalid || depth_sentinel {
            for violation in &report.violations {
                assert!(if depth_sentinel {
                    matches!(violation.kind,ViolationKind::ReachableUnreachable)
                } else {
                    matches!(violation.kind,ViolationKind::InvalidUse(_) |
                        ViolationKind::DoubleMove(_) | ViolationKind::BorrowedMove(_))
                }, "unexpected negative fixture defect {name}: {report:?}");
            }
            if depth_sentinel || name == "resource_copy_is_a_compiler_error" {
                assert!(!report.violations.is_empty(), "negative fixture was not rejected");
            }
        } else {
            validate::assert_valid(definition,&signatures);
        }
    }
}
'''.replace('fn audit(', 'pub fn audit('))
    command = ['cargo', 'test', '--manifest-path', str(target / 'Cargo.toml')]
    for name in files:
        command.extend(['--test', name])
    command.extend(['--', '--test-threads=1'])
    subprocess.run(command, check=True, cwd=repo)
