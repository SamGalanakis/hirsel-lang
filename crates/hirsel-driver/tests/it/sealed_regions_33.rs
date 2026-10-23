//! MEM-4 source grant controls and reachable independent V validation.
use hirsel_driver::*;
use hirsel_engine::*;

fn check(source: &str, expected: Option<&str>, export: bool) {
    let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
    let file = engine
        .sources
        .add("sealed-regions-33.hir", source.into())
        .unwrap();
    let output = check_progress(&mut engine, file, &default_context()).unwrap();
    let errors: Vec<_> = output
        .diagnostics
        .iter()
        .filter(|d| d.severity == hirsel_diag::Severity::Error)
        .collect();
    if let Some(rule) = expected {
        assert!(
            errors.iter().any(|d| d.rule.as_str() == rule),
            "{source}\n{errors:?}"
        );
    } else {
        assert!(errors.is_empty(), "{source}\n{errors:?}");
        if export {
            let input = engine.export_native_input().unwrap();
            engine.import_native_input(input).unwrap();
        }
    }
}

#[test]
fn sealed_regions_33_contextual_scalar_grant() {
    with_compiler_stack(|| {
        check(
            "pub fn main(){let r = arena.with(fn(a: Arena)->Region[u64]{let root=7u64;arena.seal(take a,take root)});}",
            None,
            true,
        )
    });
}

#[test]
fn sealed_regions_33_borrow_only_callback_preserved() {
    with_compiler_stack(|| {
        check(
            "fn read(a:Arena)->u64{7u64}\npub fn main(){let n=arena.with(read);}",
            None,
            true,
        )
    });
}

#[test]
fn sealed_regions_33_ordinary_borrow_is_not_a_grant() {
    with_compiler_stack(|| {
        check(
            "fn bad(a:Arena)->Region[u64]{let root=7u64;arena.seal(take a,take root)}\npub fn main(){}",
            Some("OWN-1"),
            false,
        )
    });
}

#[test]
fn sealed_regions_33_consumed_grant_stays_dead() {
    with_compiler_stack(|| {
        check(
            "pub fn main(){arena.with(fn(a:Arena){let root=7u64;let r=arena.seal(take a,take root);let xs=copy_to(a,[1u64]);});}",
            Some("OWN-2"),
            false,
        )
    });
}

#[test]
fn sealed_regions_33_grant_forwards_through_take_helper() {
    with_compiler_stack(|| {
        check(
            "fn seal(take a:Arena in a)->Region[u64]{let root=7u64;arena.seal(take a,take root)}\npub fn main(){let r=arena.with(fn(a:Arena)->Region[u64]{seal(take a)});}",
            None,
            true,
        )
    });
}

#[test]
fn sealed_regions_33_root_and_alias_source_controls() {
    with_compiler_stack(|| {
        for root in [
            "let root: List[u64] in heap=[7u64];",
            "let root: List[u64] in a=[7u64];",
        ] {
            let source = format!(
                "pub fn main(){{let r=arena.with(fn(a:Arena)->Region[List[u64]]{{{root}arena.seal(take a,take root)}});}}"
            );
            check(&source, None, true);
        }
        check(
            "pub fn main(){let r=arena.with(fn(a:Arena)->Region[List[u64]]{let root:List[u64] in a=[7u64];let alias=copy root;let r=arena.seal(take a,take root);let n=list.len(alias);r});}",
            Some("MEM-4"),
            false,
        );
        check(
            "pub fn main(){let r=arena.with(fn(a:Arena)->Region[List[u64]]{let root:List[u64] in a=[7u64];let alias=copy root;let n=list.len(alias);arena.seal(take a,take root)});}",
            None,
            true,
        );
        check(
            "pub fn main(){let root=7u64;let r=arena.seal(heap,take root);}",
            Some("MEM-4"),
            false,
        );
    });
}

#[test]
fn sealed_regions_33_region_read_source_controls() {
    with_compiler_stack(|| {
        check(
            "pub fn main(){let mut r=arena.with(fn(a:Arena)->Region[List[u64]]{let root:List[u64] in heap=[7u64];arena.seal(take a,take root)});let n=region.with(mut r,fn(root:List[u64])->u64{list.len(root)});}",
            None,
            true,
        );
        check(
            "pub fn main(){let mut r=arena.with(fn(a:Arena)->Region[List[u64]]{let root:List[u64] in heap=[7u64];arena.seal(take a,take root)});let escaped=region.with(mut r,fn(root:List[u64])->List[u64]{root});}",
            Some("MODE-1"),
            false,
        );
    });
}
