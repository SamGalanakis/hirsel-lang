//! Runners and program images (link design §2, §4): the native edit loop runs a program without
//! writing an executable.
//!
//! A *runner* (`hirsel-runner`, one per target and runtime archive) links the runtime archive
//! and the kit's unwinder with no object of its own: lld makes the runtime's
//! `hirsel_runner_main` (`hirsel_rt::runner`) its `main`, and roots and exports its export
//! manifest ([`export_manifest`]): every runtime symbol the archive defines (`hirsel_rt*`) and
//! the runtime symbols the builtin-support archive imports. After the link its dynamic
//! symbol table is checked against the manifest, and its startup audit decides how it runs:
//! a fork server (`serve`) when every constructor that runs before `main` is allowlisted
//! initialization independent of any run, otherwise a fresh runner process per run (`exec`).
//! A runner is built once per build ID (the kit, the archive and the line, by content), which is
//! its compatibility identity, under `<cache>/runners/<id>/`.
//!
//! An *image* is a shared object of a program's units and the launcher objects an executable
//! of it links (`main.o`, or a test executable's dispatcher and adapters), plus a note naming
//! its runner's build ID, linked `-shared -Bsymbolic` against the builtin-support archive and
//! the kit's glibc stubs, exporting only `main`. Its build ID is its key: every input by
//! content, the runner's identity through the note, and the line. Before an image is published
//! to `<cache>/images/` its import contract is checked: every undefined dynamic symbol is a
//! runner export of the same kind or a kit library's versioned symbol of the version it
//! requires, none is weak, it needs only kit libraries, and it has no text relocation,
//! executable stack or writable and executable segment. A refused image, an lld error, any
//! foreign-call surface and the memcheck variant fall back to a linked executable, counted by
//! reason ([`Counts`]); no user code runs before that decision.
use super::kit::{Kind, Kit, Link};
use super::link::{Toolchain, checked};
use super::stages::{NativeError, StageName};
use hirsel_engine::store::{Action, Address, Domain, Reference};
use hirsel_lir::layout_types::TargetSpec;
use hirsel_syntax::ContentHash;
use object::read::elf::{Dyn, FileHeader, ProgramHeader, Rela, SectionHeader, Sym};
use object::{LittleEndian, elf};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

type Elf = elf::FileHeader64<LittleEndian>;

fn failed(message: impl Into<String>) -> NativeError {
    NativeError::new(StageName::Link, message)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// How a runner runs each program.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Mode {
    /// One long-lived fork server per runner and requester; each run is a forked child.
    Fork,
    /// A fresh runner process per run; the reason names what the startup audit refused.
    Exec(String),
}

/// Which way a toolchain runs a program it links for a launch.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Execution {
    /// A linked executable per program.
    Executable,
    /// An image in a runner, in the mode the runner's startup audit selected.
    #[default]
    Runner,
    /// An image in a fresh runner process per run, whatever the audit selected.
    RunnerExec,
}

/// A symbol a runner exports: a function or data.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
enum SymbolKind {
    Function,
    Data,
}

/// A built and checked runner.
#[derive(Clone, Debug)]
pub struct Runner {
    pub path: PathBuf,
    pub build_id: [u8; 20],
    pub mode: Mode,
    /// For a `debug_info` toolchain, the executable containing its embedded DWARF.
    pub debug: Option<PathBuf>,
    exports: Arc<BTreeMap<String, SymbolKind>>,
}

/// Each ELF member's global symbols in `archive`: those it defines with default visibility
/// and those it leaves undefined.
fn archive_symbols(archive: &Path) -> Result<(BTreeSet<String>, BTreeSet<String>), NativeError> {
    let bytes =
        std::fs::read(archive).map_err(|e| failed(format!("{}: {e}", archive.display())))?;
    let parsed = object::read::archive::ArchiveFile::parse(&*bytes)
        .map_err(|e| failed(format!("{}: {e}", archive.display())))?;
    let (mut defined, mut undefined) = (BTreeSet::new(), BTreeSet::new());
    for member in parsed.members() {
        let member = member.map_err(|e| failed(format!("{}: {e}", archive.display())))?;
        let data = member
            .data(&*bytes)
            .map_err(|e| failed(format!("{}: {e}", archive.display())))?;
        let Ok(header) = Elf::parse(data) else {
            continue;
        };
        let endian = header.endian().map_err(|e| failed(e.to_string()))?;
        let sections = header
            .sections(endian, data)
            .map_err(|e| failed(e.to_string()))?;
        let symbols = sections
            .symbols(endian, data, elf::SHT_SYMTAB)
            .map_err(|e| failed(e.to_string()))?;
        for symbol in symbols.iter() {
            if symbol.st_bind() == elf::STB_LOCAL {
                continue;
            }
            let name = symbol
                .name(endian, symbols.strings())
                .map_err(|e| failed(e.to_string()))?;
            if name.is_empty() {
                continue;
            }
            let name = String::from_utf8_lossy(name).into_owned();
            if symbol.is_undefined(endian) {
                undefined.insert(name);
            } else if symbol.st_visibility() == elf::STV_DEFAULT {
                defined.insert(name);
            }
        }
    }
    Ok((defined, undefined))
}

/// The symbols a runner exports from `runtime` to the images it loads: every runtime entry
/// (the archive's defined globals named `hirsel_rt*`), and each symbol the builtin-support
/// archive `builtins` imports and the runtime defines (its unwind tables name the runtime's
/// personality routine).
pub(crate) fn export_manifest(runtime: &Path, builtins: &Path) -> Result<Vec<String>, NativeError> {
    let (defined, _) = archive_symbols(runtime)?;
    let (supported, imported) = archive_symbols(builtins)?;
    let names: BTreeSet<String> = defined
        .iter()
        .filter(|name| {
            name.starts_with("hirsel_rt")
                || name.starts_with("hirsel.runtime.") && name.ends_with(".provider")
                || imported.contains(*name) && !supported.contains(*name)
        })
        .cloned()
        .collect();
    if !names.iter().any(|name| name.starts_with("hirsel_rt")) {
        return Err(failed(format!(
            "{} defines no runtime symbol",
            runtime.display()
        )));
    }
    Ok(names.into_iter().collect())
}

/// What a linked ELF file presents to the dynamic loader.
#[derive(Debug, Default)]
struct Interface {
    needed: Vec<String>,
    /// Defined dynamic symbols with default or protected visibility.
    exports: BTreeMap<String, SymbolKind>,
    imports: Vec<Import>,
    /// Version definitions of each defined dynamic symbol (a kit stub's).
    versions: BTreeMap<String, BTreeSet<String>>,
    textrel: bool,
    executable_stack: bool,
    writable_code: bool,
    /// A thread-local storage segment, or an IFUNC symbol in its static symbol table: units
    /// emit neither (link design §7), so either came from an unvetted input.
    tls_or_ifunc: bool,
    soname: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Import {
    name: String,
    weak: bool,
    kind: Option<SymbolKind>,
    /// The library file and version it requires.
    version: Option<(String, String)>,
}

fn interface(bytes: &[u8]) -> Result<Interface, NativeError> {
    let err = |e: object::Error| failed(format!("an ELF file: {e}"));
    let header = Elf::parse(bytes).map_err(err)?;
    let endian = header.endian().map_err(err)?;
    let sections = header.sections(endian, bytes).map_err(err)?;
    let mut out = Interface::default();
    for segment in header.program_headers(endian, bytes).map_err(err)? {
        let flags = segment.p_flags(endian);
        match segment.p_type(endian) {
            elf::PT_GNU_STACK if flags.0 & elf::PF_X.0 != 0 => out.executable_stack = true,
            elf::PT_LOAD if flags.0 & elf::PF_W.0 != 0 && flags.0 & elf::PF_X.0 != 0 => {
                out.writable_code = true
            }
            elf::PT_TLS => out.tls_or_ifunc = true,
            _ => {}
        }
    }
    let table = sections
        .symbols(endian, bytes, elf::SHT_SYMTAB)
        .map_err(err)?;
    out.tls_or_ifunc |= table.iter().any(|s| s.st_type() == elf::STT_GNU_IFUNC);
    let symbols = sections
        .symbols(endian, bytes, elf::SHT_DYNSYM)
        .map_err(err)?;
    let versions = sections
        .versions(endian, bytes)
        .map_err(err)?
        .unwrap_or_default();
    for (index, symbol) in symbols.enumerate() {
        if index.0 == 0 {
            continue;
        }
        let name = String::from_utf8_lossy(symbol.name(endian, symbols.strings()).map_err(err)?)
            .into_owned();
        let kind = match symbol.st_type() {
            elf::STT_FUNC | elf::STT_GNU_IFUNC => Some(SymbolKind::Function),
            elf::STT_OBJECT | elf::STT_TLS => Some(SymbolKind::Data),
            _ => None,
        };
        let version = versions
            .version(versions.version_index(endian, index).index())
            .map_err(err)?;
        if symbol.is_undefined(endian) {
            out.imports.push(Import {
                name,
                weak: symbol.st_bind() == elf::STB_WEAK,
                kind,
                version: version.map(|v| {
                    (
                        String::from_utf8_lossy(v.file().unwrap_or_default()).into_owned(),
                        String::from_utf8_lossy(v.name()).into_owned(),
                    )
                }),
            });
            continue;
        }
        if let Some(version) = version {
            out.versions
                .entry(name.clone())
                .or_default()
                .insert(String::from_utf8_lossy(version.name()).into_owned());
        }
        if matches!(
            symbol.st_visibility(),
            elf::STV_DEFAULT | elf::STV_PROTECTED
        ) && let Some(kind) = kind
        {
            out.exports.insert(name, kind);
        }
    }
    if let Some((entries, index)) = sections.dynamic(endian, bytes).map_err(err)? {
        let strings = sections.strings(endian, bytes, index).map_err(err)?;
        for entry in entries {
            match entry.tag(endian) {
                elf::DT_NEEDED => out.needed.push(
                    String::from_utf8_lossy(entry.string(endian, strings).map_err(err)?)
                        .into_owned(),
                ),
                elf::DT_SONAME => {
                    out.soname = Some(
                        String::from_utf8_lossy(entry.string(endian, strings).map_err(err)?)
                            .into_owned(),
                    )
                }
                elf::DT_TEXTREL => out.textrel = true,
                elf::DT_FLAGS if entry.d_val(endian) & elf::DF_TEXTREL.0 != 0 => out.textrel = true,
                _ => {}
            }
        }
    }
    Ok(out)
}

/// The libraries an image may need, and each one's versioned definitions: the kit's stubs.
#[derive(Debug, Default)]
struct Provided {
    /// Per library file (SONAME), each symbol's version definitions.
    libraries: BTreeMap<String, BTreeMap<String, BTreeSet<String>>>,
}

fn provided(kit: &Kit, triple: &str) -> Result<Arc<Provided>, NativeError> {
    let mut out = Provided::default();
    for path in kit.libraries(triple)? {
        let bytes = std::fs::read(path).map_err(|e| failed(format!("{}: {e}", path.display())))?;
        let stub = interface(&bytes)?;
        let soname = stub.soname.clone().unwrap_or_else(|| {
            path.file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default()
        });
        out.libraries.insert(soname, stub.versions);
    }
    Ok(Arc::new(out))
}

/// Check an image's import contract against its runner's exports and the kit's libraries.
fn check_image(
    image: &Interface,
    runner: &BTreeMap<String, SymbolKind>,
    kit: &Provided,
) -> Result<(), String> {
    if image.textrel {
        return Err("it has text relocations".into());
    }
    if image.executable_stack {
        return Err("it asks for an executable stack".into());
    }
    if image.writable_code {
        return Err("it has a writable and executable segment".into());
    }
    if image.tls_or_ifunc {
        return Err("it has thread-local storage or an IFUNC".into());
    }
    for needed in &image.needed {
        if !kit.libraries.contains_key(needed) {
            return Err(format!("it needs `{needed}`, which is no kit library"));
        }
    }
    let exports: Vec<&String> = image.exports.keys().collect();
    if exports != ["main"] {
        return Err(format!("it exports {exports:?}, not only `main`"));
    }
    for import in &image.imports {
        let name = &import.name;
        if import.weak {
            return Err(format!("it imports `{name}` weakly"));
        }
        match &import.version {
            Some((file, version)) => {
                let Some(library) = kit.libraries.get(file) else {
                    return Err(format!("it imports `{name}` from `{file}`, no kit library"));
                };
                if !library.get(name).is_some_and(|v| v.contains(version)) {
                    return Err(format!(
                        "it imports `{name}@{version}`, which `{file}` does not define"
                    ));
                }
            }
            None => match runner.get(name) {
                Some(kind) if import.kind.is_none_or(|k| k == *kind) => {}
                Some(kind) => {
                    return Err(format!(
                        "it imports `{name}` as {:?} but the runner exports {kind:?}",
                        import.kind
                    ));
                }
                None => {
                    return Err(format!(
                        "it imports `{name}`, which neither the runner nor a kit library provides"
                    ));
                }
            },
        }
    }
    Ok(())
}

/// The SONAMEs whose constructors a forked runner may have run: glibc's own libraries and
/// loader, whose initialization depends on no run and which `fork` keeps consistent.
const STARTUP_LIBRARIES: [&str; 9] = [
    "libc.so.6",
    "libm.so.6",
    "libpthread.so.0",
    "libdl.so.2",
    "librt.so.1",
    "libutil.so.1",
    "libresolv.so.2",
    "ld-linux-x86-64.so.2",
    "ld-linux-aarch64.so.1",
];

/// Whether `name`, a constructor a runner runs before `main`, is admitted before fork:
/// glibc's `_init`, the kit's LLVM crtbegin's `__do_init` (its frame registry and `.ctors`
/// walk), AArch64 compiler builtins' probes of the CPU's atomics and features (they cache the
/// auxiliary vector's hardware capabilities, which a child shares), and Rust std's argv
/// capture, which nothing reads (the runtime takes each run's arguments explicitly). None of
/// them depends on a run.
fn admitted_constructor(name: &str) -> bool {
    matches!(
        name,
        "_init" | "__do_init" | "init_have_lse_atomics" | "__init_cpu_features"
    ) || name.contains("ARGV_INIT_ARRAY")
}

/// The startup audit: every constructor the runner runs before `main` (its init arrays and
/// `DT_INIT`) and every library it needs. `Fork` when all are admitted.
fn audit(bytes: &[u8]) -> Result<Mode, NativeError> {
    let err = |e: object::Error| failed(format!("the runner: {e}"));
    let header = Elf::parse(bytes).map_err(err)?;
    let endian = header.endian().map_err(err)?;
    let sections = header.sections(endian, bytes).map_err(err)?;
    let symbols = sections
        .symbols(endian, bytes, elf::SHT_SYMTAB)
        .map_err(err)?;
    let name_at = |address: u64| -> Option<String> {
        symbols.iter().find_map(|symbol| {
            (symbol.st_value(endian) == address
                && symbol.st_type() == elf::STT_FUNC
                && !symbol.is_undefined(endian))
            .then(|| symbol.name(endian, symbols.strings()).ok())
            .flatten()
            .map(|n| String::from_utf8_lossy(n).into_owned())
        })
    };
    let mut refused = Vec::new();
    let mut constructors = Vec::new();
    let arrays: Vec<(u64, u64)> = sections
        .iter()
        .filter(|s| {
            matches!(
                s.sh_type(endian),
                elf::SHT_INIT_ARRAY | elf::SHT_PREINIT_ARRAY
            )
        })
        .map(|s| (s.sh_addr(endian), s.sh_size(endian)))
        .collect();
    if sections
        .section_by_name(endian, b".ctors")
        .is_some_and(|(_, s)| s.sh_size(endian) > 16)
    {
        refused.push(".ctors entries".to_string());
    }
    let relative = match header.e_machine(endian) {
        elf::EM_X86_64 => elf::R_X86_64_RELATIVE,
        elf::EM_AARCH64 => elf::R_AARCH64_RELATIVE,
        machine => return Err(failed(format!("a runner for machine {machine}"))),
    };
    for section in sections.iter() {
        if section.sh_type(endian) != elf::SHT_RELA {
            continue;
        }
        let Some((relocations, _)) = section.rela(endian, bytes).map_err(err)? else {
            continue;
        };
        for relocation in relocations {
            let offset = relocation.r_offset(endian);
            if !arrays
                .iter()
                .any(|&(start, size)| offset >= start && offset < start + size)
            {
                continue;
            }
            if relocation.r_type(endian, false) == relative {
                constructors.push(relocation.r_addend(endian) as u64);
            } else {
                refused.push(format!("an imported constructor at {offset:#x}"));
            }
        }
    }
    if let Some((entries, index)) = sections.dynamic(endian, bytes).map_err(err)? {
        let strings = sections.strings(endian, bytes, index).map_err(err)?;
        for entry in entries {
            match entry.tag(endian) {
                elf::DT_INIT => constructors.push(entry.d_val(endian)),
                elf::DT_NEEDED => {
                    let needed =
                        String::from_utf8_lossy(entry.string(endian, strings).map_err(err)?)
                            .into_owned();
                    if !STARTUP_LIBRARIES.contains(&needed.as_str()) {
                        refused.push(format!("the library {needed}"));
                    }
                }
                _ => {}
            }
        }
    }
    for address in constructors {
        match name_at(address) {
            Some(name) if admitted_constructor(&name) => {}
            Some(name) => refused.push(format!("the constructor {name}")),
            None => refused.push(format!("an unnamed constructor at {address:#x}")),
        }
    }
    Ok(if refused.is_empty() {
        Mode::Fork
    } else {
        Mode::Exec(refused.join(", "))
    })
}

/// The runner for `target` and the toolchain's selected archive: built and checked once per
/// build ID, stored as an ordinary product. Current ELF export and startup
/// checks run again before a restored runner is returned to its caller.
pub fn runner(toolchain: &Toolchain, target: &TargetSpec) -> Result<Runner, NativeError> {
    let stripped = toolchain.archive(&target.triple).ok_or_else(|| {
        failed(format!(
            "no {} runtime archive for {}",
            toolchain.runtime_build.variant(),
            target.triple
        ))
    })?;
    let archive =
        super::link::runtime_archive(toolchain, &target.triple, stripped, toolchain.debug_info)?;
    let kit = Kit::load(&toolchain.kit)?;
    let exports = export_manifest(archive, toolchain.builtins_archive(&target.triple)?)?;
    let link = Link {
        kit: &kit,
        triple: &target.triple,
        kind: Kind::Host {
            exports: exports.clone(),
            main: Some("hirsel_runner_main".into()),
        },
        objects: &[],
        runtime: Some(archive),
        unwind: true,
        foreign: &[],
    };
    let (_, id) = link.command(&toolchain.lld, &toolchain.cache.join("probe"))?;
    let action = product_action(toolchain, &kit, target, &id)?;
    let directory = toolchain.cache.join("runners").join(hex(&action.0.0));
    let path = directory.join("hirsel-runner");
    let inspect = |bytes: &[u8]| -> Result<Runner, NativeError> {
        let linked = interface(bytes)?;
        let mut checked_exports = BTreeMap::new();
        for name in &exports {
            let kind = linked
                .exports
                .get(name)
                .ok_or_else(|| failed(format!("the runner does not export `{name}`")))?;
            checked_exports.insert(name.clone(), *kind);
        }
        Ok(Runner {
            path: path.clone(),
            build_id: id,
            mode: audit(bytes)?,
            debug: toolchain.debug_info.then(|| path.clone()),
            exports: Arc::new(checked_exports),
        })
    };
    if let Some(reference) = lookup(toolchain, RUNNER_PRODUCT, action)
        && let Ok(bytes) =
            super::object_queries::link_input_bytes(toolchain.products.as_ref(), reference)
        && let Ok(runner) = inspect(&bytes)
    {
        super::object_queries::materialize_link_input(
            toolchain.products.as_ref(),
            reference,
            &path,
            true,
        )
        .map_err(failed)?;
        return Ok(runner);
    }
    // The linker writes only private request scratch; publication follows every
    // ELF check. A crash cannot make a pathname or JSON receipt select a hit.
    let scratch = super::executable::Scratch::new()?;
    let built = scratch.0.join("hirsel-runner");
    let (mut command, _) = link.command(&toolchain.lld, &built)?;
    checked(StageName::Link, &mut command)?;
    let bytes = std::fs::read(&built).map_err(|e| failed(e.to_string()))?;
    let runner = inspect(&bytes)?;
    let reference = publish(toolchain, RUNNER_PRODUCT, action, &bytes)?;
    super::object_queries::materialize_link_input(
        toolchain.products.as_ref(),
        reference,
        &path,
        true,
    )
    .map_err(failed)?;
    Ok(runner)
}

const RUNNER_PRODUCT: Domain = Domain {
    kind: 0x4f00_0007,
    schema: 1,
};
const IMAGE_PRODUCT: Domain = Domain {
    kind: 0x4f00_0008,
    schema: 1,
};
fn product_address(domain: Domain, action: Action) -> Address {
    Address(ContentHash::of_parts(
        "hirsel.native.linked-input-address.v1",
        [domain.kind.to_le_bytes().as_slice(), action.0.0.as_slice()],
    ))
}
fn product_action(
    toolchain: &Toolchain,
    kit: &Kit,
    target: &TargetSpec,
    id: &[u8; 20],
) -> Result<Action, NativeError> {
    let mut paths = kit.link_files(&target.triple)?;
    paths.push(toolchain.lld.clone());
    // The shipped dynamic linker runtime is also an implementation input.
    let tree = toolchain
        .lld
        .parent()
        .and_then(Path::parent)
        .ok_or_else(|| failed("linker has no installed tree"))?;
    for entry in std::fs::read_dir(tree.join("lib")).map_err(|e| failed(e.to_string()))? {
        let path = entry.map_err(|e| failed(e.to_string()))?.path();
        if path.is_file()
            && path
                .file_name()
                .is_some_and(|name| name.to_string_lossy().contains(".so"))
        {
            paths.push(path);
        }
    }
    paths.sort();
    let mut parts = vec![
        crate::COMPILER_IDENTITY.to_vec(),
        id.to_vec(),
        vec![u8::from(toolchain.debug_info)],
    ];
    for path in paths {
        parts.push(
            super::file_digest::file(&path)
                .map_err(|e| failed(e.to_string()))?
                .content
                .0
                .to_vec(),
        );
    }
    Ok(Action(ContentHash::of_parts(
        "hirsel.native.linked-input-action.v1",
        parts.iter().map(Vec::as_slice),
    )))
}
fn lookup(toolchain: &Toolchain, domain: Domain, action: Action) -> Option<Reference> {
    let association = toolchain
        .products
        .lookup_product(product_address(domain, action))?;
    if association.action != action || association.reference.domain != domain {
        return None;
    }
    let bytes = toolchain.products.get_content(association.reference)?;
    if domain.reference(&bytes) != Some(association.reference) {
        return None;
    }
    serde_json::from_slice(&bytes).ok()
}
fn publish(
    toolchain: &Toolchain,
    domain: Domain,
    action: Action,
    bytes: &[u8],
) -> Result<Reference, NativeError> {
    let reference = super::object_queries::store_link_input(toolchain.products.as_ref(), bytes)
        .map_err(failed)?;
    let record = serde_json::to_vec(&reference).map_err(|e| failed(e.to_string()))?;
    let stored = toolchain
        .products
        .put_content(domain, &record)
        .ok_or_else(|| failed("linked input publication failed"))?;
    if domain.reference(&record) != Some(stored) {
        return Err(failed("linked input identity differs"));
    }
    let address = product_address(domain, action);
    toolchain
        .products
        .associate_product(address, action, stored);
    if toolchain
        .products
        .lookup_product(address)
        .is_none_or(|row| row.action != action || row.reference != stored)
    {
        return Err(failed("linked input association failed"));
    }
    Ok(reference)
}

/// A relocatable object holding only the note that names `runner` (`hirsel_rt::runner`).
fn note_object(target: &TargetSpec, runner: &Runner) -> Result<Vec<u8>, NativeError> {
    use object::write::Object;
    let architecture = if target.triple.starts_with("aarch64") {
        object::Architecture::Aarch64
    } else {
        object::Architecture::X86_64
    };
    let mut object = Object::new(
        object::BinaryFormat::Elf,
        architecture,
        object::Endianness::Little,
    );
    let section = object.add_section(
        Vec::new(),
        b".note.hirsel".to_vec(),
        object::SectionKind::Note,
    );
    // Allocated, so the link maps it in a PT_NOTE segment the child reads.
    object.section_mut(section).flags = object::SectionFlags::Elf {
        sh_type: elf::SHT_NOTE,
        sh_flags: elf::SHF_ALLOC,
    };
    let mut note = Vec::new();
    note.extend((hirsel_rt::runner::NOTE_OWNER.len() as u32).to_le_bytes());
    note.extend((runner.build_id.len() as u32).to_le_bytes());
    note.extend(hirsel_rt::runner::NOTE_TYPE.to_le_bytes());
    note.extend(hirsel_rt::runner::NOTE_OWNER);
    note.resize(note.len().next_multiple_of(4), 0);
    note.extend(runner.build_id);
    note.resize(note.len().next_multiple_of(4), 0);
    object.append_section_data(section, &note, 4);
    object
        .write()
        .map_err(|e| failed(format!("the runner note: {e}")))
}

/// A published image and the runner that loads it.
#[derive(Clone, Debug)]
pub struct Image {
    pub path: PathBuf,
    pub key: [u8; 20],
    pub runner: Runner,
    /// It was in the cache: no lld ran.
    pub cached: bool,
}

/// Link (or find) the image of `inputs` (launcher objects, then units, in shipped order) for
/// `runner`. `Err` is a fallback reason when lld or the import contract refuses it.
pub(crate) fn link_image(
    toolchain: &Toolchain,
    target: &TargetSpec,
    runner: &Runner,
    inputs: &[PathBuf],
    dir: &Path,
) -> Result<Result<Image, String>, NativeError> {
    let kit = Kit::load(&toolchain.kit)?;
    let io = |e: std::io::Error| failed(e.to_string());
    let note = dir.join("runner-note.o");
    std::fs::write(&note, note_object(target, runner)?).map_err(io)?;
    let script = dir.join("image.map");
    std::fs::write(&script, "{ global: main; local: *; };\n").map_err(io)?;
    let mut objects = vec![note];
    objects.extend_from_slice(inputs);
    let builtins = toolchain.builtins_archive(&target.triple)?;
    let link = Link {
        kit: &kit,
        triple: &target.triple,
        kind: Kind::Image { script },
        objects: &objects,
        runtime: Some(builtins),
        unwind: false,
        foreign: &[],
    };
    let (_, key) = link.command(&toolchain.lld, &dir.join("probe"))?;
    let action = product_action(toolchain, &kit, target, &key)?;
    let path = dir.join(format!("image-{}.so", hex(&action.0.0)));
    let validate = |bytes: &[u8]| -> Result<Result<(), String>, NativeError> {
        let image = interface(bytes)?;
        Ok(check_image(
            &image,
            &runner.exports,
            &*provided(&kit, &target.triple)?,
        ))
    };
    if let Some(reference) = lookup(toolchain, IMAGE_PRODUCT, action)
        && let Ok(bytes) =
            super::object_queries::link_input_bytes(toolchain.products.as_ref(), reference)
        && matches!(validate(&bytes), Ok(Ok(())))
    {
        super::object_queries::materialize_link_input(
            toolchain.products.as_ref(),
            reference,
            &path,
            true,
        )
        .map_err(failed)?;
        return Ok(Ok(Image {
            path,
            key,
            runner: runner.clone(),
            cached: true,
        }));
    }
    let scratch = super::executable::Scratch::new()?;
    let output = scratch.0.join("image.so");
    let (mut command, _) = link.command(&toolchain.lld, &output)?;
    if let Err(error) = checked(StageName::Link, &mut command) {
        return Ok(Err(format!("lld: {}", error.message)));
    }
    let bytes = std::fs::read(&output).map_err(io)?;
    match validate(&bytes)? {
        Ok(()) => {}
        Err(refusal) => return Ok(Err(format!("image refused: {refusal}"))),
    }
    let reference = publish(toolchain, IMAGE_PRODUCT, action, &bytes)?;
    super::object_queries::materialize_link_input(
        toolchain.products.as_ref(),
        reference,
        &path,
        true,
    )
    .map_err(failed)?;
    Ok(Ok(Image {
        path,
        key,
        runner: runner.clone(),
        cached: false,
    }))
}

/// A shared hold on `image` for one run, so reclamation leaves it in place.
pub(crate) fn hold(image: &Path) -> Option<std::fs::File> {
    let file = std::fs::File::open(image).ok()?;
    file.lock_shared().ok()?;
    Some(file)
}

/// Images run, cache hits and fallbacks to a linked executable, by the kind of their reason
/// (the text before its first `:`) with the first reason of each kind, in this process.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Counts {
    pub images: u64,
    pub cached: u64,
    pub fallbacks: BTreeMap<String, u64>,
    pub reasons: BTreeMap<String, String>,
}

static COUNTS: Mutex<Counts> = Mutex::new(Counts {
    images: 0,
    cached: 0,
    fallbacks: BTreeMap::new(),
    reasons: BTreeMap::new(),
});

pub(crate) fn count_image(image: &Image) {
    let mut counts = COUNTS.lock().unwrap();
    counts.images += 1;
    counts.cached += u64::from(image.cached);
}

/// Count a fallback by its reason's kind.
pub(crate) fn count_fallback(reason: &str) {
    let kind = reason.split(':').next().unwrap_or(reason).to_string();
    let mut counts = COUNTS.lock().unwrap();
    *counts.fallbacks.entry(kind.clone()).or_default() += 1;
    counts
        .reasons
        .entry(kind)
        .or_insert_with(|| reason.to_string());
}

/// This process's runner counts so far.
pub fn counts() -> Counts {
    COUNTS.lock().unwrap().clone()
}

/// A started run: its process ID (its group's, when it leads one), its output pipes when they
/// are captured, and how to wait for its status.
pub(crate) struct Started {
    pub id: u32,
    pub stdout: Option<Box<dyn std::io::Read + Send>>,
    pub stderr: Option<Box<dyn std::io::Read + Send>>,
    waiter: Waiter,
}

enum Waiter {
    Child(std::process::Child),
    Server(Box<Server>),
}

impl Started {
    pub(crate) fn spawned(mut child: std::process::Child) -> Self {
        Self {
            id: child.id(),
            stdout: child.stdout.take().map(|p| Box::new(p) as _),
            stderr: child.stderr.take().map(|p| Box::new(p) as _),
            waiter: Waiter::Child(child),
        }
    }

    pub(crate) fn wait(self) -> std::io::Result<std::process::ExitStatus> {
        use std::os::unix::process::ExitStatusExt;
        match self.waiter {
            Waiter::Child(mut child) => child.wait(),
            Waiter::Server(mut server) => {
                let status = server.read_reply(hirsel_rt::runner::REPLY_STATUS)?;
                give_back(*server);
                Ok(std::process::ExitStatus::from_raw(status))
            }
        }
    }
}

/// A fork server and the socket that drives it.
struct Server {
    key: (PathBuf, Vec<std::ffi::OsString>),
    process: std::process::Child,
    socket: std::os::unix::net::UnixStream,
}

static SERVERS: Mutex<Vec<Server>> = Mutex::new(Vec::new());

fn give_back(server: Server) {
    SERVERS.lock().unwrap().push(server);
}

impl Server {
    fn take(prefix: &std::process::Command) -> std::io::Result<Server> {
        let key = (
            PathBuf::from(prefix.get_program()),
            prefix.get_args().map(ToOwned::to_owned).collect::<Vec<_>>(),
        );
        let mut servers = SERVERS.lock().unwrap();
        while let Some(at) = servers.iter().position(|s| s.key == key) {
            let mut server = servers.swap_remove(at);
            if matches!(server.process.try_wait(), Ok(None)) {
                return Ok(server);
            }
        }
        drop(servers);
        let (ours, theirs) = std::os::unix::net::UnixStream::pair()?;
        let mut command = std::process::Command::new(prefix.get_program());
        command
            .args(prefix.get_args())
            .arg("serve")
            .env_clear()
            .stdin(std::process::Stdio::from(std::os::fd::OwnedFd::from(
                theirs,
            )))
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        for (name, value) in prefix.get_envs() {
            if let Some(value) = value {
                command.env(name, value);
            }
        }
        let process = command.spawn()?;
        Ok(Server {
            key,
            process,
            socket: ours,
        })
    }

    fn read_reply(&mut self, tag: u8) -> std::io::Result<i32> {
        use std::io::Read;
        let mut reply = [0u8; 5];
        self.socket.read_exact(&mut reply)?;
        if reply[0] != tag {
            return Err(std::io::Error::other("an unexpected runner reply"));
        }
        Ok(i32::from_le_bytes(reply[1..].try_into().unwrap()))
    }

    fn send(&mut self, message: &[u8], streams: [std::os::fd::RawFd; 3]) -> std::io::Result<()> {
        use std::io::Write;
        use std::os::fd::AsRawFd;
        // SAFETY: CMSG_SPACE is a size computation.
        let space = unsafe { libc::CMSG_SPACE(3 * size_of::<libc::c_int>() as u32) } as usize;
        let mut ancillary = vec![0u64; space.div_ceil(8)];
        let mut vector = libc::iovec {
            iov_base: message.as_ptr() as *mut libc::c_void,
            iov_len: 4,
        };
        // SAFETY: a zeroed msghdr is valid; its buffers outlive the call.
        let mut header: libc::msghdr = unsafe { std::mem::zeroed() };
        header.msg_iov = &mut vector;
        header.msg_iovlen = 1;
        header.msg_control = ancillary.as_mut_ptr().cast();
        header.msg_controllen = space as _;
        // SAFETY: fills the one control message the buffer holds.
        unsafe {
            let control = libc::CMSG_FIRSTHDR(&header);
            (*control).cmsg_level = libc::SOL_SOCKET;
            (*control).cmsg_type = libc::SCM_RIGHTS;
            (*control).cmsg_len = libc::CMSG_LEN(3 * size_of::<libc::c_int>() as u32) as _;
            let data = libc::CMSG_DATA(control).cast::<libc::c_int>();
            for (i, fd) in streams.into_iter().enumerate() {
                data.add(i).write_unaligned(fd);
            }
        }
        loop {
            // SAFETY: the header's buffers are live.
            let sent = unsafe { libc::sendmsg(self.socket.as_raw_fd(), &header, 0) };
            if sent == 4 {
                break;
            }
            let error = std::io::Error::last_os_error();
            if sent < 0 && error.kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            return Err(if sent < 0 {
                error
            } else {
                std::io::Error::other("a partial runner request")
            });
        }
        self.socket.write_all(&message[4..])
    }
}

/// Start `image` on a fork server of its runner (`prefix`, its launcher command), with
/// `command`'s arguments after one naming the image, its environment, this process's
/// directory, no standard input and captured output, as a bounded launch of `timeout` runs an
/// executable.
/// The caller waits for the run's status with [`Started::wait`].
pub(crate) fn start_served(
    prefix: &std::process::Command,
    image: &Image,
    tooling: &Path,
    command: &std::process::Command,
    timeout: Option<std::time::Duration>,
) -> std::io::Result<Started> {
    use std::os::fd::{AsRawFd, OwnedFd};
    use std::os::unix::ffi::OsStrExt;
    let stdin: OwnedFd = std::fs::File::open("/dev/null")?.into();
    let (stdout_reader, stdout) = std::io::pipe()?;
    let (stderr_reader, stderr) = std::io::pipe()?;
    let mut arguments: Vec<&[u8]> = vec![image.path.as_os_str().as_bytes()];
    arguments.extend(command.get_args().map(|a| a.as_bytes()));
    let environment: Vec<Vec<u8>> = command
        .get_envs()
        .filter_map(|(name, value)| {
            let mut entry = name.as_bytes().to_vec();
            entry.push(b'=');
            entry.extend(value?.as_bytes());
            Some(entry)
        })
        .collect();
    let environment: Vec<&[u8]> = environment.iter().map(Vec::as_slice).collect();
    let directory = std::env::current_dir()?;
    let message = hirsel_rt::runner::encode_request(
        tooling.as_os_str().as_bytes(),
        image.path.as_os_str().as_bytes(),
        directory.as_os_str().as_bytes(),
        &arguments,
        &environment,
    );
    let mut server = Server::take(prefix)?;
    // The run's status follows its kill at the timeout at once; a server silent past that
    // is broken.
    server
        .socket
        .set_read_timeout(timeout.map(|t| t + std::time::Duration::from_secs(30)))?;
    server.send(
        &message,
        [stdin.as_raw_fd(), stdout.as_raw_fd(), stderr.as_raw_fd()],
    )?;
    // The child holds the only writers now, so the pipes end when its group does.
    drop((stdin, stdout, stderr));
    let id = server.read_reply(hirsel_rt::runner::REPLY_STARTED)? as u32;
    Ok(Started {
        id,
        stdout: Some(Box::new(stdout_reader)),
        stderr: Some(Box::new(stderr_reader)),
        waiter: Waiter::Server(Box::new(server)),
    })
}

/// The tooling failure a runner child reported before running any program code, if any.
pub(crate) fn tooling_failure(tooling: &Path) -> Option<NativeError> {
    let report = std::fs::read_to_string(tooling).ok()?;
    Some(NativeError::new(
        StageName::Launch,
        format!("Tooling(ImageLoad): {}", report.trim_end()),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provided_contract_rechecks_current_kit_bytes_at_the_same_path() {
        fn copy(source: &Path, destination: &Path) {
            std::fs::create_dir_all(destination).unwrap();
            for entry in std::fs::read_dir(source).unwrap() {
                let path = entry.unwrap().path();
                let target = destination.join(path.file_name().unwrap());
                if path.is_dir() {
                    copy(&path, &target);
                } else {
                    std::fs::copy(path, target).unwrap();
                }
            }
        }
        let directory = tempfile::tempdir().unwrap();
        let source = super::super::link::environment_path("HIRSEL_LINK_KIT").unwrap();
        let captured = directory.path().join("kit");
        copy(&source, &captured);
        let kit = Kit::load(&captured).unwrap();
        let target = TargetSpec::x86_64_unknown_linux_gnu();
        assert!(!provided(&kit, &target.triple).unwrap().libraries.is_empty());
        let library = &kit.libraries(&target.triple).unwrap()[0];
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(library, std::fs::Permissions::from_mode(0o600)).unwrap();
        std::fs::write(library, b"corrupt current ELF stub").unwrap();
        assert!(provided(&kit, &target.triple).is_err());
    }

    fn contract() -> (BTreeMap<String, SymbolKind>, Provided) {
        let runner = BTreeMap::from([
            ("hirsel_rt_v2_main".to_string(), SymbolKind::Function),
            (
                "hirsel_rt_v2_bytes_descriptor".to_string(),
                SymbolKind::Data,
            ),
        ]);
        let mut provided = Provided::default();
        provided.libraries.insert(
            "libc.so.6".into(),
            BTreeMap::from([(
                "memcpy".to_string(),
                BTreeSet::from(["GLIBC_2.2.5".to_string(), "GLIBC_2.14".to_string()]),
            )]),
        );
        provided
            .libraries
            .insert("libm.so.6".into(), BTreeMap::new());
        (runner, provided)
    }

    fn image(imports: Vec<Import>) -> Interface {
        Interface {
            needed: vec!["libc.so.6".into()],
            exports: BTreeMap::from([("main".to_string(), SymbolKind::Function)]),
            imports,
            ..Default::default()
        }
    }

    fn import(name: &str, version: Option<(&str, &str)>) -> Import {
        Import {
            name: name.into(),
            weak: false,
            kind: Some(SymbolKind::Function),
            version: version.map(|(f, v)| (f.into(), v.into())),
        }
    }

    #[test]
    fn images_bind_only_runner_exports_and_kit_versions() {
        let (runner, kit) = contract();
        let accepted = image(vec![
            import("hirsel_rt_v2_main", None),
            import("memcpy", Some(("libc.so.6", "GLIBC_2.14"))),
        ]);
        assert_eq!(check_image(&accepted, &runner, &kit), Ok(()));
        // An unknown import, an absent runtime export, a wrong provider and a wrong version.
        for (imports, refusal) in [
            (vec![import("puts_nowhere", None)], "neither the runner"),
            (
                vec![import("hirsel_rt_v2_gone", None)],
                "neither the runner",
            ),
            (
                vec![import("memcpy", Some(("libm.so.6", "GLIBC_2.14")))],
                "does not define",
            ),
            (
                vec![import("memcpy", Some(("libc.so.6", "GLIBC_2.40")))],
                "does not define",
            ),
            (
                vec![import("memcpy", Some(("libfoo.so.1", "FOO_1")))],
                "no kit library",
            ),
        ] {
            let error = check_image(&image(imports), &runner, &kit).unwrap_err();
            assert!(error.contains(refusal), "{error}");
        }
        let mut data_as_function = import("hirsel_rt_v2_bytes_descriptor", None);
        assert!(check_image(&image(vec![data_as_function.clone()]), &runner, &kit).is_err());
        data_as_function.kind = Some(SymbolKind::Data);
        assert_eq!(
            check_image(&image(vec![data_as_function]), &runner, &kit),
            Ok(())
        );
        let mut weak = import("hirsel_rt_v2_main", None);
        weak.weak = true;
        assert!(check_image(&image(vec![weak]), &runner, &kit).is_err());
        for broken in [
            Interface {
                textrel: true,
                ..image(vec![])
            },
            Interface {
                executable_stack: true,
                ..image(vec![])
            },
            Interface {
                writable_code: true,
                ..image(vec![])
            },
            Interface {
                tls_or_ifunc: true,
                ..image(vec![])
            },
            Interface {
                needed: vec!["libssl.so.3".into()],
                ..image(vec![])
            },
            Interface {
                exports: BTreeMap::from([
                    ("main".to_string(), SymbolKind::Function),
                    ("leaked".to_string(), SymbolKind::Function),
                ]),
                ..image(vec![])
            },
        ] {
            assert!(check_image(&broken, &runner, &kit).is_err(), "{broken:?}");
        }
    }
}
