# hirsel-layout

The front-end bridge into native physical schemas. `layout::LayoutBuilder`
reads TypeStore and owner representation records and produces LIR layout tables;
`abi` derives declared callable ABIs; `instance` owns specialized TIR instance
keys and views. `operation::OperationRegistry` derives declaration ABIs through
LIR's `OperationDeclarations` interface, independently of candidate calls.

Allowed dependencies: hirsel-lir, hirsel-types, hirsel-tir, hirsel-syntax and
hirsel-core-contract and hirsel-codec. No implementation crate or external dependency. RC uses
this bridge in production; LIR, LLVM and runtime fixtures use a dev-dependency.
Backends and validators consume physical schemas without front-end types.

Layout keys erase relations, brands, uniqueness and sendability; callable
multiplicity remains. String and Bytes share one physical key, including inside
containing types; source types and semantic instance keys remain distinct. Owner revisions participate in containing keys. Failed
requests roll back the table, recursive payloads are reserved before deriving
children, and inline cycles fail. A payload first reached through a block (a Box,
or the element of a List, Array or Shared) is derived once the inline layouts under
way are complete, so it may hold one of them inline in any request order. The independent validator stays in hirsel-lir.
Instance identity retains complete type structure, binders, size actuals,
callable convention and environment layout, excluding implementation revisions.
Operation schema tags convert exhaustively in both directions and preserve the
canonical snapshot bytes. Nullable Ptr/FnPtr options use the null-pointer niche (layout version 4); the
internal aggregate ABI remains unchanged, while C adapters use one pointer word.
Canonical snapshots retain explicit layout, ABI and LIR versions.

Flat Lists of zero-size elements store a u64 count after the byte-extent
prefix, just as variable-element Lists do. Their elements remain fixed-size.

`with_caller_site` derives and memoizes ABI v2's optional inert source-code pair,
without changing a FunctionType. All closure/iterator callable ABIs carry it;
marked source bodies and compiled library bodies select it for their direct ABI.
