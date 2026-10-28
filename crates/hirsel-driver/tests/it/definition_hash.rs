//! LANG-6 definition hashes: renames keep every hash, a body edit moves its
//! own hash and every transitive dependent's, recursive groups hash without
//! names, and identical definitions share a hash wherever they live.
use hirsel_driver::*;
use hirsel_engine::*;
use hirsel_proof_export::{CallSiteKind, DefinitionContent};
use hirsel_syntax::ContentHash;
use hirsel_types::SignatureKind;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

fn named(name: &str) -> DefPath {
    let mut path = entry_path(&default_context());
    path.0.name = name.into();
    path
}

fn engine(files: &[(&str, &[&str], &str)]) -> QueryEngine {
    let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
    for (file, modules, text) in files {
        let context = FileContext {
            modules: modules.iter().map(|m| m.to_string()).collect(),
            ..default_context()
        };
        let file = engine.sources.add(*file, (*text).into()).unwrap();
        engine.parse_file(file, &context).unwrap();
    }
    engine
}

/// The hashes of `names` in the one-file program `source`.
fn hashes(source: &str, names: &[&str]) -> Vec<ContentHash> {
    let mut engine = engine(&[("program.hir", &[], source)]);
    let roots: Vec<_> = names.iter().map(|n| named(n)).collect();
    let all = engine
        .definition_hashes(roots.clone())
        .unwrap_or_else(|e| panic!("{source}\n{e:?}"))
        .hashes;
    roots.iter().map(|r| all[r]).collect()
}

const CHAIN: &str = "fn leaf(x: u64) -> u64 {\n  let doubled = x * 2u64\n  doubled + 1u64\n}\nfn mid(n: u64) -> u64 { leaf(n) + leaf(1u64) }\nfn other(n: u64) -> u64 { n }\npub fn main() -> Result[(), ()] {\n  let n = mid(3u64) + other(1u64)\n  Ok(())\n}\n";

#[test]
fn renaming_a_definition_a_parameter_or_a_local_keeps_its_hash_and_its_dependents() {
    with_compiler_stack(|| {
        let base = hashes(CHAIN, &["leaf", "mid", "main"]);
        let renamed = CHAIN
            .replace("leaf", "twice")
            .replace("doubled", "d")
            .replace("x: u64", "value: u64")
            .replace("x * 2u64", "value * 2u64")
            .replace("mid", "middle");
        assert_eq!(base, hashes(&renamed, &["twice", "middle", "main"]));
        // Moving definitions and editing comments and layout changes nothing.
        let moved = format!(
            "// leading comment\n\n{}",
            CHAIN.replace("fn other(n: u64) -> u64 { n }\n", "")
        ) + "fn other(n: u64) -> u64 {\n  n\n}\n";
        assert_eq!(base, hashes(&moved, &["leaf", "mid", "main"]));
    });
}

#[test]
fn a_body_edit_changes_its_hash_and_every_transitive_dependent_only() {
    with_compiler_stack(|| {
        let names = ["leaf", "mid", "main", "other"];
        let base = hashes(CHAIN, &names);
        let edited = hashes(&CHAIN.replace("doubled + 1u64", "doubled + 2u64"), &names);
        for (i, name) in names[..3].iter().enumerate() {
            assert_ne!(base[i], edited[i], "{name}");
        }
        assert_eq!(base[3], edited[3], "other does not reach leaf");
        // A signature edit is a content edit too.
        let signature = hashes(
            &CHAIN.replace("fn other(n: u64)", "fn other(take n: u64)"),
            &names,
        );
        assert_eq!(base[0], signature[0]);
        assert_ne!(base[3], signature[3]);
        assert_ne!(base[2], signature[2]);
    });
}

const EVEN_ODD: &str = "fn even(n: u64) -> bool { if n == 0u64 { true } else { odd(n - 1u64) } }\nfn odd(n: u64) -> bool { if n == 0u64 { false } else { even(n - 1u64) } }\npub fn main() -> Result[(), ()] {\n  let b = even(4u64)\n  Ok(())\n}\n";

#[test]
fn mutual_recursion_hashes_without_names_or_declaration_order() {
    with_compiler_stack(|| {
        let names = ["even", "odd", "main"];
        let base = hashes(EVEN_ODD, &names);
        assert_ne!(base[0], base[1], "the members' bodies differ");
        let renamed = EVEN_ODD.replace("even", "is_even").replace("odd", "is_odd");
        assert_eq!(base, hashes(&renamed, &["is_even", "is_odd", "main"]));
        let mut lines: Vec<_> = EVEN_ODD.lines().collect();
        lines.swap(0, 1);
        assert_eq!(base, hashes(&(lines.join("\n") + "\n"), &names));
        // An edit to one member moves the whole group and its dependents.
        let edited = hashes(&EVEN_ODD.replace("{ false }", "{ n == 7u64 }"), &names);
        for i in 0..3 {
            assert_ne!(base[i], edited[i], "{}", names[i]);
        }
        // Self recursion is a group of one.
        let fact = "fn fact(n: u64) -> u64 { if n == 0u64 { 1u64 } else { n * fact(n - 1u64) } }\n";
        assert_eq!(
            hashes(fact, &["fact"]),
            hashes(&fact.replace("fact", "factorial"), &["factorial"])
        );
        assert_ne!(
            hashes(fact, &["fact"]),
            hashes(&fact.replace("{ 1u64 }", "{ 2u64 }"), &["fact"])
        );
    });
}

#[test]
fn identical_definitions_in_different_modules_share_a_hash() {
    with_compiler_stack(|| {
        let helper = "pub fn helper(n: u64) -> u64 {\n  let m = n + 1u64\n  m * m\n}\n";
        let renamed = "pub fn square_next(k: u64) -> u64 { let j = k + 1u64; j * j }\n";
        let mut engine = engine(&[
            (
                "program.hir",
                &[],
                "pub fn main() -> Result[(), ()] { Ok(()) }\n",
            ),
            ("first.hir", &["first"], helper),
            ("second.hir", &["second"], renamed),
        ]);
        let in_module = |module: &str, name: &str| {
            let mut path = named(name);
            path.0.modules = vec![module.into()];
            path
        };
        let (first, second) = (
            in_module("first", "helper"),
            in_module("second", "square_next"),
        );
        let all = engine
            .definition_hashes([first.clone(), second.clone()])
            .unwrap()
            .hashes;
        assert_eq!(all[&first], all[&second]);
    });
}

#[test]
fn the_hash_is_a_pinned_function_of_content() {
    with_compiler_stack(|| {
        // No HashMap order, arena id, path or position enters: the same
        // content hashes to this value on every machine and build. It moves
        // with TIR_VERSION, the content encoding, or an edit to anything `f`
        // reaches. Nominal role names add no core declaration dependency here.
        // 1d696836 intentionally moved TIR 9 -> 10 and added Body.borrow_cursors:
        // even this scalar body's empty cursor list enters the canonical text.
        // Undoing both changes reproduces the previous 4f584c19... pin.
        // 2a938017 bumped TIR 10 -> 11; 4a539389 bumped it to 12 for invocation proofs.
        // b664ed74 and 4aecabbc added private uniqueness headers in TIR 14/15;
        // ca2c887f moved TIR to 16. Those versions enter this hash preimage.
        // cefa8ea6 added the third, nonretaining UniqueSummary list to the
        // content encoding without changing TIR 16. Removing that list from
        // both summaries and restoring the scalar result Root reproduces
        // def13697. Restoring Root alone reproduces 357dddcc; 589e59d4
        // deliberately removed that inert scalar proof under MODE-2.
        const PINNED: &str = "f2eef0b90be6b80e06240bf2e0438fc2b963c53f65db5da1a7955d1d53daadaf";
        let hash = hashes("fn f(x: u64) -> u64 { x + 1u64 }\n", &["f"])[0];
        let text: String = hash.0.iter().map(|b| format!("{b:02x}")).collect();
        assert!(
            text == PINNED,
            "the definition hash of `fn f(x: u64) -> u64 {{ x + 1u64 }}` is {text}, pinned {PINNED}.\n\
             A moved definition hash moves every stored code identity: runner calls, \
             code-store entries and upgrade targets recorded under the old hash no longer \
             match. Re-pin only for an intended change (TIR_VERSION, the LANG-6 content \
             encoding, or an edit to a definition `f` reaches, such as a core role type) \
             and say so in the commit: set PINNED to {text} in \
             crates/hirsel-driver/tests/it/definition_hash.rs. Any other move is a \
             determinism bug in the hash preimage."
        );
    });
}

const CORE_USERS: &str = "fn f(x: u64) -> u64 { x + 1u64 }
fn half(x: u64) -> Result[u64, ()] {
  if x == 0u64 { Error(()) } else { Ok(x / 2u64) }
}
fn tried(x: u64) -> Result[u64, ()] {
  let v = half(x)?
  Ok(v + 1u64)
}
fn matched(x: u64) -> u64 {
  match half(x) {
    Ok(v) => v,
    Error(_) => 0u64,
  }
}
pub fn launch(tasks: Tasks) -> () {
  task.scope(
    tasks,
    fn(s: Scope) {
      ()
    }
  )
}
";

fn core_names(paths: impl IntoIterator<Item = DefPath>) -> BTreeSet<String> {
    paths
        .into_iter()
        .filter(|p| p.0.package.name == "core")
        .map(|p| p.0.name.to_string())
        .collect()
}

#[test]
fn a_definition_reaches_only_the_core_types_its_signature_and_tir_name() {
    with_compiler_stack(|| {
        let mut engine = engine(&[("program.hir", &[], CORE_USERS)]);
        let mut reached = |name: &str| {
            let hashes = engine.definition_hashes([named(name)]).unwrap().hashes;
            core_names(hashes.into_keys())
        };
        // Every signature carries the core role table; it adds no dependency.
        assert_eq!(reached("f"), BTreeSet::new());
        for user in ["tried", "matched"] {
            assert!(reached(user).contains("Result"), "{user}");
        }
        let launch = reached("launch");
        assert!(
            launch.contains("Tasks") && launch.contains("Scope"),
            "{launch:?}"
        );
    });
}

/// The closure of `roots` as the engine hashes it: a program definition by its
/// signature and TIR, a core one by its path and signature.
fn closure(engine: &mut QueryEngine, roots: &[DefPath]) -> BTreeMap<DefPath, DefinitionContent> {
    let mut contents = BTreeMap::new();
    let mut pending = roots.to_vec();
    while let Some(path) = pending.pop() {
        if contents.contains_key(&path) {
            continue;
        }
        let signature = DefinitionQueries::item_signature(engine, &path)
            .unwrap()
            .value;
        let content = if path.0.package.name == "core" {
            DefinitionContent::Named {
                signature,
                source: vec![],
                reads: vec![],
            }
        } else {
            let tir = DefinitionQueries::lower_to_tir(engine, &path)
                .unwrap()
                .value;
            DefinitionContent::Body {
                signature,
                tir: Some(tir),
            }
        };
        pending.extend(hirsel_proof_export::references(engine.types(), &content));
        contents.insert(path, content);
    }
    contents
}

#[test]
fn an_edit_to_a_core_type_moves_the_definitions_that_use_it_and_no_other() {
    with_compiler_stack(|| {
        let mut engine = engine(&[("program.hir", &[], CORE_USERS)]);
        let roots: Vec<_> = ["f", "tried", "matched", "launch"].map(named).to_vec();
        let contents = closure(&mut engine, &roots);
        let base = hirsel_proof_export::definition_hashes(engine.types(), &contents).unwrap();
        for (core, users) in [
            ("Result", &["tried", "matched"][..]),
            ("Scope", &["launch"]),
            ("Tasks", &["launch"]),
        ] {
            // c302948e's edit: the type becomes serializable.
            let mut edited = contents.clone();
            let (_, content) = edited
                .iter_mut()
                .find(|(p, _)| p.0.package.name == "core" && p.0.name == core)
                .unwrap_or_else(|| panic!("{core} is reached"));
            let DefinitionContent::Named { signature, .. } = content else {
                panic!("{core} is a core definition");
            };
            let mut signature_edit = (**signature).clone();
            let SignatureKind::Type(t) = &mut signature_edit.kind else {
                panic!("{core} is a type");
            };
            t.serializable = !t.serializable;
            *signature = Arc::new(signature_edit);
            let moved = hirsel_proof_export::definition_hashes(engine.types(), &edited).unwrap();
            for user in users {
                assert_ne!(base[&named(user)], moved[&named(user)], "{core}: {user}");
            }
            assert_eq!(base[&named("f")], moved[&named("f")], "{core}: f");
        }
    });
}

#[test]
fn call_sites_record_each_callee_hash() {
    with_compiler_stack(|| {
        let source = "fn helper(n: u64) -> u64 { n }\npub fn main() -> Result[(), ()] {\n  let n = helper(1u64)\n  let f = helper\n  let m = f(2u64)\n  Ok(())\n}\n";
        let mut engine = engine(&[("program.hir", &[], source)]);
        let main = named("main");
        let result = engine.definition_hashes([main.clone()]).unwrap();
        let helper = result.hashes[&named("helper")];
        let kinds: BTreeMap<_, _> = result.call_sites[&main]
            .iter()
            .filter(|site| site.callee == named("helper"))
            .map(|site| {
                assert_eq!(site.hash, helper);
                (matches!(site.kind, CallSiteKind::Call(_)), site.hash)
            })
            .collect();
        assert_eq!(kinds.len(), 2, "{:?}", result.call_sites[&main]);
    });
}

/// PS-19: an analysis annotation is not part of a definition's meaning.
#[test]
fn analysis_annotations_move_no_definition_hash() {
    with_compiler_stack(|| {
        use super::proof_export::{ANNOTATED, blanked};
        let names = ["speed", "Point", "Shape", "Meters", "main"];
        assert_eq!(
            hashes(ANNOTATED, &names),
            hashes(&blanked(ANNOTATED), &names)
        );
    });
}

#[test]
fn a_recursive_type_group_hashes_its_meaning_not_its_text() {
    with_compiler_stack(|| {
        let base = "type Tree = Leaf | Node(List[Entry])\n\ntype Entry {\n  key: u64,\n  children: Tree,\n}\nfn size(t: Tree) -> u64 { 0u64 }\n";
        let names = ["Tree", "Entry", "size"];
        let before = hashes(base, &names);
        // Layout, a comment and a rename change no member's hash or a user's.
        assert_eq!(before, hashes(&format!("\n  {base}"), &names));
        assert_eq!(
            before,
            hashes(
                &base.replacen("type Entry", "/* c */ type Entry", 1),
                &names
            )
        );
        assert_eq!(
            before,
            hashes(&base.replace("Entry", "Item"), &["Tree", "Item", "size"])
        );
        // A member's edit moves the group and its user.
        let edited = hashes(&base.replacen("key: u64", "key: u32", 1), &names);
        assert!(before.iter().zip(&edited).all(|(a, b)| a != b));
    });
}
