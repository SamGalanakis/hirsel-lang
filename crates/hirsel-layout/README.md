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
multiplicity remains. Owner revisions participate in containing keys. Failed
requests roll back the table, recursive payloads are reserved before deriving
children, and inline cycles fail. The independent validator stays in hirsel-lir.
Instance identity retains complete type structure, binders, size actuals,
callable convention and environment layout, excluding implementation revisions.
Operation schema tags convert exhaustively in both directions and preserve the
canonical snapshot bytes. Nullable Ptr/FnPtr options use the null-pointer niche (layout version 4); the
internal aggregate ABI remains unchanged, while C adapters use one pointer word.

Flat Lists of zero-size elements store a u64 count after the byte-extent
prefix, just as variable-element Lists do. Their elements remain fixed-size.
