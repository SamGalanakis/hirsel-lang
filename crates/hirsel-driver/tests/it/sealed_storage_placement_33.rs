//! Placement regressions established by the bare-unique and helper witnesses.
use hirsel_driver::*;
use hirsel_engine::*;

fn rejected(source: &str, primary: &str) {
    rejected_by(source, "MODE-1", primary);
}
fn rejected_by(source: &str, rule: &str, primary: &str) {
    with_compiler_stack(|| {
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
        let file = engine
            .sources
            .add("sealed-placement.hir", source.into())
            .unwrap();
        let output = check_progress(&mut engine, file, &default_context()).unwrap();
        assert_eq!(output.completion, CheckCompletion::Complete);
        let errors: Vec<_> = output
            .diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.severity == hirsel_diag::Severity::Error)
            .collect();
        let [diagnostic] = errors.as_slice() else {
            panic!("{source}\n{errors:?}")
        };
        assert_eq!(diagnostic.rule.as_str(), rule, "{source}\n{diagnostic:?}");
        assert_eq!(
            output.sources.slice(diagnostic.primary),
            Some(primary),
            "{diagnostic:?}"
        );
    });
}
fn accepted(source: &str) {
    with_compiler_stack(|| {
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
        let file = engine
            .sources
            .add("sealed-placement.hir", source.into())
            .unwrap();
        let output = check_progress(&mut engine, file, &default_context()).unwrap();
        assert_eq!(output.completion, CheckCompletion::Complete);
        assert!(
            output.diagnostics.is_empty(),
            "{source}\n{:?}",
            output.diagnostics
        );
    });
}
fn closure_refused_by_v(source: &str) {
    with_compiler_stack(|| {
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
        let file = engine
            .sources
            .add("sealed-placement-boundary.hir", source.into())
            .unwrap();
        let error = check_progress(&mut engine, file, &default_context())
            .err()
            .expect("strict V closure refusal");
        assert!(
            matches!(error, QueryError::Operational(ref failure) if failure.operation == "TIR validation" && failure.message.contains("seal storage closure")),
            "{error:?}"
        );
    });
}
fn region(callback: &str, declarations: &str) -> String {
    format!(
        "{declarations}\npub fn main(){{let mut r=arena.with(fn(a:Arena)->Region[List[u64]]{{let root:List[u64] in a=[7u64];arena.seal(take a,take root)}});let escaped=region.with(mut r,{callback});}}"
    )
}

#[test]
fn sealed_storage_placement_33_original69_exact() {
    rejected(
        r###"pub fn main(){let mut r=arena.with(fn(a:Arena)->Region[List[u64]]{let root:List[u64] in a=[7u64];arena.seal(take a,take root)});let escaped=region.with(mut r,fn(root:List[u64])->unique List[u64]{unique root});}"###,
        "unique root",
    );
}
#[test]
fn sealed_storage_placement_33_named77() {
    rejected(
        &region(
            "clone_root",
            "fn clone_root(root:List[u64])->unique List[u64]{unique root}",
        ),
        "unique root",
    );
}
#[test]
fn sealed_storage_placement_33_helper77() {
    rejected(
        &region(
            "fn(root:List[u64])->unique List[u64]{clone_root(root)}",
            "fn clone_root(root:List[u64])->unique List[u64]{unique root}",
        ),
        "unique root",
    );
}
#[test]
fn sealed_storage_placement_33_alias_and_envelope() {
    rejected(
        &region(
            "fn(root:List[u64])->unique List[u64]{let alias=root;let wrapped=Envelope(value:alias,count:7u64);unique wrapped.value}",
            "type Envelope{value:List[u64],count:u64}",
        ),
        "unique wrapped.value",
    );
}
#[test]
fn sealed_storage_placement_33_heap_borrowed_unique() {
    accepted(
        "fn clone_root(root:List[u64])->unique List[u64]{unique root}\npub fn main(){let root:List[u64] in heap=[7u64];let independent=clone_root(root);}",
    );
}
#[test]
fn sealed_storage_placement_33_explicit_heap_copy() {
    accepted(&region(
        "fn(root:List[u64])->List[u64]{copy_to(heap,root)}",
        "",
    ));
}
#[test]
fn sealed_storage_placement_33_named_scalar_callback() {
    accepted(&region(
        "length",
        "fn length(root:List[u64])->u64{list.len(root)}",
    ));
}

#[test]
fn sealed_storage_provenance_33_generic_known_callback() {
    accepted(&region(
        "fn(root:List[u64])->u64{apply(length,root)}",
        "fn length(xs:List[u64])->u64{list.len(xs)}\nfn apply[T](f:fn(T)->u64,x:T)->u64{f(x)}",
    ));
}
#[test]
fn sealed_storage_provenance_33_known_callable_capture() {
    accepted(&region(
        "fn(root:List[u64])->u64{let f=length;let g=fn(xs:List[u64])->u64{f(xs)};g(root)}",
        "fn length(xs:List[u64])->u64{list.len(xs)}",
    ));
}
#[test]
fn sealed_storage_provenance_33_known_sealed_callable_root() {
    accepted(
        "fn held(take payload:Bytes)->fn()->u64{fn()->u64{bytes.len(payload)}}\nfn fits(cap:u64)->u64{let outcome=arena.bounded(cap,fn(a:Arena)->Region[fn()->u64]{let payload:Bytes in heap=copy_to(heap,b\"abc\");let root=held(take payload);arena.seal(take a,take root)});match take outcome{Ok(r)=>{let mut owned=take r;region.with(mut owned,fn(view:fn()->u64)->u64{view()})},Error(e)=>0u64,}}\npub fn main(t:Test)->Result[(),()]{failure.check(fits(10u64)==0u64,Bounds());failure.check(fits(11u64)==3u64,Bounds());Ok(())}",
    );
}

#[test]
fn sealed_storage_provenance_33_mutually_recursive_helpers_retain_placement() {
    rejected(
        &region(
            "fn(root:List[u64])->unique List[u64]{first(root,3u64)}",
            "fn first(root:List[u64],n:u64)->unique List[u64]{if n==0u64{unique root}else{second(root,n-1u64)}}\nfn second(root:List[u64],n:u64)->unique List[u64]{first(root,n)}",
        ),
        "unique root",
    );
}

#[test]
fn sealed_storage_root_allocation_33_captured_payload() {
    rejected(
        "fn held(take payload:Bytes)->fn()->u64{fn()->u64{let fresh=unique payload;bytes.len(fresh)}}\npub fn main(){let mut r=arena.with(fn(a:Arena)->Region[fn()->u64]{let payload:Bytes in heap=copy_to(heap,b\"abc\");let root=held(take payload);arena.seal(take a,take root)});let n=region.with(mut r,fn(view:fn()->u64)->u64{view()});}",
        "unique payload",
    );
}

#[test]
fn sealed_storage_destination_33_explicit_heap_and_inline_envelope() {
    accepted(&region(
        "fn(root:List[u64])->u64{let fresh:List[u64] in heap=[8u64];let wrapped=Envelope(value:fresh,count:9u64);list.len(wrapped.value)}",
        "type Envelope{value:List[u64],count:u64}",
    ));
}

#[test]
fn sealed_storage_nested_producer_33_known_read_capture() {
    accepted(&region(
        "fn(root:List[u64])->u64{let f=fn()->u64{list.len(root)};let g=fn()->u64{f()};g()}",
        "",
    ));
}

#[test]
fn sealed_storage_nested_producer_33_sealed_nested_callable_root() {
    accepted(
        "fn bytes_reader(take payload:Bytes)->fn()->u64{fn()->u64{bytes.len(payload)}}\nfn held(take payload:Bytes)->fn()->u64{let f=bytes_reader(take payload);fn()->u64{f()}}\npub fn main(){let mut r=arena.with(fn(a:Arena)->Region[fn()->u64]{let payload:Bytes in heap=copy_to(heap,b\"abc\");let root=held(take payload);arena.seal(take a,take root)});let n=region.with(mut r,fn(view:fn()->u64)->u64{view()});}",
    );
}

#[test]
fn sealed_storage_nested_producer_33_captured_allocation_retains_origin() {
    rejected(
        &region(
            "fn(root:List[u64])->u64{let f=fn()->u64{let fresh=unique root;list.len(fresh)};let g=fn()->u64{f()};g()}",
            "",
        ),
        "unique root",
    );
}

#[test]
fn sealed_storage_nested_producer_33_reused_wrapper_site() {
    accepted(
        "fn bytes_reader(take payload:Bytes)->fn()->u64{fn()->u64{bytes.len(payload)}}\nfn wrap(take f:fn()->u64)->fn()->u64{fn()->u64{f()}}\nfn held(take payload:Bytes)->fn()->u64{let f=bytes_reader(take payload);let middle=wrap(take f);wrap(take middle)}\npub fn main(){let mut r=arena.with(fn(a:Arena)->Region[fn()->u64]{let payload:Bytes in heap=copy_to(heap,b\"abc\");let root=held(take payload);arena.seal(take a,take root)});let n=region.with(mut r,fn(view:fn()->u64)->u64{view()});}",
    );
}

#[test]
fn sealed_storage_nested_v_33_mutually_recursive_source() {
    accepted(
        "fn leaf(take payload:Bytes)->fn()->u64{fn()->u64{bytes.len(payload)}}\nfn wrap(take f:fn()->u64)->fn()->u64{fn()->u64{f()}}\nfn first(take f:fn()->u64,n:u64)->fn()->u64{if n==0u64{take f}else{second(wrap(take f),n-1u64)}}\nfn second(take f:fn()->u64,n:u64)->fn()->u64{first(take f,n)}\nfn held(take payload:Bytes)->fn()->u64{first(leaf(take payload),3u64)}\npub fn main(){let mut r=arena.with(fn(a:Arena)->Region[fn()->u64]{let payload:Bytes in heap=copy_to(heap,b\"abc\");let root=held(take payload);arena.seal(take a,take root)});let n=region.with(mut r,fn(view:fn()->u64)->u64{view()});}",
    );
}

#[test]
fn sealed_storage_core_callback_33_shared_read_control() {
    accepted(&region(
        "fn(root:List[u64])->u64{let seed:unique Bytes=unique b\"x\";let s=shared.freeze(take seed);shared.with(s,fn(value:Bytes)->u64{list.len(root)+bytes.len(value)})}",
        "",
    ));
}

#[test]
fn sealed_storage_core_callback_33_shared_nested_allocation() {
    rejected(
        &region(
            "fn(root:List[u64])->u64{let seed:unique Bytes=unique b\"x\";let s=shared.freeze(take seed);shared.with(s,fn(value:Bytes)->u64{let fresh=unique root;list.len(fresh)})}",
            "",
        ),
        "unique root",
    );
}

fn copied_callable_branch(leaf: &str) -> String {
    format!(
        "{leaf}\nfn choose(flag:bool)->Region[fn()->u64]{{arena.with(fn(a:Arena)->Region[fn()->u64]{{let payload:Bytes in heap=copy_to(heap,b\"abc\");let f=leaf(take payload);let root=if flag{{copy_to(heap,f)}}else{{fn()->u64{{0u64}}}};arena.seal(take a,take root)}})}}\npub fn main(){{let mut r=choose(true);let n=region.with(mut r,fn(view:fn()->u64)->u64{{view()}});}}"
    )
}

#[test]
fn sealed_storage_copied_producer_33_implicit_branch_refused_by_v() {
    closure_refused_by_v(&copied_callable_branch(
        "fn leaf(take payload:Bytes)->fn()->u64{fn()->u64{bytes.len(payload)}}",
    ));
}

#[test]
fn sealed_storage_copied_producer_33_branch_allocation_retains_origin() {
    rejected(
        &copied_callable_branch(
            "fn leaf(take payload:Bytes)->fn()->u64{fn()->u64{let copied=unique payload;bytes.len(copied)}}",
        ),
        "unique payload",
    );
}

#[test]
fn sealed_storage_copied_producer_33_explicit_heap_read_control() {
    accepted(
        &copied_callable_branch(
            "fn leaf(take payload:Bytes)->fn()->u64{fn()->u64{bytes.len(payload)}}",
        )
        .replace("let root=if flag", "let root:fn()->u64 in heap=if flag"),
    );
}

#[test]
fn sealed_storage_copied_producer_33_explicit_heap_allocation_retains_origin() {
    rejected(&copied_callable_branch("fn leaf(take payload:Bytes)->fn()->u64{fn()->u64{let copied=unique payload;bytes.len(copied)}}")
        .replace("let root=if flag", "let root:fn()->u64 in heap=if flag"), "unique payload");
}

fn map_callable_branch(leaf: &str) -> String {
    format!(
        "{leaf}\nfn choose(flag:bool)->Region[fn()->u64]{{arena.with(fn(a:Arena)->Region[fn()->u64]{{let payload:Bytes in heap=copy_to(heap,b\"abc\");let f=leaf(take payload);let mut values:Map[u64,fn()->u64] in heap=map.empty();map.insert(mut values,1u64,take f);let selected=match map.get(values,1u64){{Some(found)=>copy found,None()=>fn()->u64{{0u64}},}};let root:fn()->u64 in heap=if flag{{selected}}else{{fn()->u64{{0u64}}}};arena.seal(take a,take root)}})}}\npub fn main(){{let mut r=choose(true);let n=region.with(mut r,fn(view:fn()->u64)->u64{{view()}});}}"
    )
}

#[test]
fn sealed_storage_map_producer_33_borrowed_read_root_refused() {
    rejected_by(
        &map_callable_branch(
            "fn leaf(take payload:Bytes)->fn()->u64{fn()->u64{bytes.len(payload)}}",
        ),
        "MEM-4",
        "arena.seal(take a,take root)",
    );
}

#[test]
fn sealed_storage_map_producer_33_borrowed_allocating_root_refused() {
    rejected_by(
        &map_callable_branch(
            "fn leaf(take payload:Bytes)->fn()->u64{fn()->u64{let copied=unique payload;bytes.len(copied)}}",
        ),
        "MEM-4",
        "arena.seal(take a,take root)",
    );
}

#[test]
fn sealed_storage_retained_33_copied_map_local_branch_refused_by_v() {
    closure_refused_by_v(
        &map_callable_branch(
            "fn leaf(take payload:Bytes)->fn()->u64{fn()->u64{bytes.len(payload)}}",
        )
        .replace(
            "Some(found)=>copy found",
            "Some(found)=>copy_to(heap,found)",
        ),
    );
}

#[test]
fn sealed_storage_retained_33_copied_map_callable_allocation() {
    rejected(&map_callable_branch("fn leaf(take payload:Bytes)->fn()->u64{fn()->u64{let copied=unique payload;bytes.len(copied)}}")
        .replace("Some(found)=>copy found", "Some(found)=>copy_to(heap,found)"), "unique payload");
}

#[test]
fn sealed_storage_retained_33_heap_map_value_control() {
    accepted(&region(
        "fn(root:List[u64])->unique Map[u64,List[u64]]{let mut values:Map[u64,List[u64]] in heap=map.empty();let child=copy_to(heap,root);map.insert(mut values,1u64,take child);unique values}",
        "",
    ));
}

#[test]
fn sealed_storage_retained_33_borrowed_map_insert_refused() {
    rejected(
        &region(
            "fn(root:List[u64])->unique Map[u64,List[u64]]{let mut values:Map[u64,List[u64]] in heap=map.empty();let child=copy root;map.insert(mut values,1u64,take child);unique values}",
            "",
        ),
        "map.insert(mut values,1u64,take child)",
    );
}

fn detached_map_callable_branch(leaf: &str) -> String {
    map_callable_branch(leaf)
        .replace(
            "Some(found)=>copy found",
            "Some(found)=>copy_to(heap,found)",
        )
        .replace(
            "let root:fn()->u64 in heap=if flag{selected}else{fn()->u64{0u64}};",
            "let root=copy_to(heap,if flag{selected}else{fn()->u64{0u64}});",
        )
}
#[test]
fn sealed_storage_map_detached_33_read_control() {
    accepted(&detached_map_callable_branch(
        "fn leaf(take payload:Bytes)->fn()->u64{fn()->u64{bytes.len(payload)}}",
    ));
}
#[test]
fn sealed_storage_map_detached_33_allocation_retains_origin() {
    rejected(
        &detached_map_callable_branch(
            "fn leaf(take payload:Bytes)->fn()->u64{fn()->u64{let copied=unique payload;bytes.len(copied)}}",
        ),
        "unique payload",
    );
}

pub(super) fn helper_map_callable_branch(leaf: &str, branch: bool) -> String {
    let replacement = if branch {
        "map.insert(mut values,1u64,fn()->u64{0u64});if flag{overwrite(mut values,take f)}else{map.insert(mut values,1u64,fn()->u64{0u64})};"
    } else {
        "map.insert(mut values,1u64,fn()->u64{0u64});overwrite(mut values,take f);"
    };
    format!(
        "fn overwrite(mut m:Map[u64,fn()->u64],take f:fn()->u64){{map.insert(mut m,1u64,take f);}}\n{}",
        detached_map_callable_branch(leaf)
            .replace("map.insert(mut values,1u64,take f);", replacement)
    )
}
#[test]
fn sealed_storage_map_helper_33_overwrite_read() {
    accepted(&helper_map_callable_branch(
        "fn leaf(take payload:Bytes)->fn()->u64{fn()->u64{bytes.len(payload)}}",
        false,
    ));
}
#[test]
fn sealed_storage_map_helper_33_overwrite_allocation() {
    rejected(
        &helper_map_callable_branch(
            "fn leaf(take payload:Bytes)->fn()->u64{fn()->u64{let copied=unique payload;bytes.len(copied)}}",
            false,
        ),
        "unique payload",
    );
}
#[test]
fn sealed_storage_map_helper_33_branch_allocation() {
    rejected(
        &helper_map_callable_branch(
            "fn leaf(take payload:Bytes)->fn()->u64{fn()->u64{let copied=unique payload;bytes.len(copied)}}",
            true,
        ),
        "unique payload",
    );
}

fn captured_map_callable_branch(leaf: &str) -> String {
    detached_map_callable_branch(leaf).replace("map.insert(mut values,1u64,take f);", "map.insert(mut values,1u64,fn()->u64{0u64});let install=fn()->(){map.insert(mut values,1u64,copy f);};install();")
}
fn borrowed_capture_write_refused(source: &str) {
    with_compiler_stack(|| {
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
        let file = engine
            .sources
            .add("storage-borrowed-capture.hir", source.into())
            .unwrap();
        let output = check_progress(&mut engine, file, &default_context()).unwrap();
        assert_eq!(output.completion, CheckCompletion::Complete);
        let errors: Vec<_> = output
            .diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.severity == hirsel_diag::Severity::Error)
            .collect();
        let [diagnostic] = errors.as_slice() else {
            panic!("{errors:?}")
        };
        assert_eq!(diagnostic.rule.as_str(), "CLO-1");
        assert_eq!(
            output.sources.slice(diagnostic.primary),
            Some("map.insert(mut values,1u64,copy f)")
        );
    });
}
#[test]
fn sealed_storage_map_capture_33_borrowed_read_leaf_write_refused() {
    borrowed_capture_write_refused(&captured_map_callable_branch(
        "fn leaf(take payload:Bytes)->fn()->u64{fn()->u64{bytes.len(payload)}}",
    ));
}
#[test]
fn sealed_storage_map_capture_33_borrowed_allocating_leaf_write_refused() {
    borrowed_capture_write_refused(&captured_map_callable_branch(
        "fn leaf(take payload:Bytes)->fn()->u64{fn()->u64{let copied=unique payload;bytes.len(copied)}}",
    ));
}
pub(super) fn owned_capture_map_callable_branch(leaf: &str) -> String {
    detached_map_callable_branch(leaf)
        .replace("map.insert(mut values,1u64,take f);", "map.insert(mut values,1u64,fn()->u64{0u64});let saved=take values;let install:once fn()->Map[u64,fn()->u64]=fn()->Map[u64,fn()->u64]{let mut owned=take saved;map.insert(mut owned,1u64,take f);take owned};let updated=install();")
        .replace("map.get(values,1u64)", "map.get(updated,1u64)")
}
#[test]
fn sealed_storage_map_owned_capture_33_read() {
    accepted(&owned_capture_map_callable_branch(
        "fn leaf(take payload:Bytes)->fn()->u64{fn()->u64{bytes.len(payload)}}",
    ));
}
#[test]
fn sealed_storage_map_owned_capture_33_allocation() {
    rejected(
        &owned_capture_map_callable_branch(
            "fn leaf(take payload:Bytes)->fn()->u64{fn()->u64{let copied=unique payload;bytes.len(copied)}}",
        ),
        "unique payload",
    );
}
#[test]
fn sealed_storage_map_error_33_entered_error_allocation() {
    let source=helper_map_callable_branch("fn leaf(take payload:Bytes)->fn()->u64{fn()->u64{let copied=unique payload;bytes.len(copied)}}", false)
        .replace("fn overwrite(mut m:Map[u64,fn()->u64],take f:fn()->u64){map.insert(mut m,1u64,take f);}", "fn overwrite(mut m:Map[u64,fn()->u64],take f:fn()->u64)->Result[(),u64]{map.insert(mut m,1u64,take f);Error(7u64)}");
    rejected(&source, "unique payload");
}

pub(super) fn nested_map_unknown_program(known_caller: bool) -> String {
    let mut source = String::from(
        "pub type Wrapped{call:fn()->u64}\npub fn inspect(take supplied:Map[u64,Wrapped],flag:bool)->u64{let mut r=arena.with(fn(a:Arena)->Region[Wrapped]{let selected=match map.get(supplied,1u64){Some(found)=>copy_to(heap,found),None()=>Wrapped(call:fn()->u64{0u64}),};let root=copy_to(heap,if flag{selected}else{Wrapped(call:fn()->u64{0u64})});arena.seal(take a,take root)});region.with(mut r,fn(view:Wrapped)->u64{view.call()})}",
    );
    if known_caller {
        source = source.replace("pub fn inspect", "fn inspect");
        source.push_str("\nfn leaf(take payload:Bytes)->fn()->u64{fn()->u64{bytes.len(payload)}}\npub fn main(){let payload:Bytes in heap=copy_to(heap,b\"abc\");let f=leaf(take payload);let mut supplied:Map[u64,Wrapped] in heap=map.empty();map.insert(mut supplied,1u64,Wrapped(call:take f));let n=inspect(take supplied,true);}");
    }
    source
}
#[test]
fn sealed_storage_map_nested_unknown_33_known_control() {
    accepted(&nested_map_unknown_program(true));
}
#[test]
fn sealed_storage_map_nested_unknown_33_external_alternative_refuses() {
    rejected(
        &nested_map_unknown_program(false),
        "region.with(mut r,fn(view:Wrapped)->u64{view.call()})",
    );
}

#[test]
fn sealed_storage_map_public_33_known_caller_cannot_certify_external_entry() {
    rejected(
        &nested_map_unknown_program(true).replace("fn inspect", "pub fn inspect"),
        "region.with(mut r,fn(view:Wrapped)->u64{view.call()})",
    );
}

#[test]
fn sealed_storage_map_nested_allocation_33_known_leaf_retains_origin() {
    rejected(
        &nested_map_unknown_program(true).replace(
            "bytes.len(payload)",
            "let copied=unique payload;bytes.len(copied)",
        ),
        "unique payload",
    );
}

pub(super) fn unknown_factory_program(known_caller: bool) -> String {
    let mut source = String::from(
        "pub type Product{call:fn()->u64}\npub fn inspect_factory(f:fn()->Product,flag:bool)->u64{let mut r=arena.with(fn(a:Arena)->Region[Product]{let maker=if flag{f}else{fn()->Product{Product(call:fn()->u64{0u64})}};let root=copy_to(heap,maker());arena.seal(take a,take root)});region.with(mut r,fn(view:Product)->u64{view.call()})}",
    );
    if known_caller {
        source = source.replace("pub fn inspect_factory", "fn inspect_factory");
        source.push_str("\nfn known_factory()->Product{Product(call:fn()->u64{7u64})}\npub fn main(){let n=inspect_factory(known_factory,true);}");
    }
    source
}
#[test]
fn sealed_storage_factory_join_33_known_control() {
    accepted(&unknown_factory_program(true));
}
#[test]
fn sealed_storage_factory_join_33_unknown_result_refuses() {
    rejected(
        &unknown_factory_program(false),
        "region.with(mut r,fn(view:Product)->u64{view.call()})",
    );
}

pub(super) fn ct5_storage_program(allocating: bool) -> String {
    let (generator, declaration, callback) = if allocating {
        (
            "fn clone_body[T](s:TypeShape[T])->Result[Body[fn(T)->T uses{}],String] uses{}{Ok(codegen.body(\"fn(v:standalone@0/Payload)->standalone@0/Payload uses{} {Payload(items:unique v.items)}\"))}",
            "fn generated_clone[T](v:T)->unique T uses{}=comptime clone_body(shape(T))",
            "fn(root:Payload)->unique Payload{generated_clone(root)}",
        )
    } else {
        (
            "fn length_body[T](s:TypeShape[T])->Result[Body[fn(T)->u64 uses{}],String] uses{}{Ok(codegen.body(\"fn(v:standalone@0/Payload)->u64 uses{} {list.len(v.items)}\"))}",
            "fn generated_len[T](v:T)->u64 uses{}=comptime length_body(shape(T))",
            "fn(root:Payload)->u64{generated_len(root)}",
        )
    };
    format!(
        "type Payload{{items:List[u64]}}\n{generator}\n{declaration}\npub fn main(){{let mut r=arena.with(fn(a:Arena)->Region[Payload]{{let items:List[u64] in a=[7u64];let root=Payload(items:take items);arena.seal(take a,take root)}});let escaped=region.with(mut r,{callback});}}"
    )
}
#[test]
fn sealed_storage_ct5_33_selected_generated_read() {
    accepted(&ct5_storage_program(false));
}
#[test]
fn sealed_storage_ct5_33_selected_generated_allocation() {
    rejected(&ct5_storage_program(true), "unique v.items");
}

#[test]
fn sealed_storage_ct5_reference_33_read() {
    accepted(&ct5_storage_program(false).replace(
        "generated_len(root)",
        "let reader:fn(Payload)->u64 uses{}=generated_len;reader(root)",
    ));
}
#[test]
fn sealed_storage_ct5_reference_33_allocation() {
    let source = ct5_storage_program(false)
        .replace(
            "list.len(v.items)",
            "let cloned=unique v.items;list.len(cloned)",
        )
        .replace(
            "generated_len(root)",
            "let reader:fn(Payload)->u64 uses{}=generated_len;reader(root)",
        );
    rejected(&source, "unique v.items");
}

#[test]
fn sealed_storage_ct5_helper_33_closed_actual_reaches_generated_body() {
    let source = format!(
        "fn forward[T](v:T)->u64 uses{{}}{{generated_len(v)}}\n{}",
        ct5_storage_program(false)
    )
    .replace("generated_len(root)", "forward(root)");
    accepted(&source);
}

#[test]
fn sealed_storage_ct5_helper_33_generated_allocation_retains_origin() {
    let source = format!(
        "fn forward[T](v:T)->u64 uses{{}}{{generated_len(v)}}\n{}",
        ct5_storage_program(false)
    )
    .replace("generated_len(root)", "forward(root)")
    .replace(
        "list.len(v.items)",
        "let cloned=unique v.items;list.len(cloned)",
    );
    rejected(&source, "unique v.items");
}
