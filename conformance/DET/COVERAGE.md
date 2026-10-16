# Coverage

DET-1 has a fixed-output replay seed. DET-2 has left-to-right output, reverse resource-drop output and first-failure-site seeds. DET-3 has a known-false runtime-contract seed. These programs are inputs to the external comparison matrix in UNTESTABLE.md; no single header establishes configuration independence. DET-4 has no .hir test because it excludes cases from the guarantee rather than assigning one required outcome.

Output follows v20's exact-text contract (explicit newline), and scalar signature defaults follow LANG-6 in v20. Other prefixes supply float, map-order, ownership, destruction and allocation witnesses for the shared matrix.
