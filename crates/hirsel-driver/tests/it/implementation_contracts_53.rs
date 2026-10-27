//! GEN-4/GEN-6: an implementation binder has no runtime operand. At
//! optimization zero, each specialization `f$with$H=impl` calls its
//! implementation's targets directly on LLVM and Cranelift, as MLton's
//! defunctorizer leaves only first-order code (defunctorize/defunctorize.sig).
use hirsel_driver::compiler_adapters;
use hirsel_driver::native::{OptLevel, Options, TargetSpec, build, emit_clif, emit_ir};
use hirsel_engine::{
    QueryEngine, check_progress, default_context, default_environment, with_compiler_stack,
};
use hirsel_lir::emission::symbol_name;

const PROGRAM: &str = "signature Source {
  type Item: plain
  size width
  fn make() -> Item uses {}
}
signature Sink {
  type Item: plain
  fn weigh(x: Item) -> u64 uses {}
}
fn make_u64() -> u64 uses {} { 5 }
fn weigh_u64(x: u64) -> u64 uses {} { x * 2 }
implementation five: Source {
  type Item = u64
  size width = 8
  make = make_u64
}
implementation double: Sink {
  type Item = u64
  weigh = weigh_u64
}
fn total[A: implementation Source, B: implementation Sink where B.Item = A.Item](n: u64) -> u64 uses {} {
  if n == 0u64 { A.width } else { B.weigh(A.make()) + total with (A = A, B = B)(n - 1u64) }
}
pub fn main(rt: Io, args: Args) -> Result[(), ()] {
  failure.check(total with (A = five, B = double)(3u64) == 38u64, TextInvariant())
  Ok(())
}
";

/// The program's LLVM IR at optimization zero, its Cranelift IR, and each of
/// its own units' definition name and symbol.
struct Compiled {
    ir: String,
    clif: String,
    units: Vec<(String, String)>,
}

fn compile() -> Compiled {
    with_compiler_stack(|| {
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
        let file = engine.sources.add("contracts.hir", PROGRAM.into()).unwrap();
        let output = check_progress(&mut engine, file, &default_context()).unwrap();
        assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
        let input = engine.export_native_input().unwrap();
        let validated = engine.import_native_input(input).unwrap();
        let ready = build(
            &validated,
            &Options::new(TargetSpec::x86_64_unknown_linux_gnu()),
        )
        .unwrap();
        let units = ready
            .units
            .iter()
            .map(|u| u.unit())
            .filter(|u| u.header.definition.0.package.name == "standalone")
            .map(|u| {
                (
                    u.header.definition.0.name.clone(),
                    symbol_name(&u.header.definition, Some(&u.header.instance)),
                )
            })
            .collect();
        Compiled {
            ir: emit_ir(&ready, OptLevel::None).unwrap(),
            clif: emit_clif(&ready).unwrap(),
            units,
        }
    })
}

/// The text of the first function whose header line `header` accepts, up to
/// the line that closes it.
fn body<'a>(text: &'a str, header: impl Fn(&str) -> bool) -> Option<&'a str> {
    let mut offset = 0;
    for line in text.split_inclusive('\n') {
        if header(line) {
            let rest = &text[offset..];
            let end = rest.find("\n}").map_or(rest.len(), |e| e + 2);
            return Some(&rest[..end]);
        }
        offset += line.len();
    }
    None
}

#[test]
fn specializations_call_their_targets_directly_at_optimization_zero() {
    let compiled = compile();
    let specialization = |name: &str| name.starts_with("total$with$");
    let specialized: Vec<_> = compiled
        .units
        .iter()
        .filter(|(name, _)| specialization(name))
        .collect();
    assert_eq!(specialized.len(), 1, "{:?}", compiled.units);
    // The abstract declaration has no code of its own.
    assert!(compiled.units.iter().all(|(name, _)| name != "total"));
    let weigh = &compiled
        .units
        .iter()
        .find(|(name, _)| name == "weigh_u64")
        .unwrap()
        .1;
    for (name, symbol) in &compiled.units {
        let ir = body(&compiled.ir, |l| {
            l.starts_with("define") && l.contains(&format!("@{symbol}("))
        })
        .unwrap_or_else(|| panic!("{name} is defined\n{}", compiled.ir));
        let indirect: Vec<_> = ir
            .lines()
            .filter(|l| {
                l.split_once("call ")
                    .and_then(|(_, rest)| rest.split_once('('))
                    .is_some_and(|(callee, _)| callee.contains(" %"))
            })
            .collect();
        assert!(indirect.is_empty(), "{name}: {indirect:?}\n{ir}");
        if specialization(name) {
            assert!(ir.contains(&format!("@{weigh}(")), "{ir}");
            // Same-map recursion calls the specialization itself.
            assert!(ir.contains(&format!("@{symbol}(")), "{ir}");
        }
    }
    // Cranelift: no `call_indirect` anywhere in the program's own functions.
    for (name, symbol) in &compiled.units {
        let clif = body(&compiled.clif, |l| l.trim_end() == format!("; {symbol}"))
            .unwrap_or_else(|| panic!("{name} is in CLIF\n{}", compiled.clif));
        assert!(!clif.contains("call_indirect"), "{name}\n{clif}");
    }
}

/// Declarations only: materializing `total`'s specialization is the one change.
const DECLARATIONS: &str = "signature Source {
  type Item: plain
  fn make() -> Item uses {}
}
fn make_u64() -> u64 uses {} { 5 }
implementation five: Source {
  type Item = u64
  make = make_u64
}
fn total[A: implementation Source]() -> A.Item uses {} { A.make() }
";

/// The standalone package's validation-input digest after checking `source`,
/// then after materializing `total`'s specialization at `five`, and whether the
/// snapshot then holds the specialization.
fn digests(source: &str) -> (hirsel_syntax::ContentHash, hirsel_syntax::ContentHash, bool) {
    use hirsel_types::SignatureProvider;
    let source = source.to_owned();
    with_compiler_stack(move || {
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
        let file = engine.sources.add("contracts.hir", source).unwrap();
        let output = check_progress(&mut engine, file, &default_context()).unwrap();
        assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
        let package = default_context().package;
        let before = engine
            .signature_snapshot()
            .validation_input_digest(&package);
        let path = |name: &str| {
            hirsel_syntax::DefPath(hirsel_syntax::ItemPath {
                package: package.clone(),
                modules: vec![],
                name: name.into(),
            })
        };
        let (base, five) = (path("total"), path("five"));
        let specialization = hirsel_types::specialization_path(&base, &[("A".into(), five)]);
        let id = hirsel_syntax::DefId::intern(&specialization);
        engine.signature(id).unwrap();
        let snapshot = engine.signature_snapshot();
        let held = matches!(snapshot.signature(id), Some(Ok(_)));
        (before, snapshot.validation_input_digest(&package), held)
    })
}

#[test]
fn a_specialization_leaves_its_package_validation_digest_unchanged() {
    let (before, after, held) = digests(DECLARATIONS);
    // The validator still finds the specialization's signature...
    assert!(held);
    // ...but materializing it in a program does not invalidate every
    // validation of its declaring package.
    assert_eq!(before, after);
    // An ordinary declaration remains part of the package's universe.
    let (other, _, _) = digests(&format!(
        "{DECLARATIONS}fn extra() -> u64 uses {{}} {{ 1 }}\n"
    ));
    assert_ne!(before, other);
}

#[test]
fn a_public_family_can_specialize_a_private_implementation() {
    let source = DECLARATIONS
        .replace("signature Source", "pub signature Source")
        .replace("fn total[", "pub fn total[");
    let (before, after, held) = digests(&source);
    assert!(held);
    assert_eq!(before, after);
}
