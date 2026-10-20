# IBM General Decimal Arithmetic vectors

The five `.decTest` files are unmodified copies from CPython
`b12497756ba0242a2a389dd1bb6fb26ddbd48602`,
`Lib/test/decimaltestdata/{add,subtract,multiply,divide,quantize}.decTest`.
They are also byte-identical to the audit's CPython `8b7d6c53` snapshot.
Copyright IBM Corporation, 1981, 2008; each file retains its original notice.
CPython's license is reproduced in `CPYTHON-LICENSE.txt`.

`stdlib_bigdecimal_dectest.rs` ports the directive/row structure of CPython
`Lib/test/test_decimal.py:297-355`, executing every applicable row rather than
randomly sampling rows. Counts include every active `->` row, including
`apply` and `subtract` operations embedded in `add.decTest` and the null rows:
2,100 add-file rows, 681 subtract, 521 multiply, 631 divide, 775 quantize
(4,708 total). The audit's 2,080/677/514/613/775 counts omitted active rows.

Hirsel decimals have an unbounded signed BigInt coefficient and an i32 scale.
The runner makes these adaptations explicit:

- Construct coefficient/scale from decTest syntax; Hirsel's public `parse`
  intentionally accepts neither exponent syntax nor a leading plus.
- For add/subtract/multiply, compute exactly, then `rescale` to the expected
  exponent with the row's rounding directive. For divide, request that scale
  directly. Quantize uses the second operand's exponent. Expected coefficient
  AND scale must match; negative zero becomes coefficient zero.
- Precision, exponent limits, clamp and IEEE condition flags are context
  features outside this API. Finite expected answers still test explicit
  rounding to the reference scale. Context overflow, nonfinite/invalid
  operands/results, encoded decimal64/128 literals and null operands do not
  map to this finite API and are counted as exclusions.
- `half_down`/`05up` inexact rows are excluded because neither rounding mode
  exists in the API. Exact rows exercise `Down`, for which results coincide.
- Cases requiring more than 10,000 decimal places of coefficient expansion
  are counted separately. Billion-place exponents would require prohibitive
  allocations in an unbounded implementation; this is a test resource bound,
  not a change to arithmetic semantics.

The Rust suites check `decimal-vectors.hir` once per suite, then execute
parameterized rows in the interpreter with a 100-million-step per-row budget.
Native decimal compilation currently hits the independently tracked RC importer
panic (`gate-std-native-shard-1` in `tools/known-failures.tsv`); the interpreter
covers all selected arithmetic now. The large BigInt suite uses a separately
compiled native `numeric-vectors.hir` checker.

Each suite asserts its total row count and execution/exclusion counts so a
new skip cannot silently reduce coverage. Existing rust_decimal-based corpus
cases continue to check exact unbounded arithmetic without rescaling.

| File | Executed | Nonfinite/invalid/encoded | Unsupported rounding | Expansion >10,000 | Context overflow |
|---|---:|---:|---:|---:|---:|
| add | 1,842 | 220 | 26 | 12 | 0 |
| subtract | 581 | 98 | 0 | 0 | 2 |
| multiply | 372 | 145 | 0 | 4 | 0 |
| divide | 450 | 180 | 1 | 0 | 0 |
| quantize | 559 | 201 | 0 | 15 | 0 |
| Total | 3,804 | 844 | 27 | 31 | 2 |
