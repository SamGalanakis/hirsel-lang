//! Actual serial candidate versus independently materialized cold trees.
//! No skipped resident switch is evidence for this path.
use super::{Artifacts, Config};
use hirsel_driver::candidate::{
    BaseLease, ByteEdit, CandidateId, CandidateRequest, CandidateResult, CandidateWorkspace,
    Completion, FileEdit, Limits as TransactionLimits, Priority, Verdict,
};
use hirsel_engine::{CheckCompletion, CompileConfig, LoweringPolicy, Verify};
use std::path::Path;

/// Produce ordinary Comptime check artifacts through the disk loader, without
/// any FrozenInputs adapter or eligible result reuse. The caller independently
/// materializes this tree (Scenario::write), not from a candidate input map.
pub fn cold(path: &Path, config: &Config) -> Result<Artifacts, String> {
    let compiler = compiler(config);
    hirsel_engine::with_compiler_stack(|| {
        let mut program = compiler
            .load_program_source_with_images(path, None, &hirsel_engine::StdImages::none())
            .map_err(|e| format!("load: {e}"))?;
        let scope: Vec<_> = program
            .files
            .iter()
            .map(|(_, context)| context.package.clone())
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect();
        let output = program
            .check(LoweringPolicy::Comptime)
            .map_err(|e| format!("check: {}", program.error(e)))?;
        let incomplete = output.completion != CheckCompletion::Complete
            || program.engine.last_comptime_limit.is_some();
        let rejecting = output
            .diagnostics
            .iter()
            .any(|d| d.severity == hirsel_diag::Severity::Error);
        let products = if incomplete {
            None
        } else {
            Some(
                program
                    .engine
                    .checked_products(&program.entry)
                    .map_err(|e| format!("products: {}", program.error(e)))?,
            )
        };
        Ok(artifacts(
            &output,
            if incomplete {
                "incomplete"
            } else if rejecting {
                "reject"
            } else {
                "pass"
            },
            products.as_ref(),
            path,
            &scope,
        ))
    })
}
pub fn workspace(config: &Config) -> Result<CandidateWorkspace, String> {
    CandidateWorkspace::new(
        compile_config(config),
        vec![],
        limits(config),
        TransactionLimits {
            retain_canonical_export: true,
            ..TransactionLimits::default()
        },
    )
    .map_err(|e| format!("workspace: {e:?}"))
}
fn compile_config(config: &Config) -> CompileConfig {
    CompileConfig {
        jobs: hirsel_intern::jobs::Jobs::new(config.jobs),
        verify: Verify::On,
        lowering: LoweringPolicy::Comptime,
        ..CompileConfig::for_memory_cap(None)
    }
}
fn limits(config: &Config) -> hirsel_interp::Limits {
    config
        .limits
        .map_or_else(hirsel_interp::Limits::default, |l| hirsel_interp::Limits {
            steps: l.steps,
            call_depth: l.call_depth,
        })
}
fn compiler(config: &Config) -> hirsel_engine::Compiler {
    hirsel_driver::COMPILER
        .clone()
        .with_config(compile_config(config))
        .with_comptime_limits(limits(config))
        .with_owned_engines()
}
/// Patches only sources that already existed in the captured source set.
pub fn submit(
    w: &CandidateWorkspace,
    base: &BaseLease,
    id: u64,
    after: &std::collections::BTreeMap<String, String>,
) -> Result<hirsel_driver::candidate::CandidateLease, String> {
    let mut edits = vec![];
    for (key, old, expected) in base.sources() {
        let path = base.source_path(key).ok_or("unknown source key")?;
        let relative = path
            .strip_prefix(base.entry().parent().unwrap())
            .map_err(|_| "oracle source outside materialized tree")?;
        let name = relative.to_str().ok_or("non-UTF8 oracle source")?;
        let Some(text) = after.get(name) else {
            continue;
        };
        if text != old {
            edits.push(FileEdit {
                file: key.clone(),
                expected,
                edits: vec![ByteEdit {
                    start: 0,
                    end: old.len() as u32,
                    replacement: text.clone(),
                }],
            });
        }
    }
    w.submit(
        base,
        CandidateRequest {
            id: CandidateId {
                client: 1,
                nonce: id,
            },
            base: base.token().clone(),
            edits,
            priority: Priority::Selected,
        },
    )
    .map_err(|e| format!("submit: {e:?}"))
}
pub fn result(
    w: &CandidateWorkspace,
    lease: &hirsel_driver::candidate::CandidateLease,
    entry: &Path,
) -> Result<Artifacts, String> {
    let CandidateResult::Ready(receipt) = w.result(lease).map_err(|e| format!("result: {e:?}"))?
    else {
        return Err("serial candidate unexpectedly pending".into());
    };
    if let Some(error) = &receipt.loader_error {
        return Err(error.clone());
    }
    let state = match (&receipt.completion, receipt.verdict) {
        (Completion::Complete, Verdict::Pass) => "pass",
        (Completion::Complete, Verdict::Reject) => "reject",
        _ => "incomplete",
    };
    Ok(artifacts(
        &receipt.output,
        state,
        receipt.checked_products.as_deref(),
        entry,
        &receipt.scope.packages,
    ))
}
fn artifacts(
    output: &hirsel_engine::CheckOutput,
    state: &str,
    products: Option<&hirsel_engine::CheckedProducts>,
    entry: &Path,
    scope: &[hirsel_syntax::PackageId],
) -> Artifacts {
    // Only the physical private-tree root differs. Preserve ordered JSON/human
    // primary/secondary spans and text verbatim beyond that root mapping.
    let root = entry.parent().unwrap().to_string_lossy();
    let json = output
        .diagnostics
        .iter()
        .map(|d| d.to_json(&output.sources).to_string())
        .collect::<Vec<_>>()
        .join("\n")
        .replace(root.as_ref(), "$TREE");
    let human = output
        .diagnostics
        .iter()
        .map(|d| d.render_human(&output.sources))
        .collect::<Vec<_>>()
        .join("\n")
        .replace(root.as_ref(), "$TREE");
    let mut out = Artifacts::from([
        (
            "scope".into(),
            format!("Verify::On/Comptime/{scope:?}").into_bytes(),
        ),
        (
            "unsafe-ffi".into(),
            format!("{:?}", output.unsafe_ffi)
                .replace(root.as_ref(), "$TREE")
                .into_bytes(),
        ),
        ("state".into(), state.as_bytes().to_vec()),
        ("diagnostics/json".into(), json.into_bytes()),
        ("diagnostics/human".into(), human.into_bytes()),
    ]);
    if let Some(products) = products {
        out.insert(
            "checked-products/canonical".into(),
            products
                .canonical
                .as_ref()
                .expect("oracle requires retained canonical export")
                .as_bytes()
                .to_vec(),
        );
        out.insert("checked-products/digest".into(), products.digest.0.to_vec());
        let mut bytes = vec![];
        for (path, reads) in &products.reads {
            hirsel_engine::memo::encode_def_path(&mut bytes, path);
            bytes.extend_from_slice(&(reads.len() as u64).to_le_bytes());
            for (key, value) in reads {
                bytes.extend_from_slice(&(key.len() as u64).to_le_bytes());
                bytes.extend_from_slice(key);
                bytes.extend_from_slice(&value.0);
            }
        }
        out.insert("checking-reads".into(), bytes);
    }
    out
}

/// One jobs/salt cell, in its own process before ids exist. Each oracle tree
/// is independently written by Scenario; every cold run is a separate process.
pub fn matrix_cell(root: &Path, config: &Config, work: &Path, exe: &Path) -> Result<(), String> {
    use super::{SCENARIOS, edits::Scenario};
    let names = [
        "body-edit",
        "signature-private",
        "signature-public",
        "definition-add",
        "definition-remove",
        "generator-helper-body",
        "reflected-helper-lookup",
        "secondary-diagnostic-site",
        "duplicate-insert-file",
        "duplicate-remove-file",
        "edit-above-item",
        "opaque-representation",
    ];
    for name in names {
        let scenario = Scenario::read(
            &root.join(SCENARIOS).join("o2").join(format!("{name}.edit")),
            root,
        )?;
        let before = work.join(name).join("base");
        let after = work.join(name).join("cold");
        let artifact_dir = work.join(name).join("oracle");
        scenario.write(&before, root, false)?;
        scenario.write(&after, root, true)?;
        let workspace = workspace(config)?;
        let base = workspace
            .capture(&scenario.entry(&before))
            .map_err(|e| format!("capture: {e:?}"))?;
        let lease = submit(&workspace, &base, 1, &scenario.after)?;
        let actual = result(&workspace, &lease, &scenario.entry(&before))?;
        let mut command = std::process::Command::new(exe);
        command
            .arg("cold-check")
            .args(config.args())
            .arg("--out")
            .arg(&artifact_dir)
            .arg(scenario.entry(&after));
        let child = command.output().map_err(|e| e.to_string())?;
        if !child.status.success() {
            return Err(format!(
                "{name}: cold failed: {}",
                String::from_utf8_lossy(&child.stderr)
            ));
        }
        let expected = super::read_artifacts(&artifact_dir).map_err(|e| e.to_string())?;
        if let Some(why) = super::difference(&expected, &actual) {
            return Err(format!("{name}: {why}"));
        }
        super::write_artifacts(&work.join(name).join("candidate"), &actual)
            .map_err(|e| e.to_string())?;
        if workspace.counters().frozen_loads != 1 {
            return Err(format!("{name}: candidate path did not execute"));
        }
        // Mutation oracle: every artifact mismatch is detected, including read
        // and TIR/private-view payload swaps. Nothing is normalized away.
        for key in expected.keys() {
            let mut corrupt = actual.clone();
            corrupt.get_mut(key).unwrap().push(0);
            if super::difference(&expected, &corrupt).is_none() {
                return Err(format!("undetected corruption in {key}"));
            }
        }
    }
    let completed = names.len() + eight_candidates(root, config, work, exe)?;
    std::fs::write(
        work.join("counts.txt"),
        format!(
            "completed={} jobs={} salt={:?}\n",
            completed, config.jobs, config.salt
        ),
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

fn eight_candidates(
    root: &Path,
    config: &Config,
    work: &Path,
    exe: &Path,
) -> Result<usize, String> {
    use super::{SCENARIOS, edits::Scenario};
    let scenario = Scenario::read(&root.join(SCENARIOS).join("o2/body-edit.edit"), root)?;
    let before = work.join("eight/base");
    scenario.write(&before, root, false)?;
    let original = format!(
        "{}\ntype Secret {{ level:u64 }}\nfn view(x:Secret)->u64{{x.level}}\n\
        fn missing_user()->u64{{formerly_missing()}}\n\
        fn source()->String{{\"fn(x: u64) -> u64 uses {{}} {{ x + 1 }}\"}}\n\
        fn body_text()->Result[Body[fn(u64)->u64 uses {{}}],String]{{Ok(codegen.body(source()))}}\n\
        fn bumped(n:u64)->u64 uses {{}} = comptime body_text()\n",
        scenario.before["main.hir"]
    );
    std::fs::write(before.join("main.hir"), &original).map_err(|e| e.to_string())?;
    let w = workspace(config)?;
    let base = w
        .capture(&scenario.entry(&before))
        .map_err(|e| format!("eight capture: {e:?}"))?;
    let literal = original.replacen("x + 1", "x + 2", 1);
    let variants = [
        literal.clone(),
        literal.clone(),
        original.replace("level:u64", "level:u32"),
        original
            .replace("level:u64", "level:u32")
            .replace("view(x:Secret)->u64", "view(x:Secret)->u32"),
        format!("{original}\nfn formerly_missing()->u64{{9}}\n"),
        original.replace("x + 1 }\"", "x + 5 }\""),
        original
            .replace(
                "fn callee(x: u64)",
                "// shifted anchor\n\nfn callee(x: u64)",
            )
            .replace("callee(x) * 2", "callee(true) * 2"),
        literal,
    ];
    let mut leases = vec![];
    for (index, text) in variants.iter().enumerate() {
        leases.push(submit(
            &w,
            &base,
            index as u64 + 1,
            &std::collections::BTreeMap::from([("main.hir".into(), text.clone())]),
        )?);
    }
    w.cancel(&leases[7]).map_err(|e| format!("cancel: {e:?}"))?;
    for index in (0..7).rev() {
        let actual = result(&w, &leases[index], &scenario.entry(&before))?;
        let after = work.join(format!("eight/cold-{index}"));
        scenario.write(&after, root, false)?;
        std::fs::write(after.join("main.hir"), &variants[index]).map_err(|e| e.to_string())?;
        let artifacts = work.join(format!("eight/oracle-{index}"));
        let child = std::process::Command::new(exe)
            .arg("cold-check")
            .args(config.args())
            .arg("--out")
            .arg(&artifacts)
            .arg(scenario.entry(&after))
            .output()
            .map_err(|e| e.to_string())?;
        if !child.status.success() {
            return Err(format!(
                "eight C{} cold: {}",
                index + 1,
                String::from_utf8_lossy(&child.stderr)
            ));
        }
        let expected = super::read_artifacts(&artifacts).map_err(|e| e.to_string())?;
        if let Some(why) = super::difference(&expected, &actual) {
            return Err(format!("eight C{}: {why}", index + 1));
        }
    }
    let CandidateResult::Ready(cancelled) =
        w.result(&leases[7]).map_err(|e| format!("result: {e:?}"))?
    else {
        return Err("cancelled waiter pending".into());
    };
    if cancelled.completion
        != Completion::Incomplete(hirsel_driver::candidate::IncompleteReason::Cancelled)
    {
        return Err("cancelled C8 got a verdict".into());
    }
    if w.counters().frozen_loads != 6 {
        return Err(format!(
            "eight batch expected6 unique frozen loads, got{}",
            w.counters().frozen_loads
        ));
    }
    if std::fs::read_to_string(before.join("main.hir")).map_err(|e| e.to_string())? != original {
        return Err("candidate wrote base source".into());
    }
    Ok(7)
}
