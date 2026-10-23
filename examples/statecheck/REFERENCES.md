NetworkX, BSD-3-Clause (LICENSES/NetworkX.txt), commit
`9d866a5f447d9fe1c989b31f90c520dc2813ee26`:

- `networkx/algorithms/components/strongly_connected.py:76–148`, iterative
  lowlink/lead DFS ported in components.hir. Whole-file SHA256
  `53f2e30104287bfc40d2d90046a43db2a8c27e2c558918459bc57a604a65bf91`.
- `networkx/algorithms/components/tests/test_strongly_connected.py:7–78`, five
  directed SCC vectors ported, translating one-based indices where needed.
  SHA256 `f8300dbe44256eb3da046c79f0c4d1ea5e9da92c63ffb69bc3bf25a9d140b1fe`.
- `networkx/algorithms/traversal/breadth_first_search.py:85–106`, visited-on-
  discovery FIFO BFS ported in reach.hir. SHA256
  `fe45bf02a09e0317a3253649656820a6f53b6871df9aa552562d30d604c02342`.
- `networkx/algorithms/traversal/tests/test_bfs.py:8–58`, directed, reverse and
  undirected-as-paired-arcs BFS vectors. SHA256
  `4b2eefd057ff5030ba564ab73b70a9a2813f0f63a6174708b16e4d4bf1f726da`.

Deviations: finite arrays indexed by declarations replace Python dicts/sets;
typed frames with edge cursors replace iterators; the single-SCC early return
is omitted to keep one finishing path; SCC representatives are normalized to
minimum declaration index. Terminal absorption, restricted reachability, fixed
caps, checked owned lasso reconstruction and no-fairness policy are application
semantics. Oracle fixtures use independent Boolean closure and exhaustive small
edge walks rather than the ported lowlink/parent algorithms.
