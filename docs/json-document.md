# Scoped JSON documents

`std@1/json_document` parses a reusable, immutable document for a synchronous callback:

```hirsel
let encoded = json_document.with_document(
  copy input,
  fn(doc: json_document.Document) -> Result[String, json.JsonError] {
    json_document.stringify(doc)
  }
)
```

The result is `Result[R, json.JsonError]`. Parsing and trailing-input validation finish
before the callback runs. A JSON error calls it zero times; success calls it once. Callback
authority is forwarded through `uses {body}`. Its result must be global, including nested
payloads. Return scalars or independently owned results; a document, byte view, copied
view, or closure borrowing the document cannot escape.

`root` returns position zero. `kind`, `first_child`, `next_sibling`, `member`, `bool_value`
and `count` return checked optional values. Kinds are `Null`, `Bool`, `Number`, `Text`,
`Array` and `Object`; `count` counts direct container children. Object children are their
value nodes, with keys available through `key_bytes`. Children and siblings preserve input
order. Wrong kinds, absent members and invalid positions return `None`. Positions are
ordinary integers: using one with another document addresses that other document's position.

`text_bytes`, `key_bytes` and `number_bytes` return `Option[Bytes] in doc`. Unescaped text
and exact number spellings refer to the supplied input slice. Escaped text shares one decoded
buffer. An empty string or key yields `Some(empty)`. These views remain inside the callback.

`text_owned` copies just the selected decoded text. `to_owned` copies the selected subtree
into an ordinary `json.Value`, including independently copied text, keys and number strings.
Neither result retains the input, node pool or decoded buffer. `stringify` writes the tree
directly into an independent compact string, preserving number spelling and the ordinary
writer's escaping. Parsing, stringifying and extraction allocate and cannot be called from
`@noalloc` functions.

The document stores inline 80-byte nodes in one list, input ownership once, a decoded byte
buffer, and bounded parser frames. Nodes have no counted fields. Ordinary growth, temporary
views and output buffers still have costs. The dense node stride and growth policy can
increase peak storage and capacity traffic; fewer allocations do not guarantee lower cycles
or memory use. Duplicate keys are rejected before checking the
colon or parsing their values. The existing UTF-8, surrogate and 128-container rules and
`JsonError` byte positions apply.

Duplicate detection scans previous sibling keys: parsing an object with k members takes
O(k²) key comparisons. Member lookup takes O(k). There is no width limit. This first API
therefore suits reusable documents with modest object widths; it makes no general performance
claim for wide objects. During the callback a small input slice can retain its whole backing
allocation. Explicit extraction is the way to keep a small result independently.

The existing `json.Value`, parser, reader and writer APIs keep their behavior. Use the reader
for one-pass consumption and an owned value when mutation or retained subtrees are needed.
