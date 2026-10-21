# First-contact friction

Minutes are approximate elapsed investigation time; overlapping waits are not a benchmark.
Copied diagnostics normalize checkout paths and strip trailing display whitespace.

| Tried | Exact diagnostic | Fix | Minutes lost | Class |
| --- | --- | --- | ---: | --- |
| Expected raw HTML from the spec overview, unlike the API index. | [Exact output](repros/incident-1.txt) | The API correctly documents seven options and safe rendering. Fixed the stale overview wording and asserted safe default output. No Markdown rules changed. | 1 | doc gap |
| Compiled `markdown.to_html` natively. | [Exact output](repros/incident-2.txt) | [Minimal program](repros/markdown-native.hir) and adjacent manifest; native RC import panics, interpreted succeeds. Routed @agentlang-24 message 557477; native-parity owns the importer fix (coordination message 557632). No compiler change in this lane. | 2 | bug |
