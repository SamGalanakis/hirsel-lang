//! Build-only std coverage. Wrappers live in the declaration's own module, so
//! private type names, labels, brands and passing modes retain their meaning.
use super::{Backend, OptLevel, Options, Partition, TargetSpec, build, emit_backend_objects};
use hirsel_engine::{DefinitionQueries, PackageDeclarations};
use hirsel_syntax::{DefPath, FileContext, GenericKind, ItemKind, PassingMode, Visibility};
use std::{collections::BTreeMap, path::Path};

/// The signature of the function a compile-time generator's `Body[fn(A, mut B)
/// -> R uses {..}]` result generates, with named parameters: `(a0: A, mut a1:
/// B) -> R uses {..}`.
fn generated_header(result: &str) -> Option<String> {
    let body = &result[result.find("Body[fn(")? + "Body[fn(".len()..];
    let (mut depth, mut arguments, mut current) = (0usize, vec![], String::new());
    let mut rest = None;
    for (at, c) in body.char_indices() {
        match c {
            '(' | '[' | '{' => depth += 1,
            ')' if depth == 0 => {
                rest = Some(&body[at + 1..]);
                break;
            }
            ')' | ']' | '}' => depth -= 1,
            ',' if depth == 0 => {
                arguments.push(std::mem::take(&mut current));
                continue;
            }
            _ => {}
        }
        current.push(c);
    }
    if !current.trim().is_empty() {
        arguments.push(current);
    }
    // The Body's closing bracket ends the generated function's type.
    let rest = rest?;
    let mut depth = 0usize;
    let end = rest.char_indices().find_map(|(at, c)| match c {
        '(' | '[' | '{' => {
            depth += 1;
            None
        }
        ']' if depth == 0 => Some(at),
        ')' | ']' | '}' => {
            depth -= 1;
            None
        }
        _ => None,
    })?;
    let parameters: Vec<_> = arguments
        .iter()
        .enumerate()
        .map(|(i, a)| {
            let a = a.trim();
            match a
                .split_once(' ')
                .filter(|(mode, _)| matches!(*mode, "mut" | "take"))
            {
                Some((mode, ty)) => format!("{mode} a{i}: {}", ty.trim()),
                None => format!("a{i}: {a}"),
            }
        })
        .collect();
    Some(format!(
        "({}){}",
        parameters.join(", "),
        rest[..end].trim_end()
    ))
}

/// The representative nominal type compile-time generators derive bodies for.
const SAMPLE: &str = "type NativeCoverageSample { count: u64, name: String, flag: bool }\n";

pub struct StdSurface {
    package: PackageDeclarations,
    pub modules: BTreeMap<String, Vec<DefPath>>,
    wrappers: BTreeMap<Vec<String>, String>,
    /// Functions executed by comptime, which have no runtime ABI.
    pub compile_time: Vec<String>,
}

impl StdSurface {
    pub fn load(manifest: &Path) -> Result<Self, String> {
        let mut package = crate::COMPILER
            .load_package_declarations(manifest)
            .map_err(|e| e.to_string())?;
        let mut modules: BTreeMap<String, Vec<DefPath>> = BTreeMap::new();
        let mut wrappers: BTreeMap<Vec<String>, String> = BTreeMap::new();
        let mut compile_time = vec![];
        for (file, context) in package.files.clone() {
            if context.package != package.package {
                continue;
            }
            let syntax = package
                .engine
                .parse_file(file, &context)
                .map_err(|e| format!("{e:?}"))?
                .value;
            let source = package.engine.sources.file(file).ok_or("missing source")?;
            for id in &syntax.roots {
                let item = &syntax.tree.items[id.0 as usize];
                let ItemKind::Function {
                    signature,
                    body: Some(_),
                    ..
                } = &item.kind
                else {
                    continue;
                };
                if item.visibility != Visibility::Public {
                    continue;
                }
                let module = context.modules.join(".");
                let roots = modules.entry(module).or_default();
                if signature.generics.is_empty() {
                    roots.push(item.definition.clone());
                    continue;
                }
                // Type parameters become `type_arg`, size parameters 4.
                let ty_with = |id: hirsel_syntax::TypeExprId, type_arg: &str| {
                    let substitutions: BTreeMap<_, _> = signature
                        .generics
                        .iter()
                        .map(|g| {
                            (
                                g.name.as_str(),
                                match g.kind {
                                    GenericKind::Size => "4".to_string(),
                                    GenericKind::Type(_) => type_arg.to_string(),
                                },
                            )
                        })
                        .collect();
                    let span = syntax.tree.types[id.0 as usize].origin.span;
                    let mut text = String::new();
                    let mut start = span.start as usize;
                    for token in &syntax.tokens {
                        if token.span.start < span.start || token.span.end > span.end {
                            continue;
                        }
                        let from = token.span.start as usize;
                        let to = token.span.end as usize;
                        if let Some(replacement) = substitutions.get(&source.text()[from..to]) {
                            text.push_str(&source.text()[start..from]);
                            text.push_str(replacement);
                            start = to;
                        }
                    }
                    text.push_str(&source.text()[start..span.end as usize]);
                    text
                };
                let ty = |id| ty_with(id, "u64");
                let parameters: Vec<_> = signature
                    .parameters
                    .iter()
                    .enumerate()
                    .map(|(i, p)| {
                        let mode = match p.passing {
                            PassingMode::Mut => "mut ",
                            PassingMode::Take => "take ",
                            _ => "",
                        };
                        let name = p
                            .name
                            .as_ref()
                            .map_or_else(|| format!("unused_{i}"), |n| n.to_string());
                        let label = p.label.as_ref().map_or(String::new(), |l| format!("{l} "));
                        (
                            format!("{label}{mode}{name}: {}", ty(p.ty)),
                            format!(
                                "{}{mode}{name}",
                                p.label.as_ref().map_or(String::new(), |l| format!("{l}: "))
                            ),
                        )
                    })
                    .collect();
                let result = signature.result.map_or("()".into(), ty);
                if parameters.iter().any(|(p, _)| p.contains("TypeShape["))
                    || result.contains("Body[")
                {
                    // Only nominal records and variants have a shape (GEN-3): the
                    // generated body's own signature, at the representative record.
                    let header = signature
                        .result
                        .and_then(|id| generated_header(&ty_with(id, "NativeCoverageSample")))
                        .ok_or_else(|| {
                            format!(
                                "{}: a compile-time generator's result names no Body[fn(..) -> ..]",
                                item.definition
                            )
                        })?;
                    let name = format!("native_coverage_{}", item.name);
                    let surface = wrappers.entry(context.modules.clone()).or_default();
                    if !surface.contains(SAMPLE) {
                        surface.push_str(SAMPLE);
                    }
                    surface.push_str(&format!(
                        "fn {name}{header} = comptime {}(shape(NativeCoverageSample))\n",
                        item.name
                    ));
                    let mut root = item.definition.clone();
                    root.0.name = name;
                    roots.push(root);
                    compile_time.push(item.definition.to_string());
                    continue;
                }
                let name = format!("native_coverage_{}", item.name);
                let wrapper = format!(
                    "fn {name}({}) -> {result} {{ {}({}) }}\n",
                    parameters
                        .iter()
                        .map(|(p, _)| p.as_str())
                        .collect::<Vec<_>>()
                        .join(", "),
                    item.name,
                    parameters
                        .iter()
                        .map(|(_, a)| a.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                );
                wrappers
                    .entry(context.modules.clone())
                    .or_default()
                    .push_str(&wrapper);
                let mut root = item.definition.clone();
                root.0.name = name.into();
                roots.push(root);
            }
        }
        Ok(Self {
            package,
            modules,
            wrappers,
            compile_time,
        })
    }

    /// Every native stage validator runs at build; both backend emitters run at O0.
    /// Objects are discarded. There is no linker or runtime launch.
    pub fn compile(&mut self, module: &str) -> Result<usize, String> {
        let modules = module.split('.').map(str::to_owned).collect::<Vec<_>>();
        if let Some(source) = self.wrappers.remove(&modules) {
            let file = self
                .package
                .engine
                .sources
                .add(format!("native-surface/{module}.hir"), source)
                .ok_or("surface too large")?;
            let context = FileContext {
                package: self.package.package.clone(),
                modules,
            };
            let parsed = self
                .package
                .engine
                .parse_file(file, &context)
                .map_err(|e| {
                    format!(
                        "{e:?}\n{}",
                        self.package.engine.sources.file(file).unwrap().text()
                    )
                })?;
            if !parsed.diagnostics.is_empty() {
                return Err(format!("surface syntax: {:?}", parsed.diagnostics));
            }
        }
        let roots = self.modules.get(module).ok_or("unknown std module")?;
        let input = self
            .package
            .engine
            .native_input_for(roots)
            .map_err(|e| format!("{e:?}"))?;
        let program = build(
            &input,
            &Options::new(TargetSpec::x86_64_unknown_linux_gnu()),
        )
        .map_err(|e| e.to_string())?;
        let mut errors = vec![];
        for backend in [Backend::Cranelift, Backend::Llvm] {
            if let Err(error) = emit_backend_objects(
                &program,
                backend,
                OptLevel::None,
                Partition::Whole,
                false,
                None,
            ) {
                errors.push(format!("{}: {error}", backend.name()));
            }
        }
        if errors.is_empty() {
            Ok(program.units.len())
        } else {
            Err(errors.join("\n"))
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_generated_body_type_becomes_a_named_signature() {
        let result = "Result[\n  Body[fn(S, mut List[u8], u64) -> Result[(), json.JsonError] uses {}],\n  String\n]";
        assert_eq!(
            super::generated_header(result).unwrap(),
            "(a0: S, mut a1: List[u8], a2: u64) -> Result[(), json.JsonError] uses {}"
        );
        assert_eq!(
            super::generated_header("Result[Body[fn(S) -> u64 uses {}], String]").unwrap(),
            "(a0: S) -> u64 uses {}"
        );
        assert_eq!(super::generated_header("Result[u64, String]"), None);
    }
}
