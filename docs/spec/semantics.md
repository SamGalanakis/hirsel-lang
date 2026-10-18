# Everyday semantics

Integers fail instead of wrapping, floats follow IEEE 754 with one observable NaN, strings
are always valid UTF-8, loops bind fresh names per iteration, and maps iterate in insertion
order. Every other behaviour is a named function.

### SEM-1: Integers

`+ - *` and unary `-` fail on overflow. Other behaviours are named functions:
`int.wrapping_add`, `int.saturating_add`, `int.checked_add` (returning an `Option`), and so
on; `unchecked_*` needs `Unsafe`. `/` truncates toward zero and `%` takes the dividend's
sign; `div_floor`, `mod` and `div_exact` are named. Division by zero, `MIN / -1` and shifts
by at least the width fail. Remainder by zero fails; signed `MIN % -1` is zero, although
`MIN / -1` fails. There are no mixed-width operators.

```hirsel
fn add(x: u8) -> u8 { x + 100 }                       // add(200) fails SEM-1
fn add_wrapping(x: u8) -> u8 { int.wrapping_add(x, 100) }
fn add_checked(x: u8) -> Option[u8] { int.checked_add(x, 100) }
```

Conversions are named: `widen`, `try_narrow` (returning an `Option`), `narrow` (failing
when the value does not fit) and `truncate`. Shift counts have the left operand's integer
type; negative counts and counts at least the width fail SEM-1. A left shift is a
fixed-width bit operation that discards high bits (not checked multiplication); a right
shift sign-extends signed integers and zero-extends unsigned ones.

### SEM-2: Floats

Floats are IEEE binary32 and binary64, round-to-nearest-even, with no mode state. There is
no contraction or reassociation; `float.mul_add` fuses explicitly. Each float type has one
observable NaN. `float.min` and `float.max` are IEEE 754-2019 `minimum` and `maximum` (NaN
propagates, −0 < +0); `float.min_number` and `float.max_number` ignore NaN.
`f32.sum` and `f64.sum` use the fixed reduction tree of the
[core library](core-library.md#numeric-and-string-details); `f32.sum_exact` and
`f64.sum_exact` are correctly rounded. Math functions are correctly rounded library
definitions. `float.to_i32` returns a `Result`; `float.total_order` is for sorting. A finite
literal spelling outside its type's finite range rounds to infinity.

### SEM-3: Strings

A character literal (`'a'`, `'é'`, `'\u{1f411}'`) denotes its Unicode scalar
value as a `char`, the type `string.scalars` yields. It is a constant in both
interpreted and native execution; it does not allocate a string.
The escapes `\n`, `\t`, `\r`, `\0`, `\\`, `\'` and `\u{...}` denote one scalar.
Character literal patterns match that scalar value. Lexical validity is defined
in [syntax](syntax.md); there is no implicit conversion to or from an integer.

`String` is always valid UTF-8; `Bytes` is arbitrary. There is no `length`: there is
`byte_len`, plus built-in views over a borrowed string (`string.scalars(s)`,
`string.positions(s)` yielding `StrPos of s`, `unicode.graphemes(s)`), which are loop
sources like sequences. Slicing with a `StrPos of s` never splits a scalar. `==`, order and
hash are bytewise; normalisation is explicit. Unicode tables are pinned definitions. Bidi
control characters in source must be escaped; invalid UTF-8 in a source file is SEM-3.

An f-string ([syntax](syntax.md)) evaluates to a fresh `unique String`: its text and
holes, left to right, appended to an empty builder through
[the formatting protocol](std-library.md#the-formatting-protocol), then finished.
The two functions below are equivalent:

```hirsel
fn label(x: u64) -> String {
  f"a{x}b"
}

fn label_spelled_out(x: u64) -> String {
  let mut out: List[u8] = []
  text.append(mut out, "a")
  u64.format_into(x, mut out)
  text.append(mut out, "b")
  text.finish_valid(take out)
}
```

except that the f-string's builder is a block-scoped name no program can write, and a
piece's text is appended only when it is nonempty.
The parser performs exactly this rewrite, so checking, ownership, lowering and both
engines see ordinary calls of `std@1`, which the program must select; otherwise every
f-string is SEM-3. A hole is a borrowed argument, read and not consumed, like an operand
of `==`; it is checked with no expected type, so an unsuffixed literal hole needs a
suffix. The f-string's `uses` are its holes' `uses`, because the std protocol operations
use nothing, and it allocates only its builder, plus whatever its holes' `@format`
functions use and allocate. A hole of type `String` is appended as is,
and `bool`, `char`, the integers and the floats as `to_string` spells them: a `char` is its
UTF-8 encoding, so `f"{'a'}"` is `"a"`. A hole of a declared type with a `@format`
association calls that association's function, below. Any other hole type, including
`Bytes`, tuples, type parameters and declared types without `@format`, is SEM-3 naming
the type; the message for a declared type names `@format`.

**Format associations.** A nominal type opts into formatting with
`@format(function)`, which names one ordinary declared function. After substituting the
type's own type and size parameters, the function is exactly
`fn(value: T, mut out: List[u8]) -> ()`: an unlabelled borrowed receiver of the declaring
type `T`, an unlabelled `mut` UTF-8 builder, a unit result and no error result. A generic type's function is generic over the same
parameters (`@format(pair_text)` on `Pair[A, B]` names
`fn pair_text[A, B](value: Pair[A, B], mut out: List[u8]) -> ()`), each inferred from the
receiver. The association implies `@preserves_unique(out)`: both the source ownership
checker and the independent IR checker prove the body under that rule, and direct
calls preserve the builder's uniqueness. No ownership annotation is needed; writing
the attribute explicitly is allowed and redundant. A body that fails the proof is
MODE-2 at the function, with a note naming the `@format` association. The function
appends the value's text and keeps the builder's existing content, as the protocol
operations do. The association is a signature input checked at the attribute,
including when no f-string uses the type; any other shape is SEM-3 there.
There is no derive, structural lookup or automatic formatting of records: an f-string
hole of type `T` is `function(hole, mut builder)`, the same ordinary call as
`u64.format_into` for a `u64` hole, checked as one. The function may be private;
publishing the association grants f-strings, and only f-strings, permission to call it,
as a loop source's association grants loops ([SEM-4](#sem-4-iteration)).

A hole may carry a format specification, such as `{n:08}`, `{mean:.2}` or `{name:>w$}`.
Options of only fill, alignment and width pad the hole's text: the parser binds the
builder's byte length before a plain append, then calls `text.pad_from` (`String`, `bool`, `char`
and `@format` types, left-aligned by default) or `text.pad_number_from` (integers and
floats, right-aligned by default) with that start, the fill, alignment and width. Any
other options pass to the selected scalar or `String` module's `format_into_spec`. On a
`char` or a `@format` type, precision, sign, `#`, `0` and radix formats are SEM-3: its text
pads like a string but has no numeric or truncating options. A width or precision written
`name$` is the value of the local or constant `name`, passed in place of the constant as
an ordinary `u64` argument, so a name of another type is LANG-7; `{x:w$}`, `{x:.p$}` and
`{x:>w$.p$}` are the same operations as their constant forms. Positional `1$` and `*`
counts are SEM-3. Invalid syntax and a radix format on a noninteger value are SEM-3,
with the unsupported format and value type named. Width, alignment, padding, precision
and radix follow [the formatting protocol](std-library.md#the-formatting-protocol).

### SEM-4: Iteration

`Bytes` is a byte sequence: `for b in data` yields `u8` values in byte order.
A `take` loop consumes the byte source; `mut` byte loops are rejected because
`Bytes` is immutable.

`for` takes a range (`a..b`, `a..=b`, with no overflow at `MAX`), a sequence (`Array`,
`List`, `Inline`, flat and `Packed` sequences, string views), a declared loop source, or an
`Iter[T]`, and binds a fresh name per iteration. Over a sequence, the source's marker sets
the mode:

- `for x in xs` binds a borrowed element; the source may be read in the body;
- `for x in mut xs` binds the element place; the source may not be used in the body, and
  the element type may not carry facts;
- `for x in take xs` consumes the source, which is gone afterwards.

Over an iterator, `for x in take it` consumes it and `for x in mut it` advances it; each
item is an owned value. `Iter[T]` is a noncopyable pull iterator built by
`iter.from(take s, next)` (an allocation site) from a state and a `next(mut S) -> Option[T]`;
it erases `S`, so each adapter step is an indirect call. A fallible iterator is an
`Iter[Result[T, E]]`; library iterators end after their first `Error`.

```hirsel
fn evens(n: u64) -> u64 {
  let mut count: u64 = 0
  for i in 0..n { if i % 2 == 0 { count += 1 } }
  count
}

fn total(m: Map[String, u64]) -> u64 {
  let mut sum: u64 = 0
  for (k, v) in m { sum += v }
  sum
}
```

Ranges are direct loop-source syntax: storing, passing, returning or wrapping a range is
SEM-4. Borrowed and mutable collection loops bind element places, not owned call results.
`Map` and `SortedMap` keys and whole entries are read-only in loops; mutable loops may
update values. Consuming loops transfer entries, removing each before publishing its
ownership, and keep the remaining container and comparator cleanup. Every collection may be
consumed while local; the cursor's, step's and item's locality follows the consumed current
content, not its receiving annotation. An element or view that still depends on the cursor
scope cannot be returned through `in xs` or kept outside the loop. A borrowed loop lends its
source for as long as it may advance again.

**Declared loop sources.** Any package may attach one borrowed loop protocol to a nominal
type with `@loop_source(open, next)`. The two paths resolve to ordinary declared functions.
After substituting the source's complete type and size arguments, `open` has one unlabelled
borrowed receiver of exactly the declaring type and returns an owned cursor related to that
receiver; `next` takes that cursor `mut` and returns the exact core `Option` of the yield
type. There is no structural method lookup, overload, implicit trait, erased constructor
allocation or privileged library hook; operation names, module paths and bodies supply no
protocol meaning. The association, the receiver, cursor and yield contracts, generic
arguments, authority, allocation and cleanup promises are signature inputs, published
before bodies and checked at the attribute, including when unused. The receiver, cursor and
yield types must be nameable at the association's visibility
([PROG-2](programs.md#prog-2-visibility)); the operations themselves may be private, and
publishing the association grants loops, and only loops, permission to invoke them.

A declared source is evaluated once and opened once, then advanced by `next` until `None`.
Each `Some` yields an owned value satisfying its declared relation; a counted borrowed
slice is an owned owner-sharing value, never a non-owning resource result. Advancing the
cursor conflicts with a live yield only where their declared retained places overlap; a
yield related to the original source acquires no cursor-scope bound. The source loan covers
`open` and every live cursor and yield holder. Normal exhaustion, `break`, `return` and
failure release iteration holders before the cursor and the retained source, with ordinary
reverse-scope cleanup and [FAIL-3](failure.md#fail-3-what-runs) shielding. A noalloc loop
means `open`, `next`, construction and every relevant cleanup are proved noalloc. `mut` and
`take` loop markers need a separately declared protocol; a borrowed association admits
neither.

### SEM-5: Maps

The operators `<`, `<=`, `>` and `>=` compare `String` and `Bytes`
byte-lexicographically; a proper prefix sorts before the longer value.

`Map[K: key, V]` iterates in insertion order; the hash seed affects placement only. Map
`==` is order-sensitive, and `map.same_entries` compares as sets. `Set` equality
ignores order. Structural comparison of `Set` values sorts each set's elements by
structural order on their element type, then compares the sorted sequences
lexicographically, the shorter first on a common prefix; nested sets use the same rule.
This internal order keeps equality as mutual containment and hashing commutative; source
ordering operators stay limited to scalar and byte operands.

The keyed reads `map.get` and `set.contains` use the same hash and key equality as
insertion and change neither the container nor its order; `map.get` returns the value
stored under an equal key, or `None`, and its result is related to the map
(`Option[V] in m`). Insertion takes its key. Map insertion replaces an existing value in its
existing position, keeping the stored key, finalizes the displaced value and then drops the
incoming key; Set insertion of an existing key changes nothing and drops the incoming one. The default hasher is keyed SipHash-1-3.

`SortedMap` is `of` an `Order` and has no `data` or `key` equality. `sorted_map.insert`
searches `[lo, hi)`, initially `[0, len)`, taking `mid = lo + (hi - lo) / 2` and comparing the
incoming key against `entries[mid]`. A zero result replaces at `mid`, a negative result sets
`hi = mid`, and a positive result sets `lo = mid + 1`; when `lo == hi`, it inserts at `lo`.
Every comparator call finishes before any write, and the probe sequence is observable
through comparator failure. The key is taken; an equal replacement keeps the existing key
and position, installs the incoming value, finalizes the displaced value, then drops the
incoming key. A comparator failure preserves the map and releases the still-owned incoming
key and value through normal cleanup.

The run's placement seed is not the Random seed, the Test seed, the scheduler seed or a
user `Hasher` argument. Varying only that seed cannot change program observations.
