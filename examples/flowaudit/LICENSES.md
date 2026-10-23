# NetworkX

The bidirectional BFS and residual rules are adapted from NetworkX commit
`9d866a5f447d9fe1c989b31f90c520dc2813ee26`,
`networkx/algorithms/flow/edmondskarp.py:11–94` and `utils.py:82–157`.
Classic, zero-flow and Cormen test vectors follow `tests/test_maxflow.py:133–199`;
independent feasibility/conservation validation follows lines 39–78.

Semantic adaptations: dense bounded vertex ordinals replace arbitrary hashable
objects; declared vertex order replaces dict insertion order; all capacities are
finite bounded integers; loops and duplicate arcs refuse; no cutoff or infinity
sentinel exists; report flows remove opposite-direction circulation via max(net,0).
Zero-capacity pairs occupy matrix cells but cannot be traversed unless cancellation
creates positive residual. Reachability is recomputed after termination. The
verifier uses a monotone fixed point and cut equality instead of augmenting paths.

NetworkX is distributed with the 3-clause BSD license.

::

   Copyright (c) 2004-2026, NetworkX Developers
   Aric Hagberg <hagberg@lanl.gov>
   Dan Schult <dschult@colgate.edu>
   Pieter Swart <swart@lanl.gov>
   All rights reserved.

   Redistribution and use in source and binary forms, with or without
   modification, are permitted provided that the following conditions are
   met:

     * Redistributions of source code must retain the above copyright
       notice, this list of conditions and the following disclaimer.

     * Redistributions in binary form must reproduce the above
       copyright notice, this list of conditions and the following
       disclaimer in the documentation and/or other materials provided
       with the distribution.

     * Neither the name of the NetworkX Developers nor the names of its
       contributors may be used to endorse or promote products derived
       from this software without specific prior written permission.

   THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS
   "AS IS" AND ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT
   LIMITED TO, THE IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR
   A PARTICULAR PURPOSE ARE DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT
   OWNER OR CONTRIBUTORS BE LIABLE FOR ANY DIRECT, INDIRECT, INCIDENTAL,
   SPECIAL, EXEMPLARY, OR CONSEQUENTIAL DAMAGES (INCLUDING, BUT NOT
   LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR SERVICES; LOSS OF USE,
   DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER CAUSED AND ON ANY
   THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY, OR TORT
   (INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE
   OF THIS SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.
