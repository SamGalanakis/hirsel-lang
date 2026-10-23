# Config migration friction

The original JSON commands and nine package tests are preserved. This extension
uses shipped std TOML/diff/file/task APIs; no parser or runtime adapter is added.
Evidence and failed drafts: `/workspace/code/hirsel-briefs/config-migrate-29/`.

| Attempt / actual diagnostic | Expected contract and resolution | Cost / effort | Disposition / owner / witness |
| --- | --- | --- | --- |
| `let result = Timeouts(...)`: LANG-14 reserved postcondition name | Rename to `decoded`; intended rejection | no runtime cost; 2 min | intended; app; draft-1, check-1.log |
| `copy toml.source(doc)` retained across `toml.set(mut doc,...)`: MODE-1 | Borrowed source views cannot survive an exclusive edit. Use explicit `unique` snapshots at document boundary | O(file bytes) snapshots; 5 min | intended; app; draft-2, check-2.log |
| `map.get` returned globally from a borrowed root: MODE-1 | Declare `in value`; text fields copied into owned schema records | O(endpoint/level bytes); 3 min | intended; app; draft-2/3, check-2/3.log |
| Borrowed fields stored in global Map/List/Set: MODE-1; task reports lacked nested uniqueness: MODE-2 | Explicit unique copies at container and task transfer boundaries; consuming generic transition carries each next record | bounded deep copies of reports/schema; 10 min | intended; app; drafts-2/3, check-2/3.log |
| Local `version`, `endpoint`, `row` shadow module functions: LANG-4 | Rename locals/helpers; declaration order cannot permit shadowing | no runtime cost; 2 min | intended; app; draft-2, check-2.log |
| Unannotated `Result` branches: LANG-2 | Write complete prefix type for admitted size | no runtime cost; 1 min | intended; app; draft-2, check-2.log |
| Native file-fault/cancellation injection has no public scripted host file provider | Reference and checked H use SimulationConfig faults; native exercises real confined files. Do not add an adapter in this lane | no workaround claimed; 5 min inspection | unsupported / INCOMPLETE; runtime owner @agentlang-24; tests/support/config_migrate_29/cases.json |
| Checked-H host-tape replay has no supported public adapter | Full checked-H scripted workflows remain required; host-tape replay is explicitly unsupported | no workaround claimed; 3 min inspection | unsupported / INCOMPLETE; runtime owner @agentlang-24 |

Formatting is std's precise contract: existing scalar `set` replaces its value
span, retaining key spelling and inline comment. New nested keys use std's table
placement. `remove` removes the deleted entry's owned trivia, including its inline
comment and preceding owned comments. Unchanged regions and already-current input
remain exact. The app never promises to relocate deleted-key comments.

## Qualification disposition

The full application is selected through its real `hirsel.json`, entry and seven
explicit modules. The six migration-facing files total 852 physical lines:
main 249, schema 195, migration 121, document 40, batch 151, publish 96.
The unchanged model is 259 lines (204 before its package tests); production
application source is therefore 1056 lines, with 1111 lines including those tests.

The external matrix and immutable receipts live under the evidence directory
above. The initial matrix preceded implementation; later tiny/plan/trivia vectors
are separately named additions, never replacements for an incomplete original.

| Obligation | Actual disposition |
| --- | --- |
| Reference full app | 32 semantic passes / 39 workflow and boundary identities; seven INCOMPLETE at the unchanged 1000000-step tooling limit |
| Checked H full app | Same 32 passes / 39 identities, including injected canceled reads/writes, write faults and output errors; same seven INCOMPLETE |
| LLVM and Cranelift | 23 supported workflow identities each; 12 cap/one-over identities each; extra requested widths 2/4 for success and partial domain failure |
| Native recordings / replay | 54 accepted success/domain-error recordings, 162 replays through reference/LLVM/Cranelift with original mounts and grants unavailable; full stdout/stderr/exit matches |
| Replay refusal controls | Truncated tape and independently changed argument/config bindings refused by all three adapters |
| Width determinism | Eight complete tape comparisons across requested widths 1/2/4; only documented placement and admission timestamp normalized; no sorting or dropped semantic events |
| Effective physical width | INCOMPLETE: short processes left metrics files empty; requested widths are evidenced, physical execution width is not claimed |
| Native cancellation/cleanup recording and replay | INCOMPLETE: injected FileError cancellation tests are not a task-cancellation recording; no public scripted native file-fault seam or app cancellation trigger was added |
| Checked-H host-tape replay | Unsupported / INCOMPLETE; no adapter added |
| Original JSON tests | All three named tests pass separately, unchanged assertions: 69 CLI + 66 data + 24 replay observations, nine package executions, six records / 18 replays |

The seven default-budget identities, in each of reference and checked H, are
`v1-v2`, `v1-v3`, `mixed`, `tiny-direct`, `files-16`, `bytes-65536`, and
`total-524288`. Retained sources and exact original failures are in
`reference-seal.json`, `test-reference.log`, `test-reference-remaining.log`,
`test-checked.log`, `test-model-final.log` and `test-bounds.log`. The tiny direct
fixture reaches the limit while serializing its full report. Owner:
`interp-step-budget-29` / `@agentlang-24`; this lane never raises the budget or
claims a pure/helper execution as the app result.

Additional harness friction:

| Attempt / behavior | Resolution / cost / active effort | Disposition / witness |
| --- | --- | --- |
| Duplicate fixture appended `version=1` after `[[workers]]`; native correctly accepted a worker field | Move the duplicate into the root; retain original expected refusal and actual success. No app change; 5 min | fixed fixture; app; original native directory `config-migrate-29-both-nmxtvq`, test-native.log |
| Canonical trace requested while Tasks grant deliberately absent; launcher refused with exit 3 | Omit tracing for this missing-grant recording, which then returns the expected application error. No runtime change; 3 min | intended observer restriction; test-native-remaining.log and test-native-final.log |
| Combined legacy test action reached RE 600 s with empty output | Run only the three unproved named identities separately at the same 600 s cap, on the supported host test launcher; assertions unchanged. No repeats of named greens; 5 min | tooling, recovered; test-legacy.log and test-legacy-{cli_arguments,data_validation,record_replay}.log |
| Pool tests do not inherit arbitrary shell evidence variables | Native qualification uses supported `hirsel.tests=host` and explicit test-executor `--env`; source/registration remain frozen through readers. Earlier pool semantic receipts are retained in their complete console logs; 5 min | tooling; test-native*.log |
| Initial native exact copied tool/config hash capture was incomplete before execution | Retain honestly labelled `late-postrun-seal.json`; later partitions add preexecution tool/std/source seals. No semantic reruns merely to strengthen sealing; 5 min | tooling/evidence limitation; native evidence directories |

The step-budget owner independently measured the original v1→v2 workflow at
1126951 steps under a separately named explicit 2M experiment. That does not
change this task's original 1M dispositions. Deadline/replay owners keep their
existing defects; no parallel runtime repair was attempted.
