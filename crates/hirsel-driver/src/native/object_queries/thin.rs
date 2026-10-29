//! Freeze the complete external member closure of an LLVM thin archive.
use crate::native::Toolchain;
use hirsel_engine::store::ResultStore;
use hirsel_engine::store::{Action, Address, Domain, Reference};
use hirsel_syntax::ContentHash;
use std::path::{Path, PathBuf};
// Prior art: llvm-project 2b6e769dd3add8bbd8d8be60e59f8d7e5f30ec54,
// llvm/tools/llvm-ar/llvm-ar.cpp:799-815,1008-1021: normal archives retain
// basename member names and quick append preserves input order/multiplicity.
// Object's existing checked archive reader resolves the thin member inventory;
// LLVM writes the actual format and symbol index. No custom archive codec.
pub fn freeze_thin_archive(
    store: &dyn ResultStore,
    tools: &Toolchain,
    path: &Path,
    directory: &Path,
) -> Result<PathBuf, String> {
    use std::os::unix::ffi::OsStrExt;
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    let archive =
        object::read::archive::ArchiveFile::parse(bytes.as_slice()).map_err(|e| e.to_string())?;
    if !archive.is_thin() {
        return Err("expected an admitted thin archive".into());
    }
    let mut members = Vec::new();
    for member in archive.members() {
        let member = member.map_err(|e| e.to_string())?;
        if !member.is_thin() {
            return Err("unexpected embedded thin archive member".into());
        }
        let name = std::ffi::OsStr::from_bytes(member.name());
        let source = path
            .parent()
            .ok_or("thin archive has no parent")?
            .join(name);
        let file = std::fs::File::open(&source)
            .map_err(|e| format!("thin archive member {}: {e}", source.display()))?;
        if !file.metadata().map_err(|e| e.to_string())?.is_file() {
            return Err("thin member is not a regular file".into());
        }
        let reference = crate::native::object_queries::store_link_input_reader(store, file)?;
        members.push((member.name().to_vec(), reference));
    }
    let tree = tools
        .lld
        .parent()
        .and_then(Path::parent)
        .ok_or("archiver has no installed tree")?;
    let transformation =
        crate::native::link::runtime_derivation(tree, "llvm-ar").map_err(|e| e.to_string())?;
    let encoding =
        serde_json::to_vec(&(1u32, transformation, &members)).map_err(|e| e.to_string())?;
    let domain = Domain {
        kind: 0x4f00_0009,
        schema: 1,
    };
    let address = Address(ContentHash::of_parts(
        "hirsel.native.thin-archive-address.v1",
        [encoding.as_slice()],
    ));
    let action = Action(address.0);
    let output = directory.join("archive.a");
    if let Some(row) = store.lookup_product(address)
        && row.action == action
        && row.reference.domain == domain
        && let Some(record) = store.get_content(row.reference)
        && domain.reference(&record) == Some(row.reference)
        && let Ok(reference) = serde_json::from_slice::<Reference>(&record)
        && crate::native::object_queries::materialize_link_input(store, reference, &output, false)
            .is_ok()
    {
        return Ok(output);
    }
    std::fs::create_dir_all(directory).map_err(|e| e.to_string())?;
    match std::fs::remove_file(&output) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.to_string()),
    }
    let mut inputs = Vec::new();
    for (index, (name, reference)) in members.iter().enumerate() {
        let basename = Path::new(std::ffi::OsStr::from_bytes(name))
            .file_name()
            .ok_or("thin member has no filename")?;
        let destination = directory
            .join("members")
            .join(index.to_string())
            .join(basename);
        crate::native::object_queries::materialize_link_input(
            store,
            *reference,
            &destination,
            false,
        )?;
        inputs.push(destination);
    }
    // Bound each argv independently of member count. q appends duplicates in
    // exact order; the final s operation writes LLVM's ordinary symbol index.
    if inputs.is_empty() {
        let mut command = archiver(tree);
        crate::native::link::checked(
            crate::native::StageName::Link,
            command.args(["qcD", "--format=gnu"]).arg(&output),
        )
        .map_err(|e| e.to_string())?;
    } else {
        for group in inputs.chunks(64) {
            let mut command = archiver(tree);
            crate::native::link::checked(
                crate::native::StageName::Link,
                command
                    .args(["qcDS", "--format=gnu"])
                    .arg(&output)
                    .args(group),
            )
            .map_err(|e| e.to_string())?;
        }
        let mut command = archiver(tree);
        crate::native::link::checked(
            crate::native::StageName::Link,
            command.arg("sD").arg(&output),
        )
        .map_err(|e| e.to_string())?;
    }
    let reference = crate::native::object_queries::store_link_input_reader(
        store,
        std::fs::File::open(&output).map_err(|e| e.to_string())?,
    )?;
    let bytes = serde_json::to_vec(&reference).map_err(|e| e.to_string())?;
    let record = store
        .put_content(domain, &bytes)
        .ok_or("normalized archive publication failed")?;
    if domain.reference(&bytes) != Some(record) {
        return Err("normalized archive identity differs".into());
    }
    store.associate_product(address, action, record);
    if store
        .lookup_product(address)
        .is_none_or(|row| row.action != action || row.reference != record)
    {
        return Err("normalized archive association failed".into());
    }
    Ok(output)
}
fn archiver(tree: &Path) -> std::process::Command {
    let mut command = std::process::Command::new(tree.join("bin/llvm-ar"));
    command.env_clear().env("LD_LIBRARY_PATH", tree.join("lib"));
    command
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native::TargetSpec;
    use crate::native::{OptLevel, RuntimeBuild};
    use std::sync::Arc;

    fn object(target: &TargetSpec, name: &str, value: u64) -> Vec<u8> {
        hirsel_llvm::compile_ir(
            &format!(
                "target triple = \"{}\"\ndefine i64 @{name}() {{ ret i64 {value} }}",
                target.triple
            ),
            target,
            OptLevel::None,
        )
        .unwrap()
        .bytes
    }
    #[test]
    fn frozen_thin_archives_preserve_duplicate_members_symbols_and_real_link_inputs() {
        hirsel_engine::with_compiler_stack(|| {
            let root = tempfile::tempdir().unwrap();
            let target = TargetSpec::x86_64_unknown_linux_gnu();
            let store = Arc::new(hirsel_engine::store::DirStore::new(
                root.path().join("products"),
            ));
            let tools = Toolchain::in_tree_with_products(
                &root.path().join("runtime"),
                &[&target],
                &[RuntimeBuild::default()],
                None,
                store.clone(),
            )
            .unwrap();
            let tree = tools.lld.parent().unwrap().parent().unwrap();
            let source = root.path().join("original with spaces");
            let first = source.join("first/member.o");
            let second = source.join("second/member.o");
            std::fs::create_dir_all(first.parent().unwrap()).unwrap();
            std::fs::create_dir_all(second.parent().unwrap()).unwrap();
            std::fs::write(&first, object(&target, "first", 20)).unwrap();
            std::fs::write(&second, object(&target, "second", 22)).unwrap();
            let thin = source.join("thin.a");
            let mut command = archiver(tree);
            crate::native::link::checked(
                crate::native::StageName::Link,
                command.current_dir(&source).args([
                    "qcTD",
                    "thin.a",
                    "first/member.o",
                    "second/member.o",
                ]),
            )
            .unwrap();
            let header = std::fs::read(&thin).unwrap();
            let frozen =
                freeze_thin_archive(store.as_ref(), &tools, &thin, &root.path().join("one"))
                    .unwrap();
            let bytes = std::fs::read(&frozen).unwrap();
            let parsed = object::read::archive::ArchiveFile::parse(bytes.as_slice()).unwrap();
            assert!(!parsed.is_thin());
            assert_eq!(
                parsed
                    .members()
                    .map(|member| member.unwrap().name().to_vec())
                    .collect::<Vec<_>>(),
                vec![b"member.o".to_vec(), b"member.o".to_vec()]
            );
            let symbols = parsed
                .symbols()
                .unwrap()
                .unwrap()
                .map(|symbol| symbol.unwrap().name().to_vec())
                .collect::<std::collections::BTreeSet<_>>();
            assert!(
                symbols.contains(b"first".as_slice()) && symbols.contains(b"second".as_slice())
            );
            let repeated =
                freeze_thin_archive(store.as_ref(), &tools, &thin, &root.path().join("two"))
                    .unwrap();
            assert_eq!(std::fs::read(repeated).unwrap(), bytes);
            // Same thin header, edited external member: a different ordinary
            // transformation output, without any stale-header/path-only hit.
            std::fs::write(&first, object(&target, "first", 21)).unwrap();
            assert_eq!(std::fs::read(&thin).unwrap(), header);
            let changed =
                freeze_thin_archive(store.as_ref(), &tools, &thin, &root.path().join("changed"))
                    .unwrap();
            assert_ne!(std::fs::read(&changed).unwrap(), bytes);
            std::fs::remove_file(&second).unwrap();
            assert!(
                freeze_thin_archive(store.as_ref(), &tools, &thin, &root.path().join("missing"))
                    .is_err()
            );
            std::fs::remove_dir_all(&source).unwrap();
            let caller = hirsel_llvm::compile_ir(
                &format!("target triple = \"{}\"\ndeclare i64 @first()\ndeclare i64 @second()\ndefine i32 @main() {{\n %a = call i64 @first()\n %b = call i64 @second()\n %sum = add i64 %a, %b\n %exit = trunc i64 %sum to i32\n ret i32 %exit\n}}", target.triple),
                &target, OptLevel::None,
            ).unwrap();
            let caller_path = root.path().join("caller.o");
            std::fs::write(&caller_path, caller.bytes).unwrap();
            for (archive, expected) in [(&frozen, 42), (&changed, 43)] {
                let executable = root.path().join(format!("program-{expected}"));
                let kit = crate::native::kit::Kit::load(&tools.kit).unwrap();
                let objects = [caller_path.clone()];
                let foreign = [archive.clone()];
                let link = crate::native::kit::Link {
                    kit: &kit,
                    triple: &target.triple,
                    kind: crate::native::kit::Kind::Executable { loader: None },
                    objects: &objects,
                    runtime: None,
                    startup_order: None,
                    unwind: false,
                    foreign: &foreign,
                };
                let (mut command, _) = link.command(&tools.lld, &executable).unwrap();
                crate::native::link::checked(crate::native::StageName::Link, &mut command).unwrap();
                assert_eq!(
                    std::process::Command::new(executable)
                        .status()
                        .unwrap()
                        .code(),
                    Some(expected)
                );
            }
        });
    }
}
