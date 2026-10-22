//! Opaque source context with independent V/C and checked lifetime controls.
//! Capture ordering/recursive vectors follow Goose 8511ae34
//! codegen_frames.h:115–154 and funval_body_captures.goose:32–89.
use crate::rc_support::*;
use hirsel_rc::import::{CachedImport, ImportCache, Lowered};
use hirsel_rc::instance::{InstanceId, Instances, Program};
struct Heap;
impl ImportCache for Heap {
    fn enabled(&self) -> bool {
        false
    }
    fn scoped_specialization(&self) -> bool {
        false
    }
    fn get(&mut self, _: &Program, _: &Instances, _: InstanceId) -> Option<CachedImport> {
        None
    }
    fn put(&mut self, _: InstanceId, _: Result<CachedImport, String>) {}
}

fn source(body: &str, declarations: &str) -> String {
    format!(
        "{declarations}\nfn invoke(f:fn()->u64 uses {{}})->u64 uses {{f}}{{f()}}\nfn main()->u64{{let reader=std@1/bufio.new(8u64);let n=std@1/bufio.buffered(reader)+3u64;{body}}}"
    )
}

fn lower(p: &Program, enabled: bool) -> Lowered {
    if enabled {
        hirsel_rc::import::lower(p, &target()).unwrap()
    } else {
        hirsel_rc::import::lower_cached(
            p,
            &target(),
            &Default::default(),
            &Default::default(),
            &mut Heap,
        )
        .unwrap()
    }
}

fn heap(p: &Program, l: Lowered, faults: bool) -> hirsel_heapcheck::Run {
    let rows = manifest(p);
    let target = target();
    let operations = p.operation_registry();
    let inputs = hirsel_lir::validate::Inputs {
        target: &target,
        layouts: &l.table,
        manifest: &rows,
        operations: &operations,
        discharge: None,
        memo: Default::default(),
    };
    hirsel_lir::validate::qualify_scoped_transport(&l.units, &inputs).unwrap();
    let units = l
        .units
        .iter()
        .zip(&l.unit_origins)
        .map(|(u, &origin)| {
            let placed = hirsel_rc::placement::place(
                u.clone(),
                &l.table,
                &hirsel_rc::placement::Options::for_instance(true, &l.instances, origin),
            )
            .unwrap();
            let rc = hirsel_rc::insert::insert(
                placed,
                &l.table,
                &hirsel_rc::insert::Proofs::for_instance(&l.instances, origin),
            )
            .unwrap();
            let errors = hirsel_lir::validate::check_scoped_transport(&rc, &inputs);
            assert!(errors.is_empty(), "{errors:?}");
            rc
        })
        .collect();
    let h = hirsel_heapcheck::Program {
        target,
        layouts: l.table,
        units,
        statics: l.statics,
    };
    if faults {
        let campaign = hirsel_heapcheck::fault_campaign(
            &h,
            &path("main"),
            &[],
            &mut || Box::new(hirsel_heapcheck::NoOperations),
            &Default::default(),
        );
        assert!(
            campaign.first_violation().is_none(),
            "{:?}",
            campaign.first_violation()
        );
        campaign.clean
    } else {
        let run = hirsel_heapcheck::run(
            &h,
            &path("main"),
            &[],
            &mut hirsel_heapcheck::NoOperations,
            &Default::default(),
        );
        assert!(run.result.is_ok(), "{:?}", run.result);
        run
    }
}

fn pair(s: &str, faults: bool) -> (hirsel_heapcheck::Run, hirsel_heapcheck::Run) {
    let p = compile(s);
    let before = heap(&p, lower(&p, false), faults);
    let after = heap(&p, lower(&p, true), faults);
    assert_eq!(
        before.result.as_ref().unwrap().observation,
        after.result.as_ref().unwrap().observation
    );
    assert_eq!(before.counters.allocations, before.counters.frees);
    assert_eq!(after.counters.allocations, after.counters.frees);
    eprintln!(
        "before={:?} after={:?}\n{s}",
        before.counters, after.counters
    );
    (before, after)
}

#[test]
fn scalar_nested_recursive_and_repeated_data() {
    for (body, decls, allocations, answer, faults) in [
        ("invoke(fn()->u64 uses {}{n})", "", 1, 3, true),
        (
            "invoke(fn()->u64 uses {}{invoke(fn()->u64 uses {}{n})})",
            "",
            2,
            3,
            true,
        ),
        (
            "relay(4u64,fn()->u64 uses {}{n})",
            "fn relay(k:u64,f:fn()->u64 uses {})->u64 uses {f}{if k==0u64{f()}else{relay(k-1u64,f)}}",
            1,
            3,
            true,
        ),
        (
            "let mut total=0u64;for i in 0u64..1000u64{total+=invoke(fn()->u64 uses {}{n+i})};total",
            "",
            1000,
            502500,
            false,
        ),
    ] {
        let (before, after) = pair(&source(body, decls), faults);
        assert_eq!(before.counters.allocations, allocations);
        assert_eq!(after.counters.allocations, 0);
        assert_eq!(after.counters.releases, 0);
        assert!(
            matches!(after.result.unwrap().observation.returned,Some(hirsel_heapcheck::Observed::Integer{bits,..}) if bits==answer)
        );
    }
}

#[test]
fn captured_resource_shared_and_failure_cleanup() {
    for d in [0, 1] {
        let s = source(
            &format!("let r=R(n:n);let d={d}u64;later(fn()->u64 uses {{}}{{r.n}},1u64/d)"),
            "resource type R{n:u64} drop(x){let _=x.n}\nfn later(f:fn()->u64 uses {},x:u64)->u64 uses {f}{f()+x}",
        );
        let (before, after) = pair(&s, true);
        assert_eq!(before.counters.allocations, 1);
        assert_eq!(after.counters.allocations, 0);
        assert_eq!(after.counters.finalizers, 1);
    }
    let s = source(
        "let text:String=\"value\";let root:Shared[String]=shared.freeze(take text);invoke(fn()->u64 uses {}{let t=copy root;match shared.thaw(take t){Ok(thawed)=>n,Error(kept)=>n+1u64}})",
        "",
    );
    let (before, after) = pair(&s, true);
    assert_eq!(before.counters.allocations, after.counters.allocations + 1);
    assert_eq!(before.counters.finalizers, after.counters.finalizers);
}

#[test]
fn retained_and_owning_callbacks_keep_heap_fallback() {
    let rejected = try_compile("fn keep(f:fn()->u64 uses {})->fn()->u64 uses {}{f}").unwrap_err();
    assert!(rejected.contains("CLO-1"), "{rejected}");
    for s in [
        source(
            "let f=make(n);f()",
            "fn make(n:u64)->fn()->u64 uses {}{fn()->u64 uses {}{n}}",
        ),
        source(
            "let r=R(n:n);let f:once fn()->u64=fn()->u64{let own=take r;own.n};consume(take f)",
            "resource type R{n:u64} drop(x){let _=x.n}\nfn consume(take f:once fn()->u64)->u64 uses {f}{f()}",
        ),
    ] {
        let p = compile(&s);
        let before = lower(&p, false);
        let after = lower(&p, true);
        assert_eq!(
            before.units.iter().map(|u| u.digest()).collect::<Vec<_>>(),
            after.units.iter().map(|u| u.digest()).collect::<Vec<_>>()
        );
        assert_eq!(before.table.digest(), after.table.digest());
        let (before, after) = pair(&s, true);
        assert_eq!(before.counters.allocations, after.counters.allocations);
        assert_eq!(before.counters.finalizers, after.counters.finalizers);
    }
}

#[test]
fn missing_owner_context_fails_closed_before_rewrite() {
    let p = compile(&source("invoke(fn()->u64 uses {}{n})", ""));
    let id = *p
        .owners
        .nominals
        .keys()
        .find(|id| id.path().0.name == "Reader")
        .unwrap();
    let mut incomplete = p.clone();
    incomplete.owners.nominals.remove(&id);
    assert!(hirsel_rc::import::lower(&incomplete, &target()).is_err());
    let before = lower(&p, false);
    let after = lower(&p, true);
    assert!(after.units.len() > before.units.len());
}

#[test]
fn independent_v_requires_shape_and_rejects_forged_or_client_projection() {
    let p = compile(&source("invoke(fn()->u64 uses {}{n})", ""));
    let mut public = hirsel_validate::SignatureMap::new(p.types.clone());
    for (path, s) in &p.signatures {
        public.insert(hirsel_syntax::DefId::intern(path), (**s).clone());
    }
    let buffered = p
        .definitions
        .values()
        .find(|d| d.path.path().0.name == "buffered")
        .unwrap();
    let check = |d: &hirsel_tir::Definition, s: &hirsel_validate::SignatureMap| {
        hirsel_validate::qualify_scoped_transport(d, &p.types, s, &mut vec![]).violations
    };
    assert!(!check(buffered, &public).is_empty());
    let mut owner = public.clone();
    for (id, record) in &p.owners.nominals {
        if let hirsel_layout::layout::OwnerRepresentation::Defined(record) = record
            && let Some(s) = owner.get_mut(*id)
            && let hirsel_types::SignatureKind::Type(d) = &mut s.kind
        {
            d.representation = Some(record.private_shape);
        }
    }
    assert!(check(buffered, &owner).is_empty());
    let mut forged = (**buffered).clone();
    let place = forged.bodies[0]
        .places
        .iter_mut()
        .find(|p| !p.projection.is_empty())
        .unwrap();
    place.projection = vec![hirsel_tir::Projection::Field(999)].into();
    assert!(!check(&forged, &owner).is_empty());
    let mut client = (**buffered).clone();
    let mut signature = owner.get(buffered.path).unwrap().clone();
    client.path = hirsel_syntax::DefId::intern(&path("forged"));
    signature.definition = client.path;
    owner.insert(client.path, signature);
    assert!(!check(&client, &owner).is_empty());
}

#[test]
fn opaque_context_native_both_backends() {
    use hirsel_driver::native::*;
    hirsel_engine::with_compiler_stack(|| {
        let source = "fn invoke(f:fn()->u64 uses {})->u64 uses {f}{f()}\nfn value(n:u64)->u64{let reader=std@1/bufio.new(8u64);let x=std@1/bufio.buffered(reader)+n;let mut total=0u64;for i in 0u64..20u64{total+=invoke(fn()->u64 uses {}{invoke(fn()->u64 uses {}{x+i})})};total}\npub fn main()->Result[(),u64]{Error(value(3u64))}";
        let source_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../hirsel-driver/tests/fixtures/std_numeric.hir");
        let mut loaded = hirsel_driver::load_program_source(&source_path, Some(source)).unwrap();
        loaded.check_for_execution().unwrap();
        let raw = loaded.engine.export_native_input().unwrap();
        let validated = loaded.engine.import_native_input(raw).unwrap();
        let sites = Sites::new(validated.snapshot());
        let entry = hirsel_syntax::DefPath(hirsel_syntax::ItemPath {
            package: loaded.entry.0.package.clone(),
            modules: vec![],
            name: "main".into(),
        });
        let mut options = Options::new(target());
        options.validation = Validation::EveryStage;
        let ready = build(&validated, &options).unwrap();
        let h = heap_campaign(&ready, &entry, &sites).unwrap();
        assert_eq!(h.counters.allocations, 0);
        let root = std::path::Path::new(env!("CARGO_TARGET_TMPDIR"))
            .join("scoped-proof-context-17-native");
        let toolchain =
            Toolchain::in_tree_diagnostic(&root.join("runtime"), &[&ready.target]).unwrap();
        for backend in [Backend::Llvm, Backend::Cranelift] {
            let dir = root.join(backend.name());
            std::fs::create_dir_all(&dir).unwrap();
            let objects = emit_backend_objects(
                &ready,
                backend,
                OptLevel::Speed,
                Partition::Whole,
                false,
                None,
            )
            .unwrap();
            let executable = link_objects(&ready, &entry, &objects, &toolchain, &dir).unwrap();
            for _ in 0..2 {
                let launched = launch(&executable, &ready.target, &toolchain, &dir, true).unwrap();
                assert_eq!(observe(&launched, &ready, &sites).unwrap(), h.observation);
            }
        }
    });
}
