# Mode implementation oracles

The .hir cases test acceptance and observable semantics. These parts require an implementation oracle:

| Rule | Part no single program can observe | Oracle |
| --- | --- | --- |
| MODE-1 | Caller-provided destination for local(x) results; shortest-lived placement independent of optimization. | Inspect typed IR destination/loan annotations; differential interpreter/native tests and modes soundness proof. |
| MODE-2 | Every reachable counted block is exclusively owned; Shared stops uniqueness traversal; mut unique roots update in place. | RC/modes model plus instrumented allocation/count traces and Shared-boundary walk tests. |
| MODE-3 | task.scope invokes its once body no more than once, under every schedule. | Library model and differential simulated schedules; once call checking is covered by programs. |
| MODE-4 | Actual movement of task ownership rather than copied counted graphs. | RC model and instrumented task-transfer traces, including regions and Shared. |
| MODE-5 | Capture modes/authority in erased or separately compiled closure types remain sound. | Typed-IR/type-signature snapshots and modes proof; source construction/use cases are covered. |
| MODE-6 | Trustworthiness of a library's global/scoped capability declaration. | Library-signature audit and capability soundness model; v20 ratifies @capability(global/local) declarations. |
| MODE-7 | No hidden body inference changes calling conventions or exported defaults. | Compare signature/typed-IR snapshots across random topologies and bodies with the same declared signature (LANG-6 v20 defaults). |

Diagnostic attribution and context-directed capture spelling are ratified in v20; the remaining entries above are model or cost guarantees.

V20 checks GEN-1 sendable against complete actual mode-bearing argument values before parameter weakening. Plain/flat/unique/closure/iterator twins cover that decision; boolean and match spellings follow the normative grammar.
