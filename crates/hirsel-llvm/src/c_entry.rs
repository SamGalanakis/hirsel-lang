//! The C view of an emitted entry, for harnesses that call one or define one. A C harness
//! never spells the lowered convention itself: it calls or implements a function of the
//! entry's ABI v1 signature (`FnAbi`: context, [environment], [caller destination],
//! [result pointer], then each declared parameter's slots, returning the u8 status), and
//! the text here bridges it to what emission lowers that signature to. The rule deciding
//! the bridge is `register_result`, the emitter's own, so a change to it rewrites every
//! harness with it instead of leaving a stale prototype to crash.

use std::fmt::Write as _;

use hirsel_lir::abi_types::{FnAbi, ParameterPassing, PhysicalType, ScalarExtension};
use hirsel_lir::layout_types::{LayoutTable, ScalarKind};

use crate::emit::{PairField, PairWord, REGISTER_PAIR, register_result};

/// The C type of a register pair field holding `word`.
fn word(word: PairWord) -> String {
    match word {
        PairWord::Int(bits) => format!("uint{bits}_t"),
        PairWord::Pointer => "void *".into(),
    }
}

/// The C typedefs of the register pairs a plain scalar result returns as, field for field
/// as emission builds them (`REGISTER_PAIR`): one for a reference and one for every other
/// scalar.
fn pairs() -> String {
    let mut c = String::from(
        "#ifndef HIRSEL_REGISTER_RESULT\n\
         #define HIRSEL_REGISTER_RESULT\n\
         #include <stdint.h>\n\
         #include <string.h>\n",
    );
    for kind in [ScalarKind::U64, ScalarKind::Pointer] {
        let fields: String = REGISTER_PAIR
            .iter()
            .map(|field| format!(" {} {};", word(field.word(kind)), field.name()))
            .collect();
        let _ = writeln!(c, "typedef struct {{{fields} }} {};", pair(kind));
    }
    c.push_str("#endif\n");
    c
}

/// The pair type of a register result of `kind`.
fn pair(kind: ScalarKind) -> &'static str {
    match kind {
        ScalarKind::Pointer => "hirsel_pointer_result",
        _ => "hirsel_word_result",
    }
}

/// The C type holding a register result of `kind`'s bits: the pointer, or an unsigned
/// integer of the value's width, the word truncated to it.
fn bits(kind: ScalarKind) -> &'static str {
    match kind {
        ScalarKind::Pointer => "void *",
        ScalarKind::Bool | ScalarKind::U8 | ScalarKind::I8 => "uint8_t",
        ScalarKind::U16 | ScalarKind::I16 => "uint16_t",
        ScalarKind::U32 | ScalarKind::I32 | ScalarKind::F32 => "uint32_t",
        _ => "uint64_t",
    }
}

/// The C type of one parameter slot. An address of a read-only parameter is `const`; a
/// narrow integer takes the signedness of its declared extension.
fn slot(ty: PhysicalType, passing: ParameterPassing, extension: ScalarExtension) -> &'static str {
    let zero = extension == ScalarExtension::Zero;
    match ty {
        PhysicalType::Pointer if passing == ParameterPassing::ReadOnlyAddress => "const void *",
        PhysicalType::Pointer => "void *",
        PhysicalType::I8 if zero => "uint8_t",
        PhysicalType::I8 => "int8_t",
        PhysicalType::I16 if zero => "uint16_t",
        PhysicalType::I16 => "int16_t",
        PhysicalType::I32 => "int32_t",
        PhysicalType::I64 => "int64_t",
        PhysicalType::I128 => "__int128",
        PhysicalType::F32 => "float",
        PhysicalType::F64 => "double",
    }
}

/// The ABI v1 parameters of `abi` as (C type, name), the result pointer included, and the
/// index of the result pointer when there is one.
fn parameters(abi: &FnAbi) -> (Vec<(&'static str, String)>, Option<usize>) {
    assert!(abi.context, "a Hirsel entry takes its run context");
    let mut out = vec![("hirsel_rt_v2_ctx *", "ctx".to_string())];
    if abi.environment {
        out.push(("void *", "environment".into()));
    }
    if abi.destination {
        out.push(("void *", "destination".into()));
    }
    let result = abi.result.is_some().then(|| {
        out.push(("void *", "out".into()));
        out.len() - 1
    });
    for (i, parameter) in abi.parameters.iter().enumerate() {
        for (j, s) in parameter.slots.iter().enumerate() {
            let ty = slot(s.ty, parameter.passing, parameter.extension);
            out.push((ty, format!("p{i}_{j}")));
        }
    }
    (out, result)
}

fn declaration(parameters: &[(&str, String)]) -> String {
    parameters
        .iter()
        .map(|(ty, name)| format!("{ty} {name}"))
        .collect::<Vec<_>>()
        .join(", ")
}

fn names<'p>(parameters: impl Iterator<Item = &'p (&'static str, String)>) -> String {
    parameters
        .map(|(_, name)| name.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

/// C text that declares the emitted entry `symbol` of `abi` as emission lowers it, and
/// defines `static uint8_t name(...)` of its ABI v1 signature calling it. A register
/// result is written through `out` on success, in the result's own width (a float by bit
/// pattern), as a result pointer would receive it; `out` is untouched on failure.
pub fn c_call(layouts: &LayoutTable, abi: &FnAbi, symbol: &str, name: &str) -> String {
    let (all, result) = parameters(abi);
    let mut c = pairs();
    match (register_result(layouts, abi), result) {
        (Some(kind), Some(out)) => {
            let lowered: Vec<_> = all
                .iter()
                .enumerate()
                .filter(|(i, _)| *i != out)
                .map(|(_, p)| p.clone())
                .collect();
            let _ = write!(
                c,
                "extern {pair} {symbol}({});\n\
                 static uint8_t {name}({}) {{\n\
                 \x20 {pair} r = {symbol}({});\n\
                 \x20 if (r.{status} == 0) {{ {bits} v = ({bits})r.{value}; memcpy(out, &v, sizeof v); }}\n\
                 \x20 return r.{status};\n\
                 }}\n",
                declaration(&lowered),
                declaration(&all),
                names(lowered.iter()),
                pair = pair(kind),
                bits = bits(kind),
                status = PairField::Status.name(),
                value = PairField::Value.name(),
            );
        }
        _ => {
            let _ = write!(
                c,
                "extern uint8_t {symbol}({});\n\
                 static uint8_t {name}({}) {{ return {symbol}({}); }}\n",
                declaration(&all),
                declaration(&all),
                names(all.iter()),
            );
        }
    }
    c
}

/// C text that defines the entry `symbol` of `abi`, as emitted callers lower their call to
/// it, over `body`: a function of its ABI v1 signature the harness declares first. For a
/// register result, `body` writes the result through `out` in its own width, and the
/// entry returns that word beside the status, zero on failure.
pub fn c_definition(layouts: &LayoutTable, abi: &FnAbi, symbol: &str, body: &str) -> String {
    let (all, result) = parameters(abi);
    let mut c = pairs();
    match (register_result(layouts, abi), result) {
        (Some(kind), Some(out)) => {
            let lowered: Vec<_> = all
                .iter()
                .enumerate()
                .filter(|(i, _)| *i != out)
                .map(|(_, p)| p.clone())
                .collect();
            let arguments: Vec<_> = all
                .iter()
                .enumerate()
                .map(|(i, (_, name))| if i == out { "&v" } else { name.as_str() })
                .collect();
            let _ = write!(
                c,
                "{pair} {symbol}({}) {{\n\
                 \x20 {bits} v = 0;\n\
                 \x20 {pair} r = {{0}};\n\
                 \x20 r.{status} = {body}({});\n\
                 \x20 if (r.{status} == 0) r.{value} = ({word})v;\n\
                 \x20 return r;\n\
                 }}\n",
                declaration(&lowered),
                arguments.join(", "),
                pair = pair(kind),
                bits = bits(kind),
                status = PairField::Status.name(),
                value = PairField::Value.name(),
                word = word(PairField::Value.word(kind)),
            );
        }
        _ => {
            let _ = write!(
                c,
                "uint8_t {symbol}({}) {{ return {body}({}); }}\n",
                declaration(&all),
                names(all.iter()),
            );
        }
    }
    c
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::{Context, Module};
    use crate::emit::pair_type;

    /// Every register result's C typedef is the struct emission returns, field for field,
    /// and both are ABI v1's `{ i8 status, word }` (README, Register results): a change to
    /// `REGISTER_PAIR` or `PairField::word` fails here, not in a harness that misreads it.
    #[test]
    fn c_pairs_are_the_emitted_pair_field_by_field() {
        use ScalarKind::*;
        let context = Context::new();
        let module = Module::new(&context, "pairs");
        let c = pairs();
        let kinds = [
            Bool, U8, U16, U32, U64, I8, I16, I32, I64, F32, F64, Pointer,
        ];
        for (i, &kind) in kinds.iter().enumerate() {
            let ty = module.function_type(pair_type(&module, kind), &[]);
            module.function(&format!("pair{i}"), ty).unwrap();
            let text = module.text();
            let emitted = text
                .lines()
                .find_map(|line| {
                    line.strip_prefix("declare { ")?
                        .strip_suffix(&format!(" }} @pair{i}()"))
                })
                .unwrap_or_else(|| panic!("no declaration of pair{i} in {text}"));
            let emitted: Vec<&str> = emitted.split(", ").collect();
            let typedef = c
                .lines()
                .find_map(|line| {
                    line.strip_prefix("typedef struct { ")?
                        .strip_suffix(&format!(" }} {};", pair(kind)))
                })
                .unwrap_or_else(|| panic!("no typedef {} in {c}", pair(kind)));
            let members: Vec<(&str, &str)> = typedef
                .split(';')
                .map(str::trim)
                .filter(|m| !m.is_empty())
                .map(|m| m.rsplit_once(' ').expect("a typed member"))
                .collect();
            let as_llvm = |c: &str| match c.trim() {
                "void *" => "ptr".to_string(),
                c => {
                    let bits = c.strip_prefix("uint").and_then(|c| c.strip_suffix("_t"));
                    format!("i{}", bits.unwrap_or_else(|| panic!("C member type {c}")))
                }
            };
            let from_c: Vec<String> = members.iter().map(|(ty, _)| as_llvm(ty)).collect();
            assert_eq!(emitted, from_c, "{kind:?}: emitted pair against {typedef}");
            let names: Vec<&str> = members.iter().map(|&(_, name)| name).collect();
            assert_eq!(names[PairField::Status.index() as usize], "status");
            assert_eq!(names[PairField::Value.index() as usize], "value");
            let word = if kind == Pointer { "ptr" } else { "i64" };
            assert_eq!(
                (emitted.as_slice(), names.as_slice()),
                (&["i8", word][..], &["status", "value"][..]),
                "{kind:?}: ABI v1 returns {{ i8 status, word }}"
            );
        }
    }
}
