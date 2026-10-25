//! Source additions selected before S5 captures its immutable front inventory.
use super::NativeSourceText;
use hirsel_engine::{DefinitionQueries, LoadedProgram, ToolError};
use std::collections::BTreeSet;

pub(super) fn install(
    program: &mut LoadedProgram,
    sources: &[NativeSourceText],
    limit: u64,
) -> Result<(), ToolError> {
    let fail = |message: &str| ToolError::Runtime(message.into());
    let mut names = BTreeSet::new();
    let mut bytes = 0u64;
    for source in sources {
        if source.context.package != program.entry.0.package {
            return Err(fail("native source addition belongs to another package"));
        }
        if !names.insert(source.name.as_str())
            || program.files.iter().any(|(file, _)| {
                program
                    .engine
                    .sources
                    .file(*file)
                    .is_some_and(|file| file.path == std::path::Path::new(&source.name))
            })
        {
            return Err(fail("native source addition repeats a source name"));
        }
        bytes = bytes
            .checked_add(source.text.len() as u64)
            .filter(|&bytes| bytes <= limit)
            .ok_or_else(|| fail("native source additions exceed snapshot admission"))?;
    }
    for source in sources {
        let file = program
            .engine
            .sources
            .add(&source.name, source.text.to_string())
            .ok_or(ToolError::SourceTooLarge)?;
        let parsed = program
            .engine
            .parse_file(file, &source.context)
            .map_err(|error| program.error(error))?;
        if !parsed.diagnostics.is_empty() {
            return Err(program.error(hirsel_engine::QueryError::Diagnostics(parsed.diagnostics)));
        }
        program.files.push((file, source.context.clone()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_source_additions_index_exact_text_without_lowering_and_refuse_bad_bindings() {
        hirsel_engine::with_compiler_stack(|| {
            let directory = tempfile::tempdir().unwrap();
            let path = directory.path().join("main.hir");
            std::fs::write(&path, "pub fn main()->u64{1u64}").unwrap();
            let mut program = crate::COMPILER.load_program(&path).unwrap();
            let context = hirsel_syntax::FileContext {
                package: program.entry.0.package.clone(),
                modules: vec![],
            };
            let mut source = NativeSourceText {
                name: "<test entry>".into(),
                context,
                text: "pub fn generated()->u64{7u64}".into(),
            };
            let mut generated = program.entry.clone();
            generated.0.name = "generated".into();
            assert!(!program.engine.indexed_definitions().contains(&generated));
            assert!(install(&mut program, &[source.clone()], 1).is_err());
            let owner = source.context.package.clone();
            source.context.package.name.push_str("_other");
            assert!(install(&mut program, &[source.clone()], 1024).is_err());
            source.context.package = owner;
            assert!(install(&mut program, &[source.clone(), source.clone()], 1024).is_err());
            install(&mut program, &[source.clone()], 1024).unwrap();
            assert!(program.engine.indexed_definitions().contains(&generated));
            assert_eq!(
                program
                    .engine
                    .recompute_count(&generated, hirsel_engine::QueryKind::Lower),
                0
            );
            let (file, installed) = program.files.last().unwrap();
            assert_eq!(installed, &source.context);
            assert_eq!(
                program.engine.sources.file(*file).unwrap().text(),
                &*source.text
            );
            assert!(install(&mut program, &[source], 1024).is_err());
        });
    }
}
