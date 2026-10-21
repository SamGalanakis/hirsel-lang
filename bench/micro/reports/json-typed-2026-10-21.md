# Direct typed JSON decoding, October 21, 2026

The pull reader removes the intermediate JSON DOM from tasks snapshot decoding.
Restart instructions fall from 3.739× to 2.931× Rust (21.60% fewer instructions).
The typed JSON micro falls from 4.472× to 3.465× Rust (22.51% fewer); the existing
dynamic JSON micro remains 1.633×. Restart remains above the 1.5× target.

[Raw counters, executable hashes and comparisons](json-typed-2026-10-21.json)
retain all 90 samples from one ten-round session, seed 20261021. The harness is
`bench/micro/typed-session`; its start timestamp precedes its wait for the CPU31
timing lock. No samples were excluded. Both Hirsel binaries use the same release
compiler/runtime at baseline revision `49b74942`, LLVM O3. The before tasks source
is that revision; after adds this lane's pull decoder. The final build used
`HIRSEL_JOBS=1`, matching std precheck, and the unchanged 4 GiB native guard.
The std precheck passed under 2 GiB, peak 1725 MiB. No compiler or runtime change
is included. The new byte entry explicitly establishes ownership of its typed
result, including recursive types.

The tasks workload loads the same seeded 10,000-item snapshot and answers one
request. Rust is the existing tasks one-worker participant. The typed micro
constructs 20,000 rows, decodes ten times, and checksums the typed fields; before
uses `json.parse` then the existing derived Value decoder, after uses the byte
entry. Rust uses serde-derived `Deserialize`. The dynamic row constructs 20,000
records and runs ten parse/stringify/drop passes through unchanged APIs. Inputs,
checksums and semantics are equal within each micro row; duplicate keys and
unusual number spellings remain covered by conformance, outside these inputs.

Server and micro processes use CPU31; client/coordinator use CPU7. The shared
CPU31 timing lock spans the session. Each cell uses the normal capped launcher;
perf counts total/user instructions, cycles and task-clock. Raw CPU15 sibling
busy shares and counter running percentages are retained. Ratios below are ratios
of medians, with 95% paired-round bootstrap intervals (5000 resamples). Timing
ratios apply only within this session and do not imply isolated-core speed.

| Workload | Participant | Median instructions | Instructions / Rust [95% CI] | User instructions / Rust | Cycles / Rust | Task-clock / Rust [95% CI] |
|---|---|---:|---|---:|---:|---|
| restart | before | 361,745,200 | 3.7389 [3.7357, 3.7412] | 3.7504 | 3.2930 | 3.0534 [2.6936, 3.2844] |
| restart | after | 283,617,326 | 2.9314 [2.9224, 2.9336] | 2.9831 | 2.5077 | 2.3370 [2.0708, 2.6576] |
| typed | before | 4,757,365,226 | 4.4721 [4.4717, 4.4726] | 4.4598 | 5.9153 | 5.9149 [5.6108, 6.1604] |
| typed | after | 3,686,254,949 | 3.4652 [3.4649, 3.4657] | 3.3608 | 5.1359 | 5.0431 [4.7588, 5.3978] |
| json | before | 4,184,275,603 | 1.6326 [1.6324, 1.6327] | 1.6303 | 1.4282 | 1.4453 [1.3646, 1.5078] |
| json | after | 4,184,252,028 | 1.6325 [1.6324, 1.6327] | 1.6303 | 1.4289 | 1.4292 [1.3838, 1.5070] |

Typed checksum at N=1000 is 5093900 for Rust, DOM and direct; dynamic checksum is
906710 for Rust, before and after. All measured N=20000 output hashes match
within each row. Successful direct decoding never constructs `json.Value`:
`json.hir` only gained the additive reader, and its DOM parser/encoder bodies
are unchanged. The typed micro's reachable native symbols contain no Hirsel
`json_node`/DOM parse function.

## Residual attribution

[Separate startup profiles](json-typed-2026-10-21-profile.json) sampled user
instructions at period 10000, with 490/366/141 samples before/after/Rust and no
lost samples. They are deliberately separate from the 90 verdict cells. These
short profiles locate work; their percentages are approximate self samples, not
an exact decomposition of the total instruction gap. Unresolved addresses,
loader work and kernel skid samples remain in the raw symbol table.

| Symbol family, self samples | Before | After |
|---|---:|---:|
| DOM/typed JSON parser and derive | 18.37% | 15.30% |
| Runtime graph copy, including copy identity tables | 3.67% | 11.75% |
| Runtime release/RC | 7.14% | 14.76% |
| UTF-8 validation | 3.27% | 4.65% |
| Runtime sequence allocation/grow | 9.18% | 10.11% |
| JSON map probe/get/insert and key SipHash | 15.09% | 0.00% |

The old `json_node` and quoted-string DOM path disappears from the after
profile. Generated known fields compare borrowed keys directly, eliminating
the DOM's field-map hashing. Remaining costs include the typed result's ownership
boundary, token-to-owned String allocation, per-number spelling conversion,
UTF-8 scanning, and ordinary grow/release operations. The after graph copy is
concrete: `board_from_json_bytes` calls `hirsel_rt_v2_unique_copy_value` after
building the typed Board. It copies the typed result, not a JSON tree. The
compiler conservatively treats a recursive result from a mutable Reader as
potentially holding the borrowed input; returning its moved success payload
still produces MODE-1, while explicitly detaching the Result passes. The
uniform byte entry therefore establishes independent ownership this way;
`from_json_reader` remains available for callers controlling the reader scope.
Removing this extra typed copy without weakening locality needs a separate
ownership/compiler improvement. No compiler construct, runtime ABI, slice-copy
behavior, or encoder change was made in this lane.

The first response also includes unchanged list selection, tag extraction,
ETag generation, HTTP startup, loading and cleanup; the profile shows those
symbols alongside decoding. The remaining 2.931× restart ratio is therefore
attributed to measured ownership/allocation/scanning work plus unchanged app
startup and response work, rather than claimed to meet the 1.5× target.

## Validation

The selected corpus grows from 67 to 108 programs: all 108 execute and pass,
including 41 new programs covering the pull/typed path and the 293 imported
UTF-8 JSONTestSuite inputs. Exact JsonError variants and offsets are compared
against unchanged `json.parse`. The affected enum twin was rerun after naming
its temporary encoded String to satisfy native cleanup ordering. All 11 selected
native cases pass LLVM controls and Cranelift parity; all 22 tasks tests pass.
The 71 stdlib shards execute 277 passing, 1 failing and 135 ignored tests. The
single failure is the pre-recorded, unrelated
`stdlib_http::http2_adapter_scripts_record_and_replay` (`net.connect`,
`tools/known-failures.tsv`, owner std-web-2). Agreement's six selected groups pass: 199 checked variants,
22 executed, 84 rejected, 93 stopped at the documented runtime limit,
0 skipped and 0 unavailable. The 108 paths are identical to the corpus selection. The exploratory CLI `test` probes without
`@test` entries executed zero tests and establish compiler acceptance only;
they are not included in these runtime counts.
