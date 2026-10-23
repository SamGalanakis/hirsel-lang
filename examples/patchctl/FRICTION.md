# patchctl authoring record

The first complete source attempt checked successfully on frozen main
`f36e8f4b04d616e24f63e1b74fa4ba97bd46f466`. No checker, std, language rule,
runner or ABI change was needed. This records the enumerated forms below,
not universal ergonomic coverage. Raw sources, commands, JSON checks, hashes
and failed setup logs are retained outside the repository in the lane evidence.

| Attempt / minimal form | Observed diagnostic or rule/span | Repair or final recipe | Investigation / disposition |
| --- | --- | --- | --- |
| `let command: model.Command = match arg.text(args, 0)? { "diff" => Diff(), "apply" => Apply(), _ => return Error(InvalidInput(Usage())) }` | Accepted; no diagnostic/span | Keep the expected enum type explicit and use borrowed strings in `model.execute` | ~2 min; intentional LANG-2 prefix context, annotation retained |
| `string.split(unique value, ",")`, `string.split(unique line, " ")` | Accepted; no diagnostic/span | `split` takes an owned input; deep-copy the borrowed parameter/line with `unique` | ~2 min; intentional ownership contract, two copies retained |
| `string.from_bytes(unique start_text)` | Accepted; no diagnostic/span | Byte slices borrow their source; make an owned copy before UTF-8 conversion | ~1 min; intentional ownership contract, copy retained |
| `Error(Rejected(unique patched.rejected))` and `Ok(Report(text: unique patched.text))` | Accepted; no diagnostic/span | Copy record fields into global result payloads; retain std's record while inspecting its rejection list | ~2 min; intentional locality recipe, two copies retained |
| `diff.apply(source, other) ? MalformedPatch` with `fn MalformedPatch(error: diff.PatchError) -> InvalidInput { match error { Malformed(line) => Malformed(line) } }` | Accepted; no diagnostic/span | Named unary mapper preserves std's one-based line in the application's typed channel | ~2 min; intentional LANG-8 wrapper/helper retained |
| `AppError = Grant(GrantError) \| Arguments(ArgError) \| Files(FileError) \| InvalidInput(model.InvalidInput) \| Output(OutputError)` | Accepted; no diagnostic/span | Typed wrappers permit ordinary `?`; bounded-read `Limit` is mapped explicitly to `BytesLimit`, while `File(error)` retains the original error | ~3 min; intentional error-channel plumbing retained |
| `file.read_all_bounded(d, mut f, model.INPUT_BYTES)` | Accepted; no diagnostic/span | Thread both the `Dir` and its branded file through the existing API; no new capability helper | ~1 min; intentional CAP-2/brand plumbing retained |
| `process.exit_code(code)` after explicit stderr rendering | Accepted; no diagnostic/span | Return `Ok(ExitCode)` to distinguish rejected hunks (1) from invalid input (2); stderr is optional on failure | ~2 min; intentional process API recipe retained |
| `bounded(value, limit)` before `diff.split_lines`; count commas/spaces before `string.split` | Accepted; no diagnostic/span | Extra pure admission helpers bound collections and hunk coordinates. Fixed header labels avoid filename/header injection. Empty patches are explicit identity operations | ~4 min; intentional bounded application policy/helpers retained |
| First host native build without `LLVM_SYS_231_PREFIX` | Tooling error: `hirsel: native link: LLVM_SYS_231_PREFIX must name an absolute toolchain input`; no source rule/span | Set the absolute pinned LLVM prefix | ~1 min; setup issue, no engine execution claimed |
| Second host native build with LLVM but without prebuilt runtime inputs | Tooling error: `hirsel: native link: runtime cache: No such file or directory (os error 2)`; no source rule/span | Build `hirsel-rt-archives`, native `kit` and `glibc_floor`; set their documented environment inputs | ~2 min; setup issue, no engine execution claimed |
| Rust checked-H harness: `let manifest = package(...); ... { use hirsel_driver::native::*; load_program(&manifest) }` | Rust E0308 at `patchctl_23.rs:102:54`: expected `&Path`, found native `manifest` function | Rename the enclosing local to `package_manifest` | ~2 min; host-language import shadowing, renamed local retained |
| Missing directory grant: `Files(_) => "file_error"` after `io.dir(rt, path.of("fixtures"))` | Runtime assertion: status 2 / empty stdout / `patchctl: file_error`, expected `patchctl: grant`; no compiler diagnostic/span. Checked H passed; original full matrix failed at this cell | Match `Files(NotGranted()) => "grant"` explicitly. The directory projection returns `FileError`, whereas stdout/stderr projections return `GrantError` | ~3 min; intentional capability API behavior, extra error arm retained; original evidence and distinct minimal source preserved |
| First reduced grant witness used `{}` as launch policy | Launch refusal: `not_granted`, root `Io`, exit 3; no source rule/span | Keep an `io` object with stdout/stderr and empty `dirs` to enter the program, then propagate `io.dir` directly as `FileError` | ~1 min; intentional launch/root boundary, first and corrected witnesses preserved |
| First landing: blank line before `mod patchctl_23;` in `modules.rs` | `modules.rs:56: expected one independent mod declaration`; no language rule/span | Remove the blank line; keep one module declaration per line for the union merge | <1 min; repository hot-file format, no application/test input change |
| Attempted Buck target `//:hirsel-cli-test-cli` | `Unknown target hirsel-cli-test-cli`; zero tests ran | Use the required `//:hirsel-cli-test-cli-shard-1` with `patchctl_23` filter | <1 min; harness setup, not a passing test |

The application deliberately uses shipped Myers, fixed context and std's offset
search. It does not duplicate a diff algorithm. The reference's replace and
identical inputs are retained, but the oracle checks valid application instead
of requiring a unique edit script; fixed labels and three context lines are the
application policy. The stricter single-file envelope and bounded coordinates
are admission policy around std's permissive header skipping.

The integration harness also uses explicit manifest dependency paths in private
temporary packages, `include_str!` for both modules and all fixture inputs, a
Python subprocess oracle, normal native-build setup limits, and sequential
runtime processes with 60 CPU-second / 2 GiB allocation limits. Host evidence
runs additionally put each runtime process under `tools/capped`. These are test
plumbing choices, with no shared-runner changes. Cleanup and unchanged input
bytes are assertions, including after rejection.

Focused acceptance executed two Rust tests: the first executable matrix had one
passing checked-H test and one failing application test at missing-directory
authority. After the reporting repair, both tests passed (61 application cases,
198 runtime observations, plus the pure checked-H audit). The successful
unchanged cells were repeated by focus03 and the already-started external05;
these are disclosed repeats, not additional feature coverage. The original
failure remains a failed run. No open compiler/std issue was inferred from it.
