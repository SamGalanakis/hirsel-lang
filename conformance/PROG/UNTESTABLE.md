# PROG coverage requiring another oracle

Issues are in [SamGalanakis/hirsel-lang](https://github.com/SamGalanakis/hirsel-lang/issues).

- **PROG-1, topology-independent identity:** package/version/module and nominal-role fixtures now use exact `.hir.json` sidecars. A whole-program versus singleton/random-unit comparison still supplies the topology oracle; one source cannot quantify every partition. No source-level imports/modules were introduced.
- **PROG-2, visibility:** direct package-private, opaque-field and internal-ancestry controls now load ordinary dependency sources. Public signature, opaque and internal ancestry rules retain their resolver controls; opaque constructor creation in its own package is a separate existing body-shape gap; the tested abstract opaque public interface remains nameable. These are ordinary selected packages, with no #4 schema blocker.
- **PROG-3, chosen entry and launch errors:** the corpus runs `pub fn main(rt: Io, args: Args) -> Result[(), ()]` and checks other root-capability entry signatures, including Env and Unsafe. Entry selection is explicit JSON launch input; hash-seed/Io configuration and denial of capabilities remain API/simulation launch tests. Oracle: launch-configuration tests selecting each public function, denying each requested capability, and rejecting non-public/non-root/non-Result entries. An ordinary function with such a signature is legal to check, even if named `main`; therefore it is not a static rejection twin. A launch error is not the specified runtime `fail` expectation. Non-main entry checks must eventually be paired with configured runs.
- **PROG-4, native interop:** the static twins (missing package, aliasing contract, prohibited types and modes, closure callbacks and non-`mut` contexts) are in the corpus with `unsafe_ffi` sidecars. Calling C needs a native build, which the interpreter-run corpus cannot express; those runs belong to the driver's `foreign` tests, which compile a C library with the pinned toolchain. The package rules (root-only selection, wrapper entries, the interpreter refusal) are tool-input errors tested there too.
- **PROG-5, entire topology law:** no single program proves that every partition preserves acceptance, meaning and signature-defined calling conventions. Oracle: compile and run the entire corpus as single-definition units, one whole-program unit and random partitions, with interpreter/native differential checks, ABI checks across boundaries and cache-warm/cache-cold runs. Include GEN-5's growing cycles so the sole program-level check cannot silently become unit-local. Tooling settings must never alter verdicts or observables.

There are no UTF rule IDs in v20 and no UTF directory; Unicode coverage belongs to SEM-3.

V20 pins Ok/Error entry exits to 0/1. Evaluator artificial limits remain #34:
use internal API tests for limit stop sites and resource cleanup rather than
claiming a configurable instruction count is the language's Depth limit.
