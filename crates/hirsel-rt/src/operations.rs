//! Native operation inventory and typed adapters. Unknown identity never selects a fallback.
use hirsel_core_contract::{
    CallOwnership, CleanupEffect, Disposition, M2Feature, Manifest, ManifestError, OperationRow,
    Passing, Unavailable,
};

/// Classify exactly the reserved inventory. The declaration producer supplies digest/ownership.
/// Runtime ABI emission for aggregate wrappers awaits I/K; those rows explicitly say unavailable.
pub fn classify(mut row: OperationRow) -> Result<OperationRow, String> {
    let path = &row.id.definition.0;
    if !path.package.package_is("core", "1") {
        return Err("foreign native operation identity".into());
    }
    let name = path
        .modules
        .iter()
        .chain(std::iter::once(&path.name))
        .map(String::as_str)
        .collect::<Vec<_>>()
        .join(".");
    let expected = include_str!("../../../native-tests/library/operations.tsv")
        .lines()
        .find_map(|line| {
            let (path, rest) = line.split_once('\t')?;
            (path == name).then_some(rest)
        })
        .ok_or_else(|| format!("missing native declaration identity: {name}"))?;
    let digest: String = row
        .id
        .declaration_digest
        .0
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    let (expected_digest, expected_ownership) = expected
        .split_once('\t')
        .ok_or("invalid native inventory fixture")?;
    if digest != expected_digest {
        return Err(format!("unreviewed native declaration digest: {name}"));
    }
    let ownership = row
        .ownership
        .as_ref()
        .ok_or("native row has no declared ownership")?;
    if ownership_encoding(ownership) != expected_ownership {
        return Err(format!(
            "native ownership disagrees with declaration: {name}"
        ));
    }
    let feature = match name.split('.').next().unwrap() {
        "task" => Some(M2Feature::Tasks),
        "channel" | "sender" => Some(M2Feature::Channels),
        "mutex" => Some(M2Feature::Mutexes),
        "atomic" => Some(M2Feature::Atomics),
        "shared" => Some(M2Feature::Shared),
        "arena" | "copy_to" => Some(M2Feature::Arenas),
        "region" => Some(M2Feature::Regions),
        "slots" => Some(M2Feature::Slots),
        _ => None,
    };
    if !NATIVE_PATHS.contains(&name.as_str()) {
        return Err(format!("no native disposition: {name}"));
    }
    row.failure = None;
    // The scalar intrinsics are LIR instructions (schema revision 6): legalization
    // replaces each call, chosen from its scalar kinds, and emission is native.
    // So are the inline library operations (schema revision 7): import replaces
    // each call by the instructions `hirsel_rc::library` names for it.
    if SCALAR_INTRINSICS.contains(&name.as_str()) || INLINE_OPERATIONS.contains(&name.as_str()) {
        row.dispositions.native = Disposition::Instruction(name);
        return Ok(row);
    }
    if matches!(
        name.as_str(),
        "slots.insert"
            | "slots.get"
            | "slots.remove"
            | "shared.freeze"
            | "shared.thaw"
            | "arena.with"
            | "arena.bounded"
            | "copy_to"
            | "failure.check"
            | "sorted_map.insert"
            | "$intrinsic.SortedMapOrder_open"
            | "$intrinsic.SortedMapOrder_next"
            | "$intrinsic.OrderCompare_u8"
            | "$intrinsic.OrderCompare_u16"
            | "$intrinsic.OrderCompare_u32"
            | "$intrinsic.OrderCompare_u64"
            | "$intrinsic.OrderCompare_u128"
            | "$intrinsic.OrderCompare_i8"
            | "$intrinsic.OrderCompare_i16"
            | "$intrinsic.OrderCompare_i32"
            | "$intrinsic.OrderCompare_i64"
            | "$intrinsic.OrderCompare_i128"
            | "list.pop"
            | "list.remove"
            | "list.swap"
            | "list.reserve"
            | "list.push"
            | "iter.from"
            | "iter.map"
            | "$intrinsic.ListConsume_open"
            | "$intrinsic.ListConsume_next"
            | "$intrinsic.ArrayConsume_open"
            | "$intrinsic.ArrayConsume_next"
            | "$intrinsic.InlineConsume_open"
            | "$intrinsic.InlineConsume_next"
            | "net.read_into"
            | "task.scope"
            | "task.async"
            | "task.await"
            | "channel.send"
            | "channel.receive"
            | "mutex.create"
            | "mutex.with"
            | "atomic.create"
            | "atomic.load"
            | "atomic.store"
            | "task.isolate"
            | "task.check_canceled"
    ) || MAP_BODIES.contains(&name.as_str())
    {
        row.dispositions.native = Disposition::Body;
        return Ok(row);
    }
    row.failure = matches!(name.as_str(), "string.slice" | "bytes.slice")
        .then(|| hirsel_core_contract::RuleCode("SEM-3".into()));
    let symbol = match name.as_str() {
        // Fixed declaration signatures: every entry has its exact physical ABI
        // in HIRSEL_ABI_EXPORTS; generic callbacks remain compiled bodies.
        "string.from_bytes"
        | "string.slice"
        | "io.stdout"
        | "string.as_bytes"
        | "string.position"
        | "string.byte_offset"
        | "bytes.slice"
        | "path.text"
        | "out.print"
        | "io.dir"
        | "dir.narrow"
        | "path.of"
        | "arg.path"
        | "arg.count"
        | "arg.text"
        | "file.open"
        | "file.read"
        | "file.seek"
        | "file.create"
        | "file.write"
        | "file.append"
        | "file.narrow"
        | "file.create_atomic"
        | "file.commit"
        | "file.size"
        | "file.read_dir"
        | "file.metadata"
        | "file.mkdir"
        | "file.rename"
        | "file.remove_dir"
        | "file.remove"
        | "file.write_bytes"
        | "file.write_all"
        | "io.monotonic_clock"
        | "io.wall_clock"
        | "clock.monotonic"
        | "clock.wall"
        | "instant.ticks"
        | "io.random"
        | "io.tasks"
        | "io.net"
        | "test.check"
        | "test.check_case"
        | "test.fail"
        | "test.dir"
        | "test.net"
        | "file.read_bytes"
        | "file.walk"
        | "io.stdin"
        | "io.stderr"
        | "stdin.read"
        | "err.print"
        | "env.get"
        | "io.process"
        | "proc.run"
        | "test.process"
        | "random.bytes"
        | "clock.sleep_until"
        | "instant.checked_add" => Some(format!("hirsel_rt_v2_{}", name.replace('.', "_"))),
        // Streams and listeners use the same explicit Net policy.
        "net.listen"
        | "net.accept"
        | "net.local_addr"
        | "net.narrow"
        | "net.resolve"
        | "net.connect"
        | "net.connect_addr"
        | "net.read"
        | "net.write"
        | "net.write_all"
        | "net.shutdown"
        | "net.set_read_timeout"
        | "net.set_write_timeout"
        | "net.peer_addr"
        | "net.close"
        | "tls.connect"
        | "tls.close" => Some(format!("hirsel_rt_v2_{}", name.replace('.', "_"))),
        // `net.read_into`'s compiled body calls this with its buffer's size.
        "$intrinsic.NetReadInto" => Some("hirsel_rt_v2_net_read_into".into()),
        // The task runtime (`tasks.rs`) behind the compiled task bodies.
        "$intrinsic.TaskScopeEnter" => Some("hirsel_rt_v2_task_scope_enter".into()),
        "$intrinsic.TaskScopeExit" => Some("hirsel_rt_v2_task_scope_exit".into()),
        "$intrinsic.TaskSpawn" => Some("hirsel_rt_v2_task_spawn".into()),
        "$intrinsic.TaskFinish" => Some("hirsel_rt_v2_task_finish".into()),
        "$intrinsic.TaskJoin" => Some("hirsel_rt_v2_task_join".into()),
        "$intrinsic.TaskCanceled" => Some("hirsel_rt_v2_task_canceled".into()),
        "task.cancel" | "task.detach" => Some(format!("hirsel_rt_v2_{}", name.replace('.', "_"))),
        // Channels (CONC-6), in the task runtime: the compiled send and receive bodies move a
        // message's block through these entries.
        "channel.open" | "sender.dup" => Some(format!("hirsel_rt_v2_{}", name.replace('.', "_"))),
        "$intrinsic.ChannelSend" => Some("hirsel_rt_v2_channel_send".into()),
        "$intrinsic.ChannelReceive" => Some("hirsel_rt_v2_channel_receive".into()),
        // Mutex and atomic cells (`sync.rs`) behind their compiled bodies.
        "$intrinsic.MutexCreate" => Some("hirsel_rt_v2_mutex_create".into()),
        "$intrinsic.MutexTake" => Some("hirsel_rt_v2_mutex_take".into()),
        "$intrinsic.MutexPut" => Some("hirsel_rt_v2_mutex_put".into()),
        "$intrinsic.MutexKill" => Some("hirsel_rt_v2_mutex_kill".into()),
        "$intrinsic.MutexCause" => Some("hirsel_rt_v2_mutex_cause".into()),
        "$intrinsic.AtomicCreate" => Some("hirsel_rt_v2_atomic_create".into()),
        "$intrinsic.AtomicLoad" => Some("hirsel_rt_v2_atomic_load".into()),
        "$intrinsic.AtomicStore" => Some("hirsel_rt_v2_atomic_store".into()),
        // `task.isolate`'s compiled body reads its caught chain through these (`isolate.rs`).
        "$intrinsic.IsolateBegin" => Some("hirsel_rt_v2_isolate_begin".into()),
        "$intrinsic.IsolateCount" => Some("hirsel_rt_v2_isolate_count".into()),
        "$intrinsic.IsolateKind" => Some("hirsel_rt_v2_isolate_kind".into()),
        "$intrinsic.IsolateRule" => Some("hirsel_rt_v2_isolate_rule".into()),
        "$intrinsic.IsolateDefinition" => Some("hirsel_rt_v2_isolate_definition".into()),
        "$intrinsic.IsolateLine" => Some("hirsel_rt_v2_isolate_line".into()),
        "$intrinsic.IsolateColumn" => Some("hirsel_rt_v2_isolate_column".into()),
        "$intrinsic.IsolateSpawn" => Some("hirsel_rt_v2_isolate_spawn".into()),
        "$intrinsic.UnicodeScalars_open" => Some("hirsel_rt_v2_unicode_scalars_open".into()),
        "$intrinsic.UnicodeScalars_next" => Some("hirsel_rt_v2_unicode_scalars_next".into()),
        "$intrinsic.ScalarPositions_open" => Some("hirsel_rt_v2_scalar_positions_open".into()),
        "$intrinsic.ScalarPositions_next" => Some("hirsel_rt_v2_scalar_positions_next".into()),
        _ => None,
    };
    if matches!(
        name.as_str(),
        "io.dir"
            | "dir.narrow"
            | "path.of"
            | "arg.path"
            | "arg.count"
            | "arg.text"
            | "io.monotonic_clock"
            | "io.wall_clock"
            | "clock.monotonic"
            | "clock.wall"
            | "clock.sleep_until"
            | "io.random"
            | "random.bytes"
            | "io.stdin"
            | "io.stderr"
            | "stdin.read"
            | "env.get"
            | "io.stdout"
            | "io.tasks"
            | "io.net"
            | "test.dir"
            | "test.net"
            | "io.process"
            | "proc.run"
            | "test.process"
            | "file.open"
            | "file.read"
            | "file.seek"
            | "file.create"
            | "file.write"
            | "file.append"
            | "file.narrow"
            | "file.create_atomic"
            | "file.commit"
            | "file.read_dir"
            | "file.metadata"
            | "file.mkdir"
            | "file.rename"
            | "file.remove_dir"
            | "file.size"
            | "file.remove"
            | "file.read_bytes"
            | "file.write_bytes"
            | "file.write_all"
            | "file.walk"
    ) || NET_EXPORTS.contains(&name.as_str())
        || name == "$intrinsic.NetReadInto"
    {
        row.failure = Some(hirsel_core_contract::RuleCode("CAP-2".into()));
    }
    // A scripted client's unmet Expect fails these LANG-9, so the runtime records
    // their failures itself (`hirsel_core_contract::RUNTIME_RULES`).
    if matches!(
        name.as_str(),
        "net.write" | "net.write_all" | "net.close" | "tls.close"
    ) {
        row.failure = None;
    }
    if matches!(
        name.as_str(),
        "test.check" | "test.check_case" | "test.fail"
    ) {
        row.failure = Some(hirsel_core_contract::RuleCode("LANG-9".into()));
    }
    row.dispositions.native = if let Some(symbol) = symbol {
        Disposition::RuntimePrimitive(symbol)
    } else {
        Disposition::Unavailable(feature.map_or(Unavailable::NotImplemented, Unavailable::M2))
    };
    Ok(row)
}

/// The core-library operations import replaces by LIR instructions (LIR schema
/// revision 7): the sequence lengths read as
/// `SeqLen`, and the `simd` operations legalization builds. No call remains.
/// Network operations with a Hirsel-ABI runtime entry (`net_abi.rs`).
pub const NET_EXPORTS: &[&str] = &[
    "net.listen",
    "net.accept",
    "net.local_addr",
    "net.narrow",
    "net.resolve",
    "net.connect",
    "net.connect_addr",
    "net.read",
    "net.write",
    "net.write_all",
    "net.shutdown",
    "net.set_read_timeout",
    "net.set_write_timeout",
    "net.peer_addr",
    "net.close",
    "tls.connect",
    "tls.close",
];

pub(crate) const MAP_BODIES: &[&str] = &[
    "$intrinsic.SetElements_open",
    "$intrinsic.SetElements_next",
    "$intrinsic.MapInsertionOrder_open",
    "$intrinsic.MapInsertionOrder_next",
    "map.get",
    "map.contains",
    "map.remove",
    "set.remove",
    "set.contains",
    "map.insert",
    "set.insert",
];

pub const INLINE_OPERATIONS: &[&str] = &[
    "$intrinsic.heap",
    "codec.encode",
    "codec.decode",
    "slots.empty",
    "bytes.from_list",
    "bytes.len",
    "map.empty",
    "map.len",
    "set.empty",
    "set.len",
    "$intrinsic.MapInsertionOrder_len",
    "sorted_map.empty",
    "$intrinsic.SortedMapOrder_len",
    "array.len",
    "forget",
    "iter.next",
    "list.len",
    "simd.add",
    "simd.splat",
    "string.byte_len",
    "string.scalars",
    "string.positions",
];

/// A physical argument class of a Hirsel ABI v1 entry (`hirsel_lir::abi_types::PhysicalType`
/// restated, since this crate does not depend on the LIR crate in production).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Physical {
    I8,
    I16,
    I32,
    I64,
    F32,
    F64,
    Pointer,
}

/// One export called as a `RuntimePrimitive`: its symbol, whether it takes the hidden
/// caller destination, the size and alignment of the result its output pointer names, and
/// the physical slots of its declared parameters in order. The context and a status result
/// are implicit. The driver checks every emitted call's `FnAbi` against its row here, so a
/// declaration and its export cannot drift apart.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AbiExport {
    pub symbol: &'static str,
    pub destination: bool,
    pub result: Option<(u64, u32)>,
    pub parameters: &'static [Physical],
}

pub const HIRSEL_ABI_EXPORTS: &[AbiExport] = &[
    AbiExport {
        symbol: "hirsel_rt_v2_task_scope_enter",
        destination: true,
        result: Some((8, 8)),
        parameters: &[Physical::Pointer],
    },
    AbiExport {
        symbol: "hirsel_rt_v2_task_scope_exit",
        destination: false,
        result: None,
        parameters: &[Physical::Pointer, Physical::I8],
    },
    AbiExport {
        symbol: "hirsel_rt_v2_task_spawn",
        destination: true,
        result: Some((8, 8)),
        parameters: &[Physical::Pointer, Physical::Pointer, Physical::Pointer],
    },
    AbiExport {
        symbol: "hirsel_rt_v2_task_finish",
        destination: false,
        result: None,
        parameters: &[Physical::Pointer, Physical::Pointer, Physical::I64],
    },
    AbiExport {
        symbol: "hirsel_rt_v2_task_join",
        destination: false,
        result: None,
        parameters: &[
            Physical::Pointer,
            Physical::Pointer,
            Physical::Pointer,
            Physical::I64,
        ],
    },
    AbiExport {
        symbol: "hirsel_rt_v2_task_cancel",
        destination: false,
        result: None,
        parameters: &[Physical::Pointer, Physical::Pointer],
    },
    AbiExport {
        symbol: "hirsel_rt_v2_task_detach",
        destination: false,
        result: None,
        parameters: &[Physical::Pointer, Physical::Pointer],
    },
    AbiExport {
        symbol: "hirsel_rt_v2_channel_open",
        destination: true,
        result: Some((16, 8)),
        parameters: &[Physical::Pointer],
    },
    AbiExport {
        symbol: "hirsel_rt_v2_sender_dup",
        destination: true,
        result: Some((8, 8)),
        parameters: &[Physical::Pointer, Physical::Pointer],
    },
    AbiExport {
        symbol: "hirsel_rt_v2_channel_send",
        destination: false,
        result: Some((4, 4)),
        parameters: &[Physical::Pointer, Physical::Pointer],
    },
    AbiExport {
        symbol: "hirsel_rt_v2_mutex_create",
        destination: true,
        result: Some((8, 8)),
        parameters: &[Physical::Pointer],
    },
    AbiExport {
        symbol: "hirsel_rt_v2_mutex_take",
        destination: false,
        result: Some((4, 4)),
        parameters: &[Physical::Pointer, Physical::Pointer],
    },
    AbiExport {
        symbol: "hirsel_rt_v2_mutex_put",
        destination: false,
        result: None,
        parameters: &[Physical::Pointer, Physical::Pointer],
    },
    AbiExport {
        symbol: "hirsel_rt_v2_mutex_kill",
        destination: false,
        result: None,
        parameters: &[Physical::Pointer],
    },
    AbiExport {
        symbol: "hirsel_rt_v2_mutex_cause",
        destination: false,
        result: None,
        parameters: &[Physical::Pointer],
    },
    AbiExport {
        symbol: "hirsel_rt_v2_atomic_create",
        destination: true,
        result: Some((8, 8)),
        parameters: &[Physical::Pointer, Physical::I64],
    },
    AbiExport {
        symbol: "hirsel_rt_v2_atomic_load",
        destination: false,
        result: None,
        parameters: &[Physical::Pointer, Physical::Pointer, Physical::I64],
    },
    AbiExport {
        symbol: "hirsel_rt_v2_atomic_store",
        destination: false,
        result: None,
        parameters: &[Physical::Pointer, Physical::Pointer, Physical::I64],
    },
    AbiExport {
        symbol: "hirsel_rt_v2_task_canceled",
        destination: false,
        result: Some((4, 4)),
        parameters: &[Physical::Pointer],
    },
    AbiExport {
        symbol: "hirsel_rt_v2_isolate_begin",
        destination: false,
        result: Some((8, 8)),
        parameters: &[Physical::Pointer],
    },
    AbiExport {
        symbol: "hirsel_rt_v2_isolate_count",
        destination: false,
        result: Some((8, 8)),
        parameters: &[Physical::Pointer],
    },
    AbiExport {
        symbol: "hirsel_rt_v2_isolate_kind",
        destination: false,
        result: Some((4, 4)),
        parameters: &[Physical::Pointer, Physical::I64],
    },
    AbiExport {
        symbol: "hirsel_rt_v2_isolate_rule",
        destination: true,
        result: Some((24, 8)),
        parameters: &[Physical::Pointer, Physical::I64],
    },
    AbiExport {
        symbol: "hirsel_rt_v2_isolate_definition",
        destination: true,
        result: Some((24, 8)),
        parameters: &[Physical::Pointer, Physical::I64],
    },
    AbiExport {
        symbol: "hirsel_rt_v2_isolate_line",
        destination: false,
        result: Some((8, 8)),
        parameters: &[Physical::Pointer, Physical::I64],
    },
    AbiExport {
        symbol: "hirsel_rt_v2_isolate_column",
        destination: false,
        result: Some((8, 8)),
        parameters: &[Physical::Pointer, Physical::I64],
    },
    AbiExport {
        symbol: "hirsel_rt_v2_isolate_spawn",
        destination: true,
        result: Some((8, 8)),
        parameters: &[Physical::Pointer, Physical::I64],
    },
    AbiExport {
        symbol: "hirsel_rt_v2_channel_receive",
        destination: false,
        result: Some((4, 4)),
        parameters: &[Physical::Pointer, Physical::Pointer],
    },
    AbiExport {
        symbol: "hirsel_rt_v2_tls_connect",
        destination: true,
        result: Some((24, 8)),
        parameters: &[
            Physical::Pointer,
            Physical::Pointer,
            Physical::Pointer,
            Physical::I64,
            Physical::I64,
            Physical::Pointer,
        ],
    },
    AbiExport {
        symbol: "hirsel_rt_v2_tls_close",
        destination: true,
        result: Some((16, 4)),
        parameters: &[Physical::Pointer, Physical::Pointer],
    },
    AbiExport {
        symbol: "hirsel_rt_v2_net_narrow",
        destination: true,
        result: Some((24, 8)),
        parameters: &[Physical::Pointer, Physical::Pointer],
    },
    AbiExport {
        symbol: "hirsel_rt_v2_net_resolve",
        destination: true,
        result: Some((24, 8)),
        parameters: &[
            Physical::Pointer,
            Physical::Pointer,
            Physical::I64,
            Physical::I64,
            Physical::I16,
        ],
    },
    AbiExport {
        symbol: "hirsel_rt_v2_net_connect",
        destination: true,
        result: Some((24, 8)),
        parameters: &[
            Physical::Pointer,
            Physical::Pointer,
            Physical::I64,
            Physical::I64,
            Physical::I16,
            Physical::Pointer,
        ],
    },
    AbiExport {
        symbol: "hirsel_rt_v2_net_connect_addr",
        destination: true,
        result: Some((24, 8)),
        parameters: &[Physical::Pointer, Physical::Pointer, Physical::Pointer],
    },
    AbiExport {
        symbol: "hirsel_rt_v2_net_read",
        destination: true,
        result: Some((24, 8)),
        parameters: &[
            Physical::Pointer,
            Physical::Pointer,
            Physical::Pointer,
            Physical::I64,
        ],
    },
    AbiExport {
        symbol: "hirsel_rt_v2_net_read_into",
        destination: true,
        result: Some((24, 8)),
        parameters: &[
            Physical::Pointer,
            Physical::Pointer,
            Physical::Pointer,
            Physical::I64,
        ],
    },
    AbiExport {
        symbol: "hirsel_rt_v2_net_write",
        destination: true,
        result: Some((24, 8)),
        parameters: &[
            Physical::Pointer,
            Physical::Pointer,
            Physical::Pointer,
            Physical::I64,
            Physical::I64,
        ],
    },
    AbiExport {
        symbol: "hirsel_rt_v2_net_write_all",
        destination: true,
        result: Some((16, 4)),
        parameters: &[
            Physical::Pointer,
            Physical::Pointer,
            Physical::Pointer,
            Physical::I64,
            Physical::I64,
        ],
    },
    AbiExport {
        symbol: "hirsel_rt_v2_net_shutdown",
        destination: true,
        result: Some((16, 4)),
        parameters: &[Physical::Pointer, Physical::Pointer, Physical::Pointer],
    },
    AbiExport {
        symbol: "hirsel_rt_v2_net_set_read_timeout",
        destination: true,
        result: Some((16, 4)),
        parameters: &[Physical::Pointer, Physical::Pointer, Physical::Pointer],
    },
    AbiExport {
        symbol: "hirsel_rt_v2_net_set_write_timeout",
        destination: true,
        result: Some((16, 4)),
        parameters: &[Physical::Pointer, Physical::Pointer, Physical::Pointer],
    },
    AbiExport {
        symbol: "hirsel_rt_v2_net_peer_addr",
        destination: true,
        result: Some((28, 4)),
        parameters: &[Physical::Pointer, Physical::Pointer],
    },
    AbiExport {
        symbol: "hirsel_rt_v2_net_listen",
        destination: true,
        result: Some((24, 8)),
        parameters: &[Physical::Pointer, Physical::Pointer],
    },
    AbiExport {
        symbol: "hirsel_rt_v2_net_accept",
        destination: true,
        result: Some((40, 8)),
        parameters: &[Physical::Pointer, Physical::Pointer],
    },
    AbiExport {
        symbol: "hirsel_rt_v2_net_local_addr",
        destination: true,
        result: Some((28, 4)),
        parameters: &[Physical::Pointer, Physical::Pointer],
    },
    AbiExport {
        symbol: "hirsel_rt_v2_net_close",
        destination: false,
        result: None,
        parameters: &[Physical::Pointer, Physical::Pointer],
    },
    AbiExport {
        symbol: "hirsel_rt_v2_string_as_bytes",
        destination: true,
        result: Some((24, 8)),
        parameters: &[Physical::Pointer, Physical::I64, Physical::I64],
    },
    AbiExport {
        symbol: "hirsel_rt_v2_bytes_slice",
        destination: true,
        result: Some((24, 8)),
        parameters: &[
            Physical::Pointer,
            Physical::I64,
            Physical::I64,
            Physical::I64,
            Physical::I64,
        ],
    },
    AbiExport {
        symbol: "hirsel_rt_v2_string_position",
        destination: true,
        result: Some((16, 8)),
        parameters: &[
            Physical::Pointer,
            Physical::I64,
            Physical::I64,
            Physical::I64,
        ],
    },
    AbiExport {
        symbol: "hirsel_rt_v2_string_byte_offset",
        destination: false,
        result: Some((8, 8)),
        parameters: &[
            Physical::Pointer,
            Physical::I64,
            Physical::I64,
            Physical::Pointer,
        ],
    },
    AbiExport {
        symbol: "hirsel_rt_v2_path_text",
        destination: true,
        result: Some((24, 8)),
        parameters: &[Physical::Pointer],
    },
    AbiExport {
        symbol: "hirsel_rt_v2_file_read_bytes",
        destination: true,
        result: Some((32, 8)),
        parameters: &[Physical::Pointer, Physical::Pointer],
    },
    AbiExport {
        symbol: "hirsel_rt_v2_file_walk",
        destination: true,
        result: Some((32, 8)),
        parameters: &[Physical::Pointer],
    },
    AbiExport {
        symbol: "hirsel_rt_v2_string_from_bytes",
        destination: true,
        result: Some((32, 8)),
        parameters: &[Physical::Pointer, Physical::I64, Physical::I64],
    },
    AbiExport {
        symbol: "hirsel_rt_v2_string_slice",
        destination: true,
        result: Some((24, 8)),
        parameters: &[
            Physical::Pointer,
            Physical::I64,
            Physical::I64,
            Physical::Pointer,
            Physical::Pointer,
        ],
    },
    AbiExport {
        symbol: "hirsel_rt_v2_io_monotonic_clock",
        destination: true,
        result: Some((16, 8)),
        parameters: &[Physical::Pointer],
    },
    AbiExport {
        symbol: "hirsel_rt_v2_io_wall_clock",
        destination: true,
        result: Some((16, 8)),
        parameters: &[Physical::Pointer],
    },
    AbiExport {
        symbol: "hirsel_rt_v2_clock_monotonic",
        destination: true,
        result: Some((16, 8)),
        parameters: &[Physical::Pointer],
    },
    AbiExport {
        symbol: "hirsel_rt_v2_clock_wall",
        destination: true,
        result: Some((16, 8)),
        parameters: &[Physical::Pointer],
    },
    AbiExport {
        symbol: "hirsel_rt_v2_clock_sleep_until",
        destination: true,
        result: Some((12, 4)),
        parameters: &[Physical::Pointer, Physical::Pointer],
    },
    AbiExport {
        symbol: "hirsel_rt_v2_instant_checked_add",
        destination: true,
        result: Some((16, 8)),
        parameters: &[Physical::Pointer, Physical::Pointer, Physical::I64],
    },
    AbiExport {
        symbol: "hirsel_rt_v2_random_bytes",
        destination: true,
        result: Some((32, 8)),
        parameters: &[Physical::Pointer, Physical::I64],
    },
    AbiExport {
        symbol: "hirsel_rt_v2_instant_ticks",
        destination: false,
        result: Some((8, 8)),
        parameters: &[Physical::Pointer],
    },
    AbiExport {
        symbol: "hirsel_rt_v2_io_tasks",
        destination: true,
        result: Some((16, 8)),
        parameters: &[Physical::Pointer],
    },
    AbiExport {
        symbol: "hirsel_rt_v2_io_net",
        destination: true,
        result: Some((16, 8)),
        parameters: &[Physical::Pointer],
    },
    AbiExport {
        symbol: "hirsel_rt_v2_io_process",
        destination: true,
        result: Some((16, 8)),
        parameters: &[Physical::Pointer],
    },
    AbiExport {
        symbol: "hirsel_rt_v2_proc_run",
        destination: true,
        result: Some((64, 8)),
        parameters: &[
            Physical::Pointer,
            Physical::Pointer,
            Physical::Pointer,
            Physical::Pointer,
            Physical::I64,
            Physical::I64,
            Physical::Pointer,
            Physical::I64,
        ],
    },
    AbiExport {
        symbol: "hirsel_rt_v2_test_process",
        destination: true,
        result: Some((8, 8)),
        parameters: &[Physical::Pointer, Physical::Pointer],
    },
    AbiExport {
        symbol: "hirsel_rt_v2_io_random",
        destination: true,
        result: Some((16, 8)),
        parameters: &[Physical::Pointer],
    },
    AbiExport {
        symbol: "hirsel_rt_v2_test_check",
        destination: false,
        result: None,
        parameters: &[Physical::Pointer, Physical::I8],
    },
    AbiExport {
        symbol: "hirsel_rt_v2_test_check_case",
        destination: false,
        result: None,
        parameters: &[Physical::Pointer, Physical::I8, Physical::I64],
    },
    AbiExport {
        symbol: "hirsel_rt_v2_test_fail",
        destination: false,
        result: None,
        parameters: &[Physical::Pointer],
    },
    AbiExport {
        symbol: "hirsel_rt_v2_test_dir",
        destination: true,
        result: Some((8, 8)),
        parameters: &[Physical::Pointer, Physical::Pointer],
    },
    AbiExport {
        symbol: "hirsel_rt_v2_test_net",
        destination: true,
        result: Some((8, 8)),
        parameters: &[Physical::Pointer, Physical::Pointer, Physical::Pointer],
    },
    AbiExport {
        symbol: "hirsel_rt_v2_io_dir",
        destination: true,
        result: Some((16, 8)),
        parameters: &[Physical::Pointer, Physical::Pointer],
    },
    AbiExport {
        symbol: "hirsel_rt_v2_dir_narrow",
        destination: true,
        result: Some((16, 8)),
        parameters: &[Physical::Pointer, Physical::Pointer],
    },
    AbiExport {
        symbol: "hirsel_rt_v2_file_create",
        destination: true,
        result: Some((16, 8)),
        parameters: &[Physical::Pointer, Physical::Pointer],
    },
    AbiExport {
        symbol: "hirsel_rt_v2_file_read",
        destination: true,
        result: Some((16, 8)),
        parameters: &[
            Physical::Pointer,
            Physical::Pointer,
            Physical::Pointer,
            Physical::I64,
        ],
    },
    AbiExport {
        symbol: "hirsel_rt_v2_file_seek",
        destination: true,
        result: Some((12, 4)),
        parameters: &[Physical::Pointer, Physical::Pointer, Physical::I64],
    },
    AbiExport {
        symbol: "hirsel_rt_v2_file_write",
        destination: true,
        result: Some((16, 8)),
        parameters: &[
            Physical::Pointer,
            Physical::Pointer,
            Physical::Pointer,
            Physical::I64,
            Physical::I64,
        ],
    },
    AbiExport {
        symbol: "hirsel_rt_v2_file_append",
        destination: true,
        result: Some((12, 4)),
        parameters: &[
            Physical::Pointer,
            Physical::Pointer,
            Physical::Pointer,
            Physical::I64,
            Physical::I64,
        ],
    },
    AbiExport {
        symbol: "hirsel_rt_v2_file_open",
        destination: true,
        result: Some((16, 8)),
        parameters: &[Physical::Pointer, Physical::Pointer],
    },
    AbiExport {
        symbol: "hirsel_rt_v2_file_narrow",
        destination: true,
        result: Some((16, 8)),
        parameters: &[Physical::Pointer, Physical::Pointer],
    },
    AbiExport {
        symbol: "hirsel_rt_v2_file_create_atomic",
        destination: true,
        result: Some((16, 8)),
        parameters: &[Physical::Pointer, Physical::Pointer],
    },
    AbiExport {
        symbol: "hirsel_rt_v2_file_commit",
        destination: true,
        result: Some((12, 4)),
        parameters: &[Physical::Pointer, Physical::Pointer],
    },
    AbiExport {
        symbol: "hirsel_rt_v2_file_read_dir",
        destination: true,
        result: Some((16, 8)),
        parameters: &[Physical::Pointer, Physical::Pointer],
    },
    AbiExport {
        symbol: "hirsel_rt_v2_file_metadata",
        destination: true,
        result: Some((64, 16)),
        parameters: &[Physical::Pointer, Physical::Pointer],
    },
    AbiExport {
        symbol: "hirsel_rt_v2_file_mkdir",
        destination: true,
        result: Some((12, 4)),
        parameters: &[Physical::Pointer, Physical::Pointer],
    },
    AbiExport {
        symbol: "hirsel_rt_v2_file_remove_dir",
        destination: true,
        result: Some((12, 4)),
        parameters: &[Physical::Pointer, Physical::Pointer],
    },
    AbiExport {
        symbol: "hirsel_rt_v2_file_rename",
        destination: true,
        result: Some((12, 4)),
        parameters: &[Physical::Pointer, Physical::Pointer, Physical::Pointer],
    },
    AbiExport {
        symbol: "hirsel_rt_v2_file_size",
        destination: true,
        result: Some((16, 8)),
        parameters: &[Physical::Pointer, Physical::Pointer],
    },
    AbiExport {
        symbol: "hirsel_rt_v2_file_remove",
        destination: true,
        result: Some((12, 4)),
        parameters: &[Physical::Pointer, Physical::Pointer],
    },
    AbiExport {
        symbol: "hirsel_rt_v2_file_write_bytes",
        destination: true,
        result: Some((12, 4)),
        parameters: &[
            Physical::Pointer,
            Physical::Pointer,
            Physical::Pointer,
            Physical::I64,
            Physical::I64,
        ],
    },
    AbiExport {
        symbol: "hirsel_rt_v2_file_write_all",
        destination: true,
        result: Some((12, 4)),
        parameters: &[
            Physical::Pointer,
            Physical::Pointer,
            Physical::Pointer,
            Physical::I64,
            Physical::I64,
        ],
    },
    AbiExport {
        symbol: "hirsel_rt_v2_path_of",
        destination: true,
        result: Some((8, 8)),
        parameters: &[Physical::Pointer, Physical::I64, Physical::I64],
    },
    AbiExport {
        symbol: "hirsel_rt_v2_arg_count",
        destination: false,
        result: Some((8, 8)),
        parameters: &[Physical::Pointer],
    },
    AbiExport {
        symbol: "hirsel_rt_v2_arg_text",
        destination: true,
        result: Some((32, 8)),
        parameters: &[Physical::Pointer, Physical::I64],
    },
    AbiExport {
        symbol: "hirsel_rt_v2_arg_path",
        destination: true,
        result: Some((16, 8)),
        parameters: &[Physical::Pointer, Physical::I64],
    },
    AbiExport {
        symbol: "hirsel_rt_v2_unicode_scalars_open",
        destination: true,
        result: Some((32, 8)),
        parameters: &[Physical::Pointer, Physical::I64, Physical::I64],
    },
    AbiExport {
        symbol: "hirsel_rt_v2_unicode_scalars_next",
        destination: true,
        result: Some((8, 4)),
        parameters: &[Physical::Pointer],
    },
    AbiExport {
        symbol: "hirsel_rt_v2_scalar_positions_open",
        destination: true,
        result: Some((32, 8)),
        parameters: &[Physical::Pointer, Physical::I64, Physical::I64],
    },
    AbiExport {
        symbol: "hirsel_rt_v2_scalar_positions_next",
        destination: true,
        result: Some((16, 8)),
        parameters: &[Physical::Pointer],
    },
    AbiExport {
        symbol: "hirsel_rt_v2_io_stdin",
        destination: true,
        result: Some((16, 8)),
        parameters: &[Physical::Pointer],
    },
    AbiExport {
        symbol: "hirsel_rt_v2_io_stderr",
        destination: true,
        result: Some((16, 8)),
        parameters: &[Physical::Pointer],
    },
    AbiExport {
        symbol: "hirsel_rt_v2_stdin_read",
        destination: true,
        result: Some((16, 8)),
        parameters: &[Physical::Pointer, Physical::Pointer, Physical::I64],
    },
    AbiExport {
        symbol: "hirsel_rt_v2_err_print",
        destination: true,
        result: Some((12, 4)),
        parameters: &[
            Physical::Pointer,
            Physical::Pointer,
            Physical::I64,
            Physical::I64,
        ],
    },
    AbiExport {
        symbol: "hirsel_rt_v2_env_get",
        destination: true,
        result: Some((40, 8)),
        parameters: &[
            Physical::Pointer,
            Physical::Pointer,
            Physical::I64,
            Physical::I64,
        ],
    },
    AbiExport {
        symbol: "hirsel_rt_v2_io_stdout",
        destination: true,
        result: Some((16, 8)),
        parameters: &[Physical::Pointer],
    },
    AbiExport {
        symbol: "hirsel_rt_v2_out_print",
        destination: true,
        result: Some((12, 4)),
        parameters: &[
            Physical::Pointer,
            Physical::Pointer,
            Physical::I64,
            Physical::I64,
        ],
    },
];

/// The scalar core-library operations legalized into LIR instructions.
pub const SCALAR_INTRINSICS: &[&str] = &[
    "int.widen",
    "int.try_narrow",
    "int.narrow",
    "int.truncate",
    "int.wrapping_add",
    "int.saturating_add",
    "int.checked_add",
    "int.div_floor",
    "int.mod",
    "int.div_exact",
    "float.min",
    "float.max",
    "float.min_number",
    "float.max_number",
    "float.total_order",
    "float.mul_add",
    "float.to_i32",
    "float.from_int",
    "f32.bits",
    "f32.from_bits",
    "f64.bits",
    "f64.from_bits",
];
pub fn manifest(rows: impl IntoIterator<Item = OperationRow>) -> Result<Manifest, String> {
    let rows: Vec<_> = rows.into_iter().collect();
    let mut seen = std::collections::BTreeSet::new();
    if rows.iter().any(|r| !seen.insert(r.id.clone())) {
        return Err("duplicate native operation identity".into());
    }
    let mut manifest = Manifest::skeleton(rows.iter().map(|r| r.id.clone()));
    for row in rows {
        manifest
            .replace(classify(row)?)
            .map_err(|e: ManifestError| format!("{e:?}"))?;
    }
    Ok(manifest)
}

/// Explicit identities: additions to the registry fail coverage until reviewed here.
pub const NATIVE_PATHS: &[&str] = &[
    "codec.encode",
    "codec.decode",
    "net.narrow",
    "net.resolve",
    "net.connect",
    "net.connect_addr",
    "net.read",
    "net.read_into",
    "net.write",
    "net.write_all",
    "net.shutdown",
    "net.set_read_timeout",
    "net.set_write_timeout",
    "net.peer_addr",
    "net.close",
    "tls.connect",
    "tls.close",
    "net.listen",
    "net.accept",
    "net.local_addr",
    "test.net",
    "$intrinsic.MapInsertionOrder_len",
    "$intrinsic.SortedMapOrder_len",
    "$intrinsic.ArrayConsume_next",
    "$intrinsic.ArrayConsume_open",
    "$intrinsic.InlineConsume_next",
    "$intrinsic.InlineConsume_open",
    "$intrinsic.ListConsume_next",
    "$intrinsic.ListConsume_open",
    "$intrinsic.MapInsertionOrder_next",
    "$intrinsic.MapInsertionOrder_open",
    "$intrinsic.NetReadInto",
    "$intrinsic.TaskScopeEnter",
    "$intrinsic.TaskScopeExit",
    "$intrinsic.TaskSpawn",
    "$intrinsic.TaskFinish",
    "$intrinsic.TaskJoin",
    "$intrinsic.ChannelSend",
    "$intrinsic.ChannelReceive",
    "$intrinsic.MutexCreate",
    "$intrinsic.MutexTake",
    "$intrinsic.MutexPut",
    "$intrinsic.MutexKill",
    "$intrinsic.MutexCause",
    "$intrinsic.AtomicCreate",
    "$intrinsic.AtomicLoad",
    "$intrinsic.AtomicStore",
    "$intrinsic.TaskCanceled",
    "$intrinsic.IsolateBegin",
    "$intrinsic.IsolateCount",
    "$intrinsic.IsolateKind",
    "$intrinsic.IsolateRule",
    "$intrinsic.IsolateDefinition",
    "$intrinsic.IsolateLine",
    "$intrinsic.IsolateColumn",
    "$intrinsic.IsolateSpawn",
    "$intrinsic.OrderCompare_i128",
    "$intrinsic.OrderCompare_i16",
    "$intrinsic.OrderCompare_i32",
    "$intrinsic.OrderCompare_i64",
    "$intrinsic.OrderCompare_i8",
    "$intrinsic.OrderCompare_u128",
    "$intrinsic.OrderCompare_u16",
    "$intrinsic.OrderCompare_u32",
    "$intrinsic.OrderCompare_u64",
    "$intrinsic.OrderCompare_u8",
    "$intrinsic.ScalarPositions_next",
    "$intrinsic.ScalarPositions_open",
    "$intrinsic.SetElements_next",
    "$intrinsic.SetElements_open",
    "$intrinsic.SortedMapOrder_next",
    "$intrinsic.SortedMapOrder_open",
    "$intrinsic.UnicodeScalars_next",
    "$intrinsic.UnicodeScalars_open",
    "$intrinsic.heap",
    "arena.bounded",
    "arena.seal",
    "arena.with",
    "arg.count",
    "arg.path",
    "arg.text",
    "array.len",
    "atomic.create",
    "atomic.load",
    "atomic.store",
    "bytes.len",
    "bytes.from_list",
    "bytes.slice",
    "string.as_bytes",
    "string.position",
    "string.byte_offset",
    "path.text",
    "failure.check",
    "channel.open",
    "channel.receive",
    "channel.send",
    "copy_to",
    "dir.narrow",
    "file.append",
    "file.commit",
    "file.create",
    "file.create_atomic",
    "file.narrow",
    "file.open",
    "file.read",
    "file.read_bytes",
    "file.read_dir",
    "file.metadata",
    "file.mkdir",
    "file.rename",
    "file.remove_dir",
    "file.remove",
    "file.seek",
    "file.size",
    "file.walk",
    "file.write",
    "file.write_all",
    "file.write_bytes",
    "f32.bits",
    "f32.from_bits",
    "f64.bits",
    "f64.from_bits",
    "float.from_int",
    "float.max",
    "float.max_number",
    "float.min",
    "float.min_number",
    "float.mul_add",
    "float.to_i32",
    "float.total_order",
    "forget",
    "int.checked_add",
    "int.div_exact",
    "int.div_floor",
    "int.mod",
    "int.narrow",
    "int.saturating_add",
    "int.truncate",
    "int.try_narrow",
    "int.unchecked_add",
    "int.widen",
    "int.wrapping_add",
    "io.monotonic_clock",
    "io.wall_clock",
    "clock.monotonic",
    "clock.wall",
    "instant.ticks",
    "io.dir",
    "io.net",
    "io.random",
    "io.stdin",
    "io.stderr",
    "stdin.read",
    "err.print",
    "env.get",
    "io.stdout",
    "io.tasks",
    "io.process",
    "proc.run",
    "random.bytes",
    "clock.sleep_until",
    "instant.checked_add",
    "test.process",
    "iter.from",
    "iter.map",
    "iter.next",
    "list.len",
    "list.push",
    "list.pop",
    "list.remove",
    "list.swap",
    "list.reserve",
    "map.empty",
    "map.len",
    "map.get",
    "map.contains",
    "map.remove",
    "map.insert",
    "mutex.create",
    "mutex.with",
    "out.print",
    "path.of",
    "region.with",
    "reservations.insert",
    "sender.dup",
    "set.contains",
    "set.empty",
    "set.len",
    "set.insert",
    "set.remove",
    "shared.freeze",
    "shared.thaw",
    "simd.add",
    "simd.splat",
    "slots.empty",
    "slots.get",
    "slots.insert",
    "slots.remove",
    "sorted_map.empty",
    "sorted_map.insert",
    "stock.get",
    "stock.put",
    "store.transact",
    "string.byte_len",
    "string.from_bytes",
    "string.positions",
    "string.scalars",
    "string.slice",
    "task.async",
    "task.await",
    "task.cancel",
    "task.check_canceled",
    "task.detach",
    "task.isolate",
    "task.scope",
    "test.check",
    "test.check_case",
    "test.dir",
    "test.fail",
    "test.net",
];

/// Stable declaration ownership transport, used by the independently retained inventory.
pub fn ownership_encoding(ownership: &CallOwnership) -> String {
    fn cleanup(c: &CleanupEffect) -> String {
        match c {
            CleanupEffect::NoAlloc => "n".into(),
            CleanupEffect::MayAllocate => "a".into(),
            CleanupEffect::DependsOn(ps) => format!(
                "[{}]",
                ps.iter().map(u32::to_string).collect::<Vec<_>>().join(",")
            ),
        }
    }
    let mut out = ownership
        .parameters
        .iter()
        .map(|p| {
            format!(
                "{}:{}:{}",
                p.ordinal,
                match p.passing {
                    Passing::Borrow => "b",
                    Passing::Take => "t",
                    Passing::Mut => "m",
                },
                cleanup(&p.cleanup)
            )
        })
        .collect::<Vec<_>>()
        .join(";");
    out.push_str("->");
    out.push_str(&cleanup(&ownership.result_cleanup));
    out
}
