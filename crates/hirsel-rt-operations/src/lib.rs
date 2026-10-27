//! Native operation inventory and typed adapters. Unknown identity never selects a fallback.
pub mod descriptors;

use hirsel_core_contract::{
    CallOwnership, CleanupEffect, Disposition, M2Feature, Manifest, ManifestError, OperationRow,
    Passing, Unavailable,
};

include!(concat!(env!("OUT_DIR"), "/reviewed.rs"));

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
    let (expected_digest, expected_ownership) =
        reviewed(&name).ok_or_else(|| format!("missing native declaration identity: {name}"))?;
    if row.id.declaration_digest.0 != expected_digest {
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
            | "shared.with"
            | "arena.seal"
            | "region.with"
            | "arena.with"
            | "arena.bounded"
            | "copy_to"
            | "failure.check"
            | "sorted_map.insert"
            | "sorted_map.get"
            | "sorted_map.remove"
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
            | "list.truncate"
            | "list.pop"
            | "list.remove"
            | "list.swap"
            | "list.reserve"
            | "list.push"
            | "iter.from"
            | "$intrinsic.ListConsume_open"
            | "$intrinsic.ListConsume_next"
            | "$intrinsic.ArrayConsume_open"
            | "$intrinsic.ArrayConsume_next"
            | "$intrinsic.InlineConsume_open"
            | "$intrinsic.InlineConsume_next"
            | "net.read_into"
            | "read_half.read_into"
            | "cpu.map_scoped"
            | "task.scope"
            | "task.scope_until"
            | "task.async"
            | "task.await"
            | "task.await_at"
            | "channel.open"
            | "channel.send"
            | "channel.receive"
            | "mutex.create"
            | "mutex.with"
            | "atomic.create"
            | "atomic.load"
            | "atomic.store"
            | "task.isolate"
            | "task.check_canceled"
            | "mapped.with"
            | "runner.compute"
            | "runner.call"
            | "runner.spawn"
            | "runner.grants"
    ) || MAP_BODIES.contains(&name.as_str())
    {
        row.dispositions.native = Disposition::Body;
        return Ok(row);
    }
    row.failure = matches!(
        name.as_str(),
        "string.slice" | "bytes.slice" | "bytes.append_from_history"
    )
    .then(|| hirsel_core_contract::RuleCode("SEM-3".into()));
    if matches!(
        name.as_str(),
        "bytes.sha1" | "bytes.sha224" | "bytes.sha256"
    ) {
        row.failure = Some(hirsel_core_contract::RuleCode("SEM-1".into()));
    }
    if name == "bytes.replace_range" {
        row.failure = Some(hirsel_core_contract::RuleCode("MEM-14".into()));
    }
    let symbol = match name.as_str() {
        "proc.spawn" => Some("hirsel_rt_v2_proc_spawn_at_site".into()),
        "proc.spawn_pipeline" => Some("hirsel_rt_v2_proc_spawn_pipeline_at_site".into()),
        "child.write" => Some("hirsel_rt_v2_child_write_at_site".into()),
        "child.close_stdin" => Some("hirsel_rt_v2_child_close_stdin_at_site".into()),
        "child.wait" => Some("hirsel_rt_v2_child_wait_at_site".into()),
        "proc.wait_group" => Some("hirsel_rt_v2_proc_wait_group_at_site".into()),
        "out.print" => Some("hirsel_rt_v2_out_print_at_site".into()),
        "clock.sleep_until" => Some("hirsel_rt_v2_clock_sleep_until_at_site".into()),
        // Fixed declaration signatures: every entry has its exact physical ABI
        // in HIRSEL_ABI_EXPORTS; generic callbacks remain compiled bodies.
        "bytes.json_number_end"
        | "bytes.integer"
        | "bytes.decimal_word"
        | "bytes.find_any"
        | "bytes.base64_encode"
        | "bytes.base64_decode"
        | "bytes.sha1"
        | "bytes.sha224"
        | "bytes.sha256"
        | "bytes.sha384"
        | "bytes.sha512"
        | "bytes.sha512_256"
        | "string.from_bytes"
        | "string.slice"
        | "io.stdout"
        | "bytes.append"
        | "bytes.append_from_history"
        | "bytes.replace_range"
        | "string.position"
        | "string.byte_offset"
        | "bytes.slice"
        | "path.text"
        | "out.write"
        | "io.dir"
        | "dir.narrow"
        | "dir.read_only"
        | "path.of"
        | "arg.path"
        | "arg.len"
        | "arg.text"
        | "file.open"
        | "file.read"
        | "file.seek"
        | "file.open_append"
        | "file.create"
        | "file.write"
        | "file.append"
        | "file.narrow"
        | "file.create_atomic"
        | "file.commit"
        | "file.map"
        | "file.watch"
        | "watcher.next"
        | "file.size"
        | "file.read_dir"
        | "file.metadata"
        | "file.mkdir"
        | "file.rename"
        | "file.remove_dir"
        | "file.set_permissions"
        | "dir.remove_all"
        | "file.remove"
        | "file.write_bytes"
        | "file.write_all"
        | "file.sync"
        | "file.put_new"
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
        | "test.clock"
        | "test.seed"
        | "test.tasks"
        | "test.log"
        | "test.expect_snapshot"
        | "test.dir"
        | "test.dir_event"
        | "test.net"
        | "test.crash"
        | "file.read_bytes"
        | "file.walk"
        | "io.stdin"
        | "io.stderr"
        | "io.signals"
        | "signal.wait"
        | "stdin.read"
        | "err.print"
        | "err.write"
        | "env.get"
        | "env.narrow"
        | "io.process"
        | "proc.run"
        | "proc.pipeline"
        | "proc.narrow"
        | "child.read_stdout"
        | "child.read_stderr"
        | "child.kill"
        | "test.process"
        | "random.bytes"
        | "io.telemetry"
        | "capture.begin"
        | "capture.context"
        | "capture.event"
        | "capture.finish"
        | "capture.log"
        | "capture.measure"
        | "instant.checked_add" => Some(format!("hirsel_rt_v2_{}", name.replace('.', "_"))),
        // A ChildGroup's streams and kill are a Child's entries on its handle.
        "group.write" => Some("hirsel_rt_v2_child_write_at_site".into()),
        "group.close_stdin" => Some("hirsel_rt_v2_child_close_stdin_at_site".into()),
        "group.read_stdout" | "group.kill" => Some(format!(
            "hirsel_rt_v2_child_{}",
            name.strip_prefix("group.").unwrap_or_default()
        )),
        // Streams and listeners use the same explicit Net policy.
        "net.udp_bind"
        | "udp.send_to"
        | "udp.recv_from"
        | "udp.close"
        | "net.listen"
        | "net.unix_connect"
        | "net.unix_listen"
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
        | "net.split"
        | "net.join"
        | "tls.connect"
        | "tls.connect_as"
        | "tls.connect_with"
        | "tls.accept"
        | "tls.peer"
        | "tls.negotiated_protocol"
        | "tls.close" => Some(format!("hirsel_rt_v2_{}", name.replace('.', "_"))),
        // `net.read_into`'s compiled body calls this with its buffer's size.
        "$intrinsic.NetReadInto" => Some("hirsel_rt_v2_net_read_into".into()),
        // A half's operations are its stream's entries on the half's handle.
        name if HALF_EXPORTS.iter().any(|(n, _)| *n == name) => HALF_EXPORTS
            .iter()
            .find(|(n, _)| *n == name)
            .map(|(_, symbol)| (*symbol).into()),
        // The task runtime (`tasks.rs`) behind the compiled task bodies.
        "$intrinsic.CpuMapScoped" => Some("hirsel_rt_v2_cpu_map_scoped".into()),
        "$intrinsic.TaskScopeEnter" => Some("hirsel_rt_v2_task_scope_enter".into()),
        "$intrinsic.TaskScopeExit" => Some("hirsel_rt_v2_task_scope_exit".into()),
        "$intrinsic.TaskScopeEnterUntil" => Some("hirsel_rt_v2_task_scope_enter_until".into()),
        "$intrinsic.TaskScopeExitUntil" => Some("hirsel_rt_v2_task_scope_exit_until".into()),
        "$intrinsic.TaskSpawn" => Some("hirsel_rt_v2_task_spawn".into()),
        "$intrinsic.TaskFinish" => Some("hirsel_rt_v2_task_finish".into()),
        "$intrinsic.TaskJoin" => Some("hirsel_rt_v2_task_join".into()),
        "$intrinsic.TaskCanceled" => Some("hirsel_rt_v2_task_canceled".into()),
        // `mapped.with`'s compiled body lends its body this view, then checks the file.
        "$intrinsic.MappedView" => Some("hirsel_rt_v2_mapped_view".into()),
        "$intrinsic.MappedCheck" => Some("hirsel_rt_v2_mapped_check".into()),
        "task.cancel" | "task.detach" | "task.set" | "task.add" | "task.await_any"
        | "task.cancel_all" | "task.detach_all" => {
            Some(format!("hirsel_rt_v2_{}", name.replace('.', "_")))
        }
        "$intrinsic.TaskSetHas" => Some("hirsel_rt_v2_task_set_has".into()),
        "$intrinsic.TaskSetTake" => Some("hirsel_rt_v2_task_set_take".into()),
        // Channels (CONC-6), in the task runtime: the compiled open, send and receive bodies
        // pass a channel its element layout and move a message's `Option` image through these
        // entries.
        "sender.dup" => Some("hirsel_rt_v2_sender_dup".into()),
        "$intrinsic.ChannelOpen" => Some("hirsel_rt_v2_channel_open".into()),
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
        // `shared.thaw`'s compiled body asks the runtime whether its root is the last.
        "$intrinsic.SharedThawTurn" => Some("hirsel_rt_v2_thaw_turn".into()),
        // `task.isolate`'s compiled body reads its caught chain through these (`isolate.rs`).
        "$intrinsic.IsolateBegin" => Some("hirsel_rt_v2_isolate_begin".into()),
        "$intrinsic.IsolateEnd" => Some("hirsel_rt_v2_isolate_end".into()),
        "$intrinsic.IsolateCount" => Some("hirsel_rt_v2_isolate_count".into()),
        "$intrinsic.IsolateKind" => Some("hirsel_rt_v2_isolate_kind".into()),
        "$intrinsic.IsolateRule" => Some("hirsel_rt_v2_isolate_rule".into()),
        "$intrinsic.IsolateDefinition" => Some("hirsel_rt_v2_isolate_definition".into()),
        "$intrinsic.IsolateLine" => Some("hirsel_rt_v2_isolate_line".into()),
        "$intrinsic.IsolateColumn" => Some("hirsel_rt_v2_isolate_column".into()),
        "$intrinsic.IsolateSpawn" => Some("hirsel_rt_v2_isolate_spawn".into()),
        // Runners (RUN-1..4, `runners.rs`): the grants and narrowings are runtime entries;
        // the calls' compiled bodies send, wait and reply through the intrinsics.
        "io.runner" | "test.runner" | "runner.narrow" | "runner.only" | "runner.entry_path" => {
            Some(format!("hirsel_rt_v2_{}", name.replace('.', "_")))
        }
        "$intrinsic.RunnerSend" => Some("hirsel_rt_v2_runner_send".into()),
        "$intrinsic.RunnerWait" => Some("hirsel_rt_v2_runner_wait".into()),
        "$intrinsic.RunnerTake" => Some("hirsel_rt_v2_runner_take".into()),
        "$intrinsic.RunnerEntry" => Some("hirsel_rt_v2_runner_entry".into()),
        "$intrinsic.RunnerArgument" => Some("hirsel_rt_v2_runner_argument".into()),
        "$intrinsic.RunnerIo" => Some("hirsel_rt_v2_runner_io".into()),
        "$intrinsic.RunnerTasks" => Some("hirsel_rt_v2_runner_tasks".into()),
        "$intrinsic.RunnerReply" => Some("hirsel_rt_v2_runner_reply".into()),
        "$intrinsic.RunnerGrants" => Some("hirsel_rt_v2_runner_grants".into()),
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
            | "dir.read_only"
            | "path.of"
            | "arg.path"
            | "arg.len"
            | "arg.text"
            | "io.monotonic_clock"
            | "io.wall_clock"
            | "clock.monotonic"
            | "clock.wall"
            | "clock.sleep_until"
            | "io.random"
            | "random.bytes"
            | "io.telemetry"
            | "capture.begin"
            | "capture.context"
            | "capture.event"
            | "capture.finish"
            | "capture.log"
            | "capture.measure"
            | "io.stdin"
            | "io.stderr"
            | "io.signals"
            | "signal.wait"
            | "stdin.read"
            | "env.get"
            | "env.narrow"
            | "io.stdout"
            | "io.tasks"
            | "io.net"
            | "test.clock"
            | "test.dir"
            | "test.net"
            | "io.process"
            | "proc.run"
            | "proc.pipeline"
            | "proc.spawn"
            | "proc.narrow"
            | "child.write"
            | "child.close_stdin"
            | "child.read_stdout"
            | "child.read_stderr"
            | "child.wait"
            | "child.kill"
            | "proc.spawn_pipeline"
            | "group.write"
            | "group.close_stdin"
            | "group.read_stdout"
            | "proc.wait_group"
            | "group.kill"
            | "test.process"
            | "file.open"
            | "file.read"
            | "file.seek"
            | "file.open_append"
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
            | "file.set_permissions"
            | "dir.remove_all"
            | "file.size"
            | "file.remove"
            | "file.read_bytes"
            | "file.write_bytes"
            | "file.write_all"
            | "file.walk"
            | "file.sync"
            | "file.put_new"
            | "file.map"
            | "$intrinsic.MappedView"
            | "$intrinsic.MappedCheck"
            | "file.watch"
            | "watcher.next"
            | "test.dir_event"
    ) || NET_EXPORTS.contains(&name.as_str())
        || HALF_EXPORTS.iter().any(|(n, _)| *n == name)
        || name == "$intrinsic.NetReadInto"
    {
        row.failure = Some(hirsel_core_contract::RuleCode("CAP-2".into()));
    }
    // A scripted client's or child's unmet Expect fails these LANG-9, so the runtime
    // records their failures itself (`hirsel_core_contract::RUNTIME_RULES`).
    if matches!(
        name.as_str(),
        "net.write"
            | "net.write_all"
            | "net.close"
            | "net.join"
            | "write_half.write"
            | "write_half.write_all"
            | "read_half.close"
            | "write_half.close"
            | "tls.connect"
            | "tls.connect_as"
            | "tls.connect_with"
            | "tls.accept"
            | "tls.close"
            | "child.write"
            | "child.close_stdin"
            | "child.wait"
            | "proc.spawn_pipeline"
            | "group.write"
            | "group.close_stdin"
            | "proc.wait_group"
    ) {
        row.failure = None;
    }
    if matches!(
        name.as_str(),
        "test.check" | "test.check_case" | "test.fail" | "test.expect_snapshot" | "test.crash"
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
    "net.udp_bind",
    "udp.send_to",
    "udp.recv_from",
    "udp.close",
    "net.listen",
    "net.unix_connect",
    "net.unix_listen",
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
    "net.split",
    "net.join",
    "tls.connect",
    "tls.connect_as",
    "tls.connect_with",
    "tls.accept",
    "tls.peer",
    "tls.negotiated_protocol",
    "tls.close",
];

/// The `net.split` halves' operations and their entries: each is its stream operation's
/// entry on the half's handle, except a write half's shutdown, which names no direction.
pub const HALF_EXPORTS: &[(&str, &str)] = &[
    ("read_half.read", "hirsel_rt_v2_net_read"),
    ("$intrinsic.ReadHalfReadInto", "hirsel_rt_v2_net_read_into"),
    ("read_half.set_timeout", "hirsel_rt_v2_net_set_read_timeout"),
    ("read_half.close", "hirsel_rt_v2_net_close"),
    ("write_half.write", "hirsel_rt_v2_net_write"),
    ("write_half.write_all", "hirsel_rt_v2_net_write_all"),
    ("write_half.shutdown", "hirsel_rt_v2_write_half_shutdown"),
    (
        "write_half.set_timeout",
        "hirsel_rt_v2_net_set_write_timeout",
    ),
    ("write_half.close", "hirsel_rt_v2_net_close"),
];

pub(crate) const MAP_BODIES: &[&str] = &[
    "$intrinsic.SetElements_open",
    "$intrinsic.SetElements_next",
    "$intrinsic.MapInsertionOrder_open",
    "$intrinsic.MapInsertionOrder_advance",
    "$intrinsic.MapInsertionOrder_next",
    "map.get",
    "map.contains",
    "map.remove",
    "set.remove",
    "set.contains",
    "map.insert",
    "map.upsert_with",
    "map.update_with",
    "map.update_value_with",
    "set.insert",
];

pub const INLINE_OPERATIONS: &[&str] = &[
    "$intrinsic.TextFinish",
    "$intrinsic.heap",
    "codec.encode",
    "codec.decode",
    "slots.empty",
    "bytes.from_list",
    "string.as_bytes",
    "bytes.len",
    "map.empty",
    "map.len",
    "set.empty",
    "set.len",
    "$intrinsic.MapInsertionOrder_len",
    "sorted_map.empty",
    "sorted_map.len",
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

/// A physical argument class of a Hirsel ABI v2 entry (`hirsel_lir::abi_types::PhysicalType`
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
    pub caller_site: bool,
    pub symbol: &'static str,
    pub destination: bool,
    pub result: Option<(u64, u32)>,
    pub parameters: &'static [Physical],
}

pub const HIRSEL_ABI_EXPORTS: &[AbiExport] = &[
    AbiExport {
        caller_site: false,
        symbol: "hirsel_rt_v2_io_telemetry",
        destination: true,
        result: Some((16, 8)),
        parameters: &[Physical::Pointer],
    },
    AbiExport {
        caller_site: false,
        symbol: "hirsel_rt_v2_capture_begin",
        destination: true,
        result: Some((16, 8)),
        parameters: &[
            Physical::Pointer,
            Physical::Pointer,
            Physical::I64,
            Physical::I64,
            Physical::Pointer,
            Physical::I64,
            Physical::I64,
        ],
    },
    AbiExport {
        caller_site: false,
        symbol: "hirsel_rt_v2_capture_context",
        destination: true,
        result: Some((24, 8)),
        parameters: &[Physical::Pointer, Physical::Pointer],
    },
    AbiExport {
        caller_site: false,
        symbol: "hirsel_rt_v2_capture_event",
        destination: true,
        result: Some((8, 4)),
        parameters: &[
            Physical::Pointer,
            Physical::Pointer,
            Physical::Pointer,
            Physical::I64,
            Physical::I64,
        ],
    },
    AbiExport {
        caller_site: false,
        symbol: "hirsel_rt_v2_capture_finish",
        destination: true,
        result: Some((8, 4)),
        parameters: &[
            Physical::Pointer,
            Physical::Pointer,
            Physical::Pointer,
            Physical::I64,
            Physical::I64,
        ],
    },
    AbiExport {
        caller_site: false,
        symbol: "hirsel_rt_v2_capture_log",
        destination: true,
        result: Some((8, 4)),
        parameters: &[
            Physical::Pointer,
            Physical::Pointer,
            Physical::I64,
            Physical::I64,
        ],
    },
    AbiExport {
        caller_site: false,
        symbol: "hirsel_rt_v2_capture_measure",
        destination: true,
        result: Some((8, 4)),
        parameters: &[
            Physical::Pointer,
            Physical::Pointer,
            Physical::I64,
            Physical::I64,
        ],
    },
    AbiExport {
        caller_site: false,
        symbol: "hirsel_rt_v2_cpu_map_scoped",
        destination: false,
        result: Some((4, 4)),
        parameters: &[
            Physical::Pointer,
            Physical::I32,
            Physical::I32,
            Physical::I64,
        ],
    },
    AbiExport {
        caller_site: false,
        symbol: "hirsel_rt_v2_task_scope_enter",
        destination: true,
        result: Some((8, 8)),
        parameters: &[Physical::Pointer],
    },
    AbiExport {
        caller_site: true,
        symbol: "hirsel_rt_v2_task_scope_exit",
        destination: false,
        result: None,
        parameters: &[Physical::Pointer, Physical::I8],
    },
    AbiExport {
        caller_site: false,
        symbol: "hirsel_rt_v2_task_scope_enter_until",
        destination: true,
        result: Some((8, 8)),
        parameters: &[Physical::Pointer, Physical::Pointer],
    },
    AbiExport {
        caller_site: true,
        symbol: "hirsel_rt_v2_task_scope_exit_until",
        destination: false,
        result: Some((1, 1)),
        parameters: &[Physical::Pointer, Physical::I8],
    },
    AbiExport {
        caller_site: true,
        symbol: "hirsel_rt_v2_task_spawn",
        destination: true,
        result: Some((8, 8)),
        parameters: &[
            Physical::Pointer,
            Physical::Pointer,
            Physical::Pointer,
            Physical::I64,
        ],
    },
    AbiExport {
        caller_site: false,
        symbol: "hirsel_rt_v2_task_finish",
        destination: false,
        result: None,
        parameters: &[
            Physical::Pointer,
            Physical::Pointer,
            Physical::I64,
            Physical::I32,
        ],
    },
    AbiExport {
        caller_site: true,
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
        caller_site: false,
        symbol: "hirsel_rt_v2_task_cancel",
        destination: false,
        result: None,
        parameters: &[Physical::Pointer, Physical::Pointer],
    },
    AbiExport {
        caller_site: false,
        symbol: "hirsel_rt_v2_task_detach",
        destination: false,
        result: None,
        parameters: &[Physical::Pointer, Physical::Pointer],
    },
    AbiExport {
        caller_site: false,
        symbol: "hirsel_rt_v2_task_set",
        destination: true,
        result: Some((8, 8)),
        parameters: &[Physical::Pointer],
    },
    AbiExport {
        caller_site: false,
        symbol: "hirsel_rt_v2_task_add",
        destination: false,
        result: None,
        parameters: &[Physical::Pointer, Physical::Pointer, Physical::Pointer],
    },
    AbiExport {
        caller_site: true,
        symbol: "hirsel_rt_v2_task_await_any",
        destination: true,
        result: Some((16, 8)),
        parameters: &[Physical::Pointer, Physical::Pointer],
    },
    AbiExport {
        caller_site: false,
        symbol: "hirsel_rt_v2_task_set_has",
        destination: false,
        result: Some((1, 1)),
        parameters: &[Physical::Pointer, Physical::Pointer, Physical::I64],
    },
    AbiExport {
        caller_site: false,
        symbol: "hirsel_rt_v2_task_set_take",
        destination: true,
        result: Some((8, 8)),
        parameters: &[Physical::Pointer, Physical::Pointer, Physical::I64],
    },
    AbiExport {
        caller_site: true,
        symbol: "hirsel_rt_v2_task_cancel_all",
        destination: false,
        result: None,
        parameters: &[Physical::Pointer, Physical::Pointer],
    },
    AbiExport {
        caller_site: false,
        symbol: "hirsel_rt_v2_task_detach_all",
        destination: false,
        result: None,
        parameters: &[Physical::Pointer, Physical::Pointer],
    },
    AbiExport {
        caller_site: false,
        symbol: "hirsel_rt_v2_channel_open",
        destination: true,
        result: Some((16, 8)),
        parameters: &[Physical::Pointer, Physical::Pointer, Physical::I32],
    },
    AbiExport {
        caller_site: false,
        symbol: "hirsel_rt_v2_sender_dup",
        destination: true,
        result: Some((8, 8)),
        parameters: &[Physical::Pointer, Physical::Pointer],
    },
    AbiExport {
        caller_site: false,
        symbol: "hirsel_rt_v2_channel_send",
        destination: false,
        result: Some((4, 4)),
        parameters: &[Physical::Pointer, Physical::Pointer],
    },
    AbiExport {
        caller_site: false,
        symbol: "hirsel_rt_v2_mutex_create",
        destination: true,
        result: Some((8, 8)),
        parameters: &[
            Physical::Pointer,
            Physical::Pointer,
            Physical::I32,
            Physical::I32,
        ],
    },
    AbiExport {
        caller_site: false,
        symbol: "hirsel_rt_v2_mutex_take",
        destination: false,
        result: Some((4, 4)),
        parameters: &[Physical::Pointer, Physical::Pointer],
    },
    AbiExport {
        caller_site: false,
        symbol: "hirsel_rt_v2_mutex_put",
        destination: false,
        result: None,
        parameters: &[Physical::Pointer, Physical::Pointer],
    },
    AbiExport {
        caller_site: false,
        symbol: "hirsel_rt_v2_mutex_kill",
        destination: false,
        result: None,
        parameters: &[Physical::Pointer],
    },
    AbiExport {
        caller_site: false,
        symbol: "hirsel_rt_v2_mutex_cause",
        destination: false,
        result: None,
        parameters: &[Physical::Pointer],
    },
    AbiExport {
        caller_site: false,
        symbol: "hirsel_rt_v2_atomic_create",
        destination: true,
        result: Some((8, 8)),
        parameters: &[
            Physical::Pointer,
            Physical::I64,
            Physical::I64,
            Physical::I64,
        ],
    },
    AbiExport {
        caller_site: false,
        symbol: "hirsel_rt_v2_thaw_turn",
        destination: false,
        result: Some((1, 1)),
        parameters: &[Physical::Pointer],
    },
    AbiExport {
        caller_site: false,
        symbol: "hirsel_rt_v2_atomic_load",
        destination: false,
        result: None,
        parameters: &[Physical::Pointer, Physical::Pointer, Physical::I64],
    },
    AbiExport {
        caller_site: false,
        symbol: "hirsel_rt_v2_atomic_store",
        destination: false,
        result: None,
        parameters: &[Physical::Pointer, Physical::Pointer, Physical::I64],
    },
    AbiExport {
        caller_site: false,
        symbol: "hirsel_rt_v2_task_canceled",
        destination: false,
        result: Some((4, 4)),
        parameters: &[Physical::Pointer],
    },
    AbiExport {
        caller_site: false,
        symbol: "hirsel_rt_v2_isolate_begin",
        destination: false,
        result: Some((8, 8)),
        parameters: &[Physical::Pointer],
    },
    AbiExport {
        caller_site: false,
        symbol: "hirsel_rt_v2_isolate_end",
        destination: false,
        result: None,
        parameters: &[Physical::Pointer],
    },
    AbiExport {
        caller_site: false,
        symbol: "hirsel_rt_v2_isolate_count",
        destination: false,
        result: Some((8, 8)),
        parameters: &[Physical::Pointer],
    },
    AbiExport {
        caller_site: false,
        symbol: "hirsel_rt_v2_isolate_kind",
        destination: false,
        result: Some((4, 4)),
        parameters: &[Physical::Pointer, Physical::I64],
    },
    AbiExport {
        caller_site: false,
        symbol: "hirsel_rt_v2_isolate_rule",
        destination: true,
        result: Some((24, 8)),
        parameters: &[Physical::Pointer, Physical::I64],
    },
    AbiExport {
        caller_site: false,
        symbol: "hirsel_rt_v2_isolate_definition",
        destination: true,
        result: Some((24, 8)),
        parameters: &[Physical::Pointer, Physical::I64],
    },
    AbiExport {
        caller_site: false,
        symbol: "hirsel_rt_v2_isolate_line",
        destination: false,
        result: Some((8, 8)),
        parameters: &[Physical::Pointer, Physical::I64],
    },
    AbiExport {
        caller_site: false,
        symbol: "hirsel_rt_v2_isolate_column",
        destination: false,
        result: Some((8, 8)),
        parameters: &[Physical::Pointer, Physical::I64],
    },
    AbiExport {
        caller_site: false,
        symbol: "hirsel_rt_v2_isolate_spawn",
        destination: true,
        result: Some((8, 8)),
        parameters: &[Physical::Pointer, Physical::I64],
    },
    AbiExport {
        caller_site: false,
        symbol: "hirsel_rt_v2_io_runner",
        destination: true,
        result: Some((16, 8)),
        parameters: &[
            Physical::Pointer,
            Physical::Pointer,
            Physical::I64,
            Physical::I64,
        ],
    },
    AbiExport {
        caller_site: false,
        symbol: "hirsel_rt_v2_test_runner",
        destination: true,
        result: Some((8, 8)),
        parameters: &[Physical::Pointer],
    },
    AbiExport {
        caller_site: false,
        symbol: "hirsel_rt_v2_runner_narrow",
        destination: true,
        result: Some((16, 8)),
        parameters: &[Physical::Pointer, Physical::Pointer],
    },
    AbiExport {
        caller_site: false,
        symbol: "hirsel_rt_v2_runner_only",
        destination: true,
        result: Some((16, 8)),
        parameters: &[Physical::Pointer, Physical::Pointer],
    },
    AbiExport {
        caller_site: false,
        symbol: "hirsel_rt_v2_runner_entry_path",
        destination: true,
        result: Some((24, 8)),
        parameters: &[Physical::Pointer],
    },
    AbiExport {
        caller_site: false,
        symbol: "hirsel_rt_v2_runner_send",
        destination: false,
        result: Some((8, 8)),
        parameters: &[
            Physical::Pointer,
            Physical::Pointer,
            Physical::Pointer,
            Physical::I64,
            Physical::I64,
            Physical::Pointer,
            Physical::Pointer,
            Physical::I64,
        ],
    },
    AbiExport {
        caller_site: false,
        symbol: "hirsel_rt_v2_runner_wait",
        destination: false,
        result: Some((8, 8)),
        parameters: &[Physical::I64],
    },
    AbiExport {
        caller_site: false,
        symbol: "hirsel_rt_v2_runner_take",
        destination: true,
        result: Some((24, 8)),
        parameters: &[Physical::I64],
    },
    AbiExport {
        caller_site: false,
        symbol: "hirsel_rt_v2_runner_entry",
        destination: true,
        result: Some((16, 8)),
        parameters: &[],
    },
    AbiExport {
        caller_site: false,
        symbol: "hirsel_rt_v2_runner_argument",
        destination: true,
        result: Some((24, 8)),
        parameters: &[],
    },
    AbiExport {
        caller_site: false,
        symbol: "hirsel_rt_v2_runner_io",
        destination: true,
        result: Some((8, 8)),
        parameters: &[],
    },
    AbiExport {
        caller_site: false,
        symbol: "hirsel_rt_v2_runner_tasks",
        destination: true,
        result: Some((8, 8)),
        parameters: &[],
    },
    AbiExport {
        caller_site: false,
        symbol: "hirsel_rt_v2_runner_reply",
        destination: false,
        result: None,
        parameters: &[
            Physical::I64,
            Physical::Pointer,
            Physical::I64,
            Physical::I64,
        ],
    },
    AbiExport {
        caller_site: false,
        symbol: "hirsel_rt_v2_runner_grants",
        destination: false,
        result: Some((8, 8)),
        parameters: &[Physical::Pointer],
    },
    AbiExport {
        caller_site: true,
        symbol: "hirsel_rt_v2_channel_receive",
        destination: false,
        result: Some((4, 4)),
        parameters: &[Physical::Pointer, Physical::Pointer],
    },
    AbiExport {
        caller_site: false,
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
        caller_site: false,
        symbol: "hirsel_rt_v2_tls_connect_as",
        destination: true,
        result: Some((24, 8)),
        parameters: &[
            Physical::Pointer,
            Physical::Pointer,
            Physical::Pointer,
            Physical::I64,
            Physical::I64,
            Physical::Pointer,
            Physical::Pointer,
        ],
    },
    AbiExport {
        caller_site: false,
        symbol: "hirsel_rt_v2_tls_connect_with",
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
        caller_site: false,
        symbol: "hirsel_rt_v2_tls_peer",
        destination: true,
        result: Some((32, 8)),
        parameters: &[Physical::Pointer, Physical::Pointer],
    },
    AbiExport {
        caller_site: false,
        symbol: "hirsel_rt_v2_tls_negotiated_protocol",
        destination: true,
        result: Some((32, 8)),
        parameters: &[Physical::Pointer, Physical::Pointer],
    },
    AbiExport {
        caller_site: false,
        symbol: "hirsel_rt_v2_tls_accept",
        destination: true,
        result: Some((24, 8)),
        parameters: &[Physical::Pointer, Physical::Pointer, Physical::Pointer],
    },
    AbiExport {
        caller_site: false,
        symbol: "hirsel_rt_v2_tls_close",
        destination: true,
        result: Some((16, 4)),
        parameters: &[Physical::Pointer, Physical::Pointer],
    },
    AbiExport {
        caller_site: false,
        symbol: "hirsel_rt_v2_net_narrow",
        destination: true,
        result: Some((24, 8)),
        parameters: &[Physical::Pointer, Physical::Pointer],
    },
    AbiExport {
        caller_site: false,
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
        caller_site: false,
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
        caller_site: false,
        symbol: "hirsel_rt_v2_net_connect_addr",
        destination: true,
        result: Some((24, 8)),
        parameters: &[Physical::Pointer, Physical::Pointer, Physical::Pointer],
    },
    AbiExport {
        caller_site: false,
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
        caller_site: false,
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
        caller_site: false,
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
        caller_site: false,
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
        caller_site: false,
        symbol: "hirsel_rt_v2_net_shutdown",
        destination: true,
        result: Some((16, 4)),
        parameters: &[Physical::Pointer, Physical::Pointer, Physical::Pointer],
    },
    AbiExport {
        caller_site: false,
        symbol: "hirsel_rt_v2_net_set_read_timeout",
        destination: true,
        result: Some((16, 4)),
        parameters: &[Physical::Pointer, Physical::Pointer, Physical::Pointer],
    },
    AbiExport {
        caller_site: false,
        symbol: "hirsel_rt_v2_net_set_write_timeout",
        destination: true,
        result: Some((16, 4)),
        parameters: &[Physical::Pointer, Physical::Pointer, Physical::Pointer],
    },
    AbiExport {
        caller_site: false,
        symbol: "hirsel_rt_v2_net_peer_addr",
        destination: true,
        result: Some((28, 4)),
        parameters: &[Physical::Pointer, Physical::Pointer],
    },
    AbiExport {
        caller_site: false,
        symbol: "hirsel_rt_v2_net_udp_bind",
        destination: true,
        result: Some((24, 8)),
        parameters: &[Physical::Pointer, Physical::Pointer],
    },
    AbiExport {
        caller_site: false,
        symbol: "hirsel_rt_v2_udp_send_to",
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
        caller_site: false,
        symbol: "hirsel_rt_v2_udp_recv_from",
        destination: true,
        result: Some((48, 8)),
        parameters: &[
            Physical::Pointer,
            Physical::Pointer,
            Physical::Pointer,
            Physical::I64,
        ],
    },
    AbiExport {
        caller_site: false,
        symbol: "hirsel_rt_v2_udp_close",
        destination: false,
        result: None,
        parameters: &[Physical::Pointer, Physical::Pointer],
    },
    AbiExport {
        caller_site: false,
        symbol: "hirsel_rt_v2_net_listen",
        destination: true,
        result: Some((24, 8)),
        parameters: &[Physical::Pointer, Physical::Pointer],
    },
    AbiExport {
        caller_site: false,
        symbol: "hirsel_rt_v2_net_unix_connect",
        destination: true,
        result: Some((24, 8)),
        parameters: &[Physical::Pointer, Physical::Pointer],
    },
    AbiExport {
        caller_site: false,
        symbol: "hirsel_rt_v2_net_unix_listen",
        destination: true,
        result: Some((24, 8)),
        parameters: &[Physical::Pointer, Physical::Pointer],
    },
    AbiExport {
        caller_site: false,
        symbol: "hirsel_rt_v2_net_accept",
        destination: true,
        result: Some((40, 8)),
        parameters: &[Physical::Pointer, Physical::Pointer],
    },
    AbiExport {
        caller_site: false,
        symbol: "hirsel_rt_v2_net_local_addr",
        destination: true,
        result: Some((28, 4)),
        parameters: &[Physical::Pointer, Physical::Pointer],
    },
    AbiExport {
        caller_site: false,
        symbol: "hirsel_rt_v2_net_close",
        destination: false,
        result: None,
        parameters: &[Physical::Pointer, Physical::Pointer],
    },
    AbiExport {
        caller_site: false,
        symbol: "hirsel_rt_v2_net_split",
        destination: true,
        result: Some((16, 8)),
        parameters: &[Physical::Pointer, Physical::Pointer],
    },
    AbiExport {
        caller_site: false,
        symbol: "hirsel_rt_v2_net_join",
        destination: true,
        result: Some((24, 8)),
        parameters: &[Physical::Pointer, Physical::Pointer, Physical::Pointer],
    },
    AbiExport {
        caller_site: false,
        symbol: "hirsel_rt_v2_write_half_shutdown",
        destination: true,
        result: Some((16, 4)),
        parameters: &[Physical::Pointer, Physical::Pointer],
    },
    AbiExport {
        caller_site: false,
        symbol: "hirsel_rt_v2_bytes_append_from_history",
        destination: false,
        result: None,
        parameters: &[Physical::Pointer, Physical::I64, Physical::I64],
    },
    AbiExport {
        caller_site: false,
        symbol: "hirsel_rt_v2_bytes_replace_range",
        destination: false,
        result: None,
        parameters: &[
            Physical::Pointer,
            Physical::I64,
            Physical::Pointer,
            Physical::I64,
            Physical::I64,
        ],
    },
    AbiExport {
        caller_site: false,
        symbol: "hirsel_rt_v2_bytes_append",
        destination: false,
        result: None,
        parameters: &[
            Physical::Pointer,
            Physical::Pointer,
            Physical::I64,
            Physical::I64,
        ],
    },
    AbiExport {
        caller_site: false,
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
        caller_site: false,
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
        caller_site: false,
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
        caller_site: false,
        symbol: "hirsel_rt_v2_path_text",
        destination: true,
        result: Some((24, 8)),
        parameters: &[Physical::Pointer],
    },
    AbiExport {
        caller_site: false,
        symbol: "hirsel_rt_v2_file_read_bytes",
        destination: true,
        result: Some((32, 8)),
        parameters: &[Physical::Pointer, Physical::Pointer],
    },
    AbiExport {
        caller_site: false,
        symbol: "hirsel_rt_v2_file_walk",
        destination: true,
        result: Some((32, 8)),
        parameters: &[Physical::Pointer],
    },
    AbiExport {
        caller_site: false,
        symbol: "hirsel_rt_v2_bytes_json_number_end",
        destination: false,
        result: Some((16, 8)),
        parameters: &[
            Physical::Pointer,
            Physical::I64,
            Physical::I64,
            Physical::I64,
        ],
    },
    AbiExport {
        caller_site: false,
        symbol: "hirsel_rt_v2_bytes_integer",
        destination: false,
        result: Some((24, 8)),
        parameters: &[
            Physical::Pointer,
            Physical::I64,
            Physical::I64,
            Physical::I8,
            Physical::I8,
            Physical::I64,
            Physical::I64,
            Physical::I64,
            Physical::I64,
        ],
    },
    AbiExport {
        caller_site: false,
        symbol: "hirsel_rt_v2_bytes_decimal_word",
        destination: false,
        result: Some((8, 8)),
        parameters: &[Physical::I64, Physical::Pointer],
    },
    AbiExport {
        caller_site: false,
        symbol: "hirsel_rt_v2_bytes_find_any",
        destination: false,
        result: Some((8, 8)),
        parameters: &[
            Physical::Pointer,
            Physical::I64,
            Physical::I64,
            Physical::Pointer,
            Physical::I64,
            Physical::I64,
            Physical::I64,
        ],
    },
    AbiExport {
        caller_site: false,
        symbol: "hirsel_rt_v2_bytes_sha1",
        destination: false,
        result: Some((20, 1)),
        parameters: &[Physical::Pointer, Physical::I64, Physical::I64],
    },
    AbiExport {
        caller_site: false,
        symbol: "hirsel_rt_v2_bytes_sha224",
        destination: false,
        result: Some((28, 1)),
        parameters: &[Physical::Pointer, Physical::I64, Physical::I64],
    },
    AbiExport {
        caller_site: false,
        symbol: "hirsel_rt_v2_bytes_sha256",
        destination: false,
        result: Some((32, 1)),
        parameters: &[Physical::Pointer, Physical::I64, Physical::I64],
    },
    AbiExport {
        caller_site: false,
        symbol: "hirsel_rt_v2_bytes_sha384",
        destination: false,
        result: Some((48, 1)),
        parameters: &[Physical::Pointer, Physical::I64, Physical::I64],
    },
    AbiExport {
        caller_site: false,
        symbol: "hirsel_rt_v2_bytes_sha512",
        destination: false,
        result: Some((64, 1)),
        parameters: &[Physical::Pointer, Physical::I64, Physical::I64],
    },
    AbiExport {
        caller_site: false,
        symbol: "hirsel_rt_v2_bytes_sha512_256",
        destination: false,
        result: Some((32, 1)),
        parameters: &[Physical::Pointer, Physical::I64, Physical::I64],
    },
    AbiExport {
        caller_site: false,
        symbol: "hirsel_rt_v2_bytes_base64_encode",
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
        caller_site: false,
        symbol: "hirsel_rt_v2_bytes_base64_decode",
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
        caller_site: false,
        symbol: "hirsel_rt_v2_string_from_bytes",
        destination: true,
        result: Some((32, 8)),
        parameters: &[Physical::Pointer, Physical::I64, Physical::I64],
    },
    AbiExport {
        caller_site: false,
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
        caller_site: false,
        symbol: "hirsel_rt_v2_io_monotonic_clock",
        destination: true,
        result: Some((16, 8)),
        parameters: &[Physical::Pointer],
    },
    AbiExport {
        caller_site: false,
        symbol: "hirsel_rt_v2_io_wall_clock",
        destination: true,
        result: Some((16, 8)),
        parameters: &[Physical::Pointer],
    },
    AbiExport {
        caller_site: false,
        symbol: "hirsel_rt_v2_clock_monotonic",
        destination: true,
        result: Some((16, 8)),
        parameters: &[Physical::Pointer],
    },
    AbiExport {
        caller_site: false,
        symbol: "hirsel_rt_v2_clock_wall",
        destination: true,
        result: Some((16, 8)),
        parameters: &[Physical::Pointer],
    },
    AbiExport {
        caller_site: true,
        symbol: "hirsel_rt_v2_clock_sleep_until_at_site",
        destination: true,
        result: Some((12, 4)),
        parameters: &[Physical::Pointer, Physical::Pointer],
    },
    AbiExport {
        caller_site: false,
        symbol: "hirsel_rt_v2_clock_sleep_until",
        destination: true,
        result: Some((12, 4)),
        parameters: &[Physical::Pointer, Physical::Pointer],
    },
    AbiExport {
        caller_site: false,
        symbol: "hirsel_rt_v2_instant_checked_add",
        destination: true,
        result: Some((16, 8)),
        parameters: &[Physical::Pointer, Physical::Pointer, Physical::I64],
    },
    AbiExport {
        caller_site: false,
        symbol: "hirsel_rt_v2_random_bytes",
        destination: true,
        result: Some((32, 8)),
        parameters: &[Physical::Pointer, Physical::I64],
    },
    AbiExport {
        caller_site: false,
        symbol: "hirsel_rt_v2_instant_ticks",
        destination: false,
        result: Some((8, 8)),
        parameters: &[Physical::Pointer],
    },
    AbiExport {
        caller_site: false,
        symbol: "hirsel_rt_v2_io_tasks",
        destination: true,
        result: Some((16, 8)),
        parameters: &[Physical::Pointer],
    },
    AbiExport {
        caller_site: false,
        symbol: "hirsel_rt_v2_io_net",
        destination: true,
        result: Some((16, 8)),
        parameters: &[Physical::Pointer],
    },
    AbiExport {
        caller_site: false,
        symbol: "hirsel_rt_v2_io_process",
        destination: true,
        result: Some((16, 8)),
        parameters: &[Physical::Pointer],
    },
    AbiExport {
        caller_site: false,
        symbol: "hirsel_rt_v2_proc_run",
        destination: true,
        result: Some((64, 8)),
        parameters: &[
            Physical::Pointer,
            Physical::Pointer,
            Physical::Pointer,
            Physical::I64,
            Physical::I64,
            Physical::I64,
            Physical::Pointer,
        ],
    },
    AbiExport {
        caller_site: false,
        symbol: "hirsel_rt_v2_proc_pipeline",
        destination: true,
        result: Some((64, 8)),
        parameters: &[
            Physical::Pointer,
            Physical::Pointer,
            Physical::Pointer,
            Physical::I64,
            Physical::I64,
            Physical::I64,
            Physical::Pointer,
        ],
    },
    AbiExport {
        caller_site: false,
        symbol: "hirsel_rt_v2_test_process",
        destination: true,
        result: Some((8, 8)),
        parameters: &[Physical::Pointer, Physical::Pointer, Physical::Pointer],
    },
    AbiExport {
        caller_site: false,
        symbol: "hirsel_rt_v2_proc_narrow",
        destination: true,
        result: Some((16, 8)),
        parameters: &[Physical::Pointer, Physical::Pointer],
    },
    AbiExport {
        caller_site: false,
        symbol: "hirsel_rt_v2_env_narrow",
        destination: true,
        result: Some((16, 8)),
        parameters: &[Physical::Pointer, Physical::Pointer],
    },
    AbiExport {
        caller_site: false,
        symbol: "hirsel_rt_v2_proc_spawn",
        destination: true,
        result: Some((16, 8)),
        parameters: &[Physical::Pointer, Physical::Pointer],
    },
    AbiExport {
        caller_site: false,
        symbol: "hirsel_rt_v2_child_write",
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
        caller_site: false,
        symbol: "hirsel_rt_v2_child_close_stdin",
        destination: false,
        result: None,
        parameters: &[Physical::Pointer, Physical::Pointer],
    },
    AbiExport {
        caller_site: false,
        symbol: "hirsel_rt_v2_child_read_stdout",
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
        caller_site: false,
        symbol: "hirsel_rt_v2_child_read_stderr",
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
        caller_site: false,
        symbol: "hirsel_rt_v2_child_wait",
        destination: true,
        result: Some((12, 4)),
        parameters: &[Physical::Pointer, Physical::Pointer, Physical::Pointer],
    },
    AbiExport {
        caller_site: false,
        symbol: "hirsel_rt_v2_child_kill",
        destination: false,
        result: None,
        parameters: &[Physical::Pointer, Physical::Pointer, Physical::Pointer],
    },
    AbiExport {
        caller_site: false,
        symbol: "hirsel_rt_v2_proc_spawn_pipeline",
        destination: true,
        result: Some((16, 8)),
        parameters: &[Physical::Pointer, Physical::Pointer, Physical::I64],
    },
    AbiExport {
        caller_site: false,
        symbol: "hirsel_rt_v2_proc_wait_group",
        destination: true,
        result: Some((24, 8)),
        parameters: &[Physical::Pointer, Physical::Pointer, Physical::Pointer],
    },
    AbiExport {
        caller_site: false,
        symbol: "hirsel_rt_v2_io_random",
        destination: true,
        result: Some((16, 8)),
        parameters: &[Physical::Pointer],
    },
    AbiExport {
        caller_site: false,
        symbol: "hirsel_rt_v2_test_check",
        destination: false,
        result: None,
        parameters: &[Physical::Pointer, Physical::I8],
    },
    AbiExport {
        caller_site: false,
        symbol: "hirsel_rt_v2_test_check_case",
        destination: false,
        result: None,
        parameters: &[Physical::Pointer, Physical::I8, Physical::I64],
    },
    AbiExport {
        caller_site: false,
        symbol: "hirsel_rt_v2_test_crash",
        destination: false,
        result: None,
        parameters: &[Physical::Pointer, Physical::Pointer, Physical::I64],
    },
    AbiExport {
        caller_site: false,
        symbol: "hirsel_rt_v2_file_sync",
        destination: true,
        result: Some((12, 4)),
        parameters: &[Physical::Pointer, Physical::Pointer],
    },
    AbiExport {
        caller_site: false,
        symbol: "hirsel_rt_v2_file_put_new",
        destination: true,
        result: Some((12, 4)),
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
        caller_site: false,
        symbol: "hirsel_rt_v2_test_fail",
        destination: false,
        result: None,
        parameters: &[Physical::Pointer],
    },
    AbiExport {
        caller_site: false,
        symbol: "hirsel_rt_v2_test_clock",
        destination: true,
        result: Some((8, 8)),
        parameters: &[Physical::Pointer],
    },
    AbiExport {
        caller_site: false,
        symbol: "hirsel_rt_v2_test_seed",
        destination: false,
        result: Some((8, 8)),
        parameters: &[Physical::Pointer],
    },
    AbiExport {
        caller_site: false,
        symbol: "hirsel_rt_v2_test_tasks",
        destination: true,
        result: Some((8, 8)),
        parameters: &[Physical::Pointer],
    },
    AbiExport {
        caller_site: false,
        symbol: "hirsel_rt_v2_test_log",
        destination: false,
        result: None,
        parameters: &[
            Physical::Pointer,
            Physical::Pointer,
            Physical::I64,
            Physical::I64,
        ],
    },
    AbiExport {
        caller_site: false,
        symbol: "hirsel_rt_v2_test_expect_snapshot",
        destination: false,
        result: None,
        parameters: &[
            Physical::Pointer,
            Physical::Pointer,
            Physical::I64,
            Physical::I64,
            Physical::Pointer,
            Physical::I64,
            Physical::I64,
        ],
    },
    AbiExport {
        caller_site: false,
        symbol: "hirsel_rt_v2_test_dir",
        destination: true,
        result: Some((8, 8)),
        parameters: &[Physical::Pointer, Physical::Pointer],
    },
    AbiExport {
        caller_site: false,
        symbol: "hirsel_rt_v2_test_net",
        destination: true,
        result: Some((8, 8)),
        parameters: &[Physical::Pointer, Physical::Pointer, Physical::Pointer],
    },
    AbiExport {
        caller_site: false,
        symbol: "hirsel_rt_v2_io_dir",
        destination: true,
        result: Some((16, 8)),
        parameters: &[Physical::Pointer, Physical::Pointer],
    },
    AbiExport {
        caller_site: false,
        symbol: "hirsel_rt_v2_dir_narrow",
        destination: true,
        result: Some((16, 8)),
        parameters: &[Physical::Pointer, Physical::Pointer],
    },
    AbiExport {
        caller_site: false,
        symbol: "hirsel_rt_v2_dir_read_only",
        destination: true,
        result: Some((16, 8)),
        parameters: &[Physical::Pointer],
    },
    AbiExport {
        caller_site: false,
        symbol: "hirsel_rt_v2_file_open_append",
        destination: true,
        result: Some((16, 8)),
        parameters: &[Physical::Pointer, Physical::Pointer],
    },
    AbiExport {
        caller_site: false,
        symbol: "hirsel_rt_v2_file_create",
        destination: true,
        result: Some((16, 8)),
        parameters: &[Physical::Pointer, Physical::Pointer],
    },
    AbiExport {
        caller_site: false,
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
        caller_site: false,
        symbol: "hirsel_rt_v2_file_seek",
        destination: true,
        result: Some((12, 4)),
        parameters: &[Physical::Pointer, Physical::Pointer, Physical::I64],
    },
    AbiExport {
        caller_site: false,
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
        caller_site: false,
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
        caller_site: false,
        symbol: "hirsel_rt_v2_file_open",
        destination: true,
        result: Some((16, 8)),
        parameters: &[Physical::Pointer, Physical::Pointer],
    },
    AbiExport {
        caller_site: false,
        symbol: "hirsel_rt_v2_file_map",
        destination: true,
        result: Some((16, 8)),
        parameters: &[Physical::Pointer, Physical::Pointer],
    },
    AbiExport {
        caller_site: false,
        symbol: "hirsel_rt_v2_mapped_view",
        destination: true,
        result: Some((24, 8)),
        parameters: &[Physical::Pointer, Physical::Pointer],
    },
    AbiExport {
        caller_site: false,
        symbol: "hirsel_rt_v2_file_watch",
        destination: true,
        result: Some((16, 8)),
        parameters: &[Physical::Pointer, Physical::Pointer, Physical::I8],
    },
    AbiExport {
        caller_site: false,
        symbol: "hirsel_rt_v2_watcher_next",
        destination: true,
        result: Some((32, 8)),
        parameters: &[Physical::Pointer, Physical::Pointer],
    },
    AbiExport {
        caller_site: false,
        symbol: "hirsel_rt_v2_test_dir_event",
        destination: false,
        result: None,
        parameters: &[Physical::Pointer, Physical::Pointer, Physical::Pointer],
    },
    AbiExport {
        caller_site: false,
        symbol: "hirsel_rt_v2_mapped_check",
        destination: false,
        result: Some((4, 4)),
        parameters: &[Physical::Pointer, Physical::Pointer],
    },
    AbiExport {
        caller_site: false,
        symbol: "hirsel_rt_v2_file_narrow",
        destination: true,
        result: Some((16, 8)),
        parameters: &[Physical::Pointer, Physical::Pointer],
    },
    AbiExport {
        caller_site: false,
        symbol: "hirsel_rt_v2_file_create_atomic",
        destination: true,
        result: Some((16, 8)),
        parameters: &[Physical::Pointer, Physical::Pointer],
    },
    AbiExport {
        caller_site: false,
        symbol: "hirsel_rt_v2_file_commit",
        destination: true,
        result: Some((12, 4)),
        parameters: &[Physical::Pointer, Physical::Pointer],
    },
    AbiExport {
        caller_site: false,
        symbol: "hirsel_rt_v2_file_read_dir",
        destination: true,
        result: Some((32, 8)),
        parameters: &[Physical::Pointer, Physical::Pointer],
    },
    AbiExport {
        caller_site: false,
        symbol: "hirsel_rt_v2_file_metadata",
        destination: true,
        result: Some((80, 16)),
        parameters: &[Physical::Pointer, Physical::Pointer],
    },
    AbiExport {
        caller_site: false,
        symbol: "hirsel_rt_v2_file_mkdir",
        destination: true,
        result: Some((12, 4)),
        parameters: &[Physical::Pointer, Physical::Pointer],
    },
    AbiExport {
        caller_site: false,
        symbol: "hirsel_rt_v2_dir_remove_all",
        destination: true,
        result: Some((12, 4)),
        parameters: &[Physical::Pointer, Physical::Pointer],
    },
    AbiExport {
        caller_site: false,
        symbol: "hirsel_rt_v2_file_set_permissions",
        destination: true,
        result: Some((12, 4)),
        parameters: &[Physical::Pointer, Physical::Pointer, Physical::I32],
    },
    AbiExport {
        caller_site: false,
        symbol: "hirsel_rt_v2_file_remove_dir",
        destination: true,
        result: Some((12, 4)),
        parameters: &[Physical::Pointer, Physical::Pointer],
    },
    AbiExport {
        caller_site: false,
        symbol: "hirsel_rt_v2_file_rename",
        destination: true,
        result: Some((12, 4)),
        parameters: &[Physical::Pointer, Physical::Pointer, Physical::Pointer],
    },
    AbiExport {
        caller_site: false,
        symbol: "hirsel_rt_v2_file_size",
        destination: true,
        result: Some((16, 8)),
        parameters: &[Physical::Pointer, Physical::Pointer],
    },
    AbiExport {
        caller_site: false,
        symbol: "hirsel_rt_v2_file_remove",
        destination: true,
        result: Some((12, 4)),
        parameters: &[Physical::Pointer, Physical::Pointer],
    },
    AbiExport {
        caller_site: false,
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
        caller_site: false,
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
        caller_site: false,
        symbol: "hirsel_rt_v2_path_of",
        destination: true,
        result: Some((8, 8)),
        parameters: &[Physical::Pointer, Physical::I64, Physical::I64],
    },
    AbiExport {
        caller_site: false,
        symbol: "hirsel_rt_v2_arg_len",
        destination: false,
        result: Some((8, 8)),
        parameters: &[Physical::Pointer],
    },
    AbiExport {
        caller_site: false,
        symbol: "hirsel_rt_v2_arg_text",
        destination: true,
        result: Some((32, 8)),
        parameters: &[Physical::Pointer, Physical::I64],
    },
    AbiExport {
        caller_site: false,
        symbol: "hirsel_rt_v2_arg_path",
        destination: true,
        result: Some((16, 8)),
        parameters: &[Physical::Pointer, Physical::I64],
    },
    AbiExport {
        caller_site: false,
        symbol: "hirsel_rt_v2_unicode_scalars_open",
        destination: true,
        result: Some((32, 8)),
        parameters: &[Physical::Pointer, Physical::I64, Physical::I64],
    },
    AbiExport {
        caller_site: false,
        symbol: "hirsel_rt_v2_unicode_scalars_next",
        destination: true,
        result: Some((8, 4)),
        parameters: &[Physical::Pointer],
    },
    AbiExport {
        caller_site: false,
        symbol: "hirsel_rt_v2_scalar_positions_open",
        destination: true,
        result: Some((32, 8)),
        parameters: &[Physical::Pointer, Physical::I64, Physical::I64],
    },
    AbiExport {
        caller_site: false,
        symbol: "hirsel_rt_v2_scalar_positions_next",
        destination: true,
        result: Some((16, 8)),
        parameters: &[Physical::Pointer],
    },
    AbiExport {
        caller_site: false,
        symbol: "hirsel_rt_v2_io_signals",
        destination: true,
        result: Some((16, 8)),
        parameters: &[Physical::Pointer],
    },
    AbiExport {
        caller_site: false,
        symbol: "hirsel_rt_v2_signal_wait",
        destination: true,
        result: Some((12, 4)),
        parameters: &[Physical::Pointer, Physical::Pointer],
    },
    AbiExport {
        caller_site: false,
        symbol: "hirsel_rt_v2_io_stdin",
        destination: true,
        result: Some((16, 8)),
        parameters: &[Physical::Pointer],
    },
    AbiExport {
        caller_site: false,
        symbol: "hirsel_rt_v2_io_stderr",
        destination: true,
        result: Some((16, 8)),
        parameters: &[Physical::Pointer],
    },
    AbiExport {
        caller_site: false,
        symbol: "hirsel_rt_v2_stdin_read",
        destination: true,
        result: Some((16, 8)),
        parameters: &[Physical::Pointer, Physical::Pointer, Physical::I64],
    },
    AbiExport {
        caller_site: false,
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
        caller_site: false,
        symbol: "hirsel_rt_v2_err_write",
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
        caller_site: false,
        symbol: "hirsel_rt_v2_env_get",
        destination: true,
        result: Some((32, 8)),
        parameters: &[
            Physical::Pointer,
            Physical::Pointer,
            Physical::I64,
            Physical::I64,
        ],
    },
    AbiExport {
        caller_site: false,
        symbol: "hirsel_rt_v2_io_stdout",
        destination: true,
        result: Some((16, 8)),
        parameters: &[Physical::Pointer],
    },
    AbiExport {
        caller_site: true,
        symbol: "hirsel_rt_v2_out_print_at_site",
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
        caller_site: false,
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
    AbiExport {
        caller_site: false,
        symbol: "hirsel_rt_v2_out_write",
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
        caller_site: true,
        symbol: "hirsel_rt_v2_proc_spawn_at_site",
        destination: true,
        result: Some((16, 8)),
        parameters: &[Physical::Pointer, Physical::Pointer],
    },
    AbiExport {
        caller_site: true,
        symbol: "hirsel_rt_v2_proc_spawn_pipeline_at_site",
        destination: true,
        result: Some((16, 8)),
        parameters: &[Physical::Pointer, Physical::Pointer, Physical::I64],
    },
    AbiExport {
        caller_site: true,
        symbol: "hirsel_rt_v2_child_write_at_site",
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
        caller_site: true,
        symbol: "hirsel_rt_v2_child_close_stdin_at_site",
        destination: false,
        result: None,
        parameters: &[Physical::Pointer, Physical::Pointer],
    },
    AbiExport {
        caller_site: true,
        symbol: "hirsel_rt_v2_child_wait_at_site",
        destination: true,
        result: Some((12, 4)),
        parameters: &[Physical::Pointer, Physical::Pointer, Physical::Pointer],
    },
    AbiExport {
        caller_site: true,
        symbol: "hirsel_rt_v2_proc_wait_group_at_site",
        destination: true,
        result: Some((24, 8)),
        parameters: &[Physical::Pointer, Physical::Pointer, Physical::Pointer],
    },
];

/// The scalar core-library operations legalized into LIR instructions.
pub const SCALAR_INTRINSICS: &[&str] = &[
    "int.unchecked_add",
    "int.widen",
    "int.try_narrow",
    "int.narrow",
    "int.truncate",
    "int.wrapping_add",
    "int.saturating_add",
    "int.checked_add",
    "int.wrapping_sub",
    "int.saturating_sub",
    "int.checked_sub",
    "int.wrapping_mul",
    "int.checked_mul",
    "int.leading_zeros",
    "int.trailing_zeros",
    "int.count_ones",
    "int.swap_bytes",
    "int.reverse_bits",
    "int.div_floor",
    "int.mod",
    "int.div_exact",
    "float.min",
    "float.max",
    "float.min_number",
    "float.max_number",
    "float.mul_add",
    "float.sqrt",
    "float.floor",
    "float.ceil",
    "float.trunc",
    "float.round",
    "float.abs",
    "float.to_i32",
    "float.from_int",
    "f32.bits",
    "f32.from_bits",
    "f64.bits",
    "f64.from_bits",
    "f64.from_f32",
    "f32.from_f64",
    "char.to_u32",
    "char.from_u32",
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
    "$intrinsic.TextFinish",
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
    "net.split",
    "net.join",
    "read_half.read",
    "read_half.read_into",
    "read_half.set_timeout",
    "read_half.close",
    "write_half.write",
    "write_half.write_all",
    "write_half.shutdown",
    "write_half.set_timeout",
    "write_half.close",
    "tls.connect",
    "tls.connect_as",
    "tls.connect_with",
    "tls.accept",
    "tls.peer",
    "tls.negotiated_protocol",
    "tls.close",
    "net.udp_bind",
    "udp.send_to",
    "udp.recv_from",
    "udp.close",
    "net.listen",
    "net.unix_connect",
    "net.unix_listen",
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
    "$intrinsic.MapInsertionOrder_advance",
    "$intrinsic.NetReadInto",
    "$intrinsic.ReadHalfReadInto",
    "cpu.map_scoped",
    "$intrinsic.CpuMapScoped",
    "$intrinsic.TaskScopeEnter",
    "$intrinsic.TaskScopeExit",
    "$intrinsic.TaskScopeEnterUntil",
    "$intrinsic.TaskScopeExitUntil",
    "$intrinsic.TaskSpawn",
    "$intrinsic.TaskFinish",
    "$intrinsic.TaskJoin",
    "$intrinsic.TaskSetHas",
    "$intrinsic.TaskSetTake",
    "$intrinsic.ChannelOpen",
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
    "$intrinsic.SharedThawTurn",
    "$intrinsic.TaskCanceled",
    "$intrinsic.MappedView",
    "$intrinsic.MappedCheck",
    "$intrinsic.IsolateBegin",
    "$intrinsic.IsolateEnd",
    "$intrinsic.IsolateCount",
    "$intrinsic.IsolateKind",
    "$intrinsic.IsolateRule",
    "$intrinsic.IsolateDefinition",
    "$intrinsic.IsolateLine",
    "$intrinsic.IsolateColumn",
    "$intrinsic.IsolateSpawn",
    "$intrinsic.RunnerSend",
    "$intrinsic.RunnerWait",
    "$intrinsic.RunnerTake",
    "$intrinsic.RunnerEntry",
    "$intrinsic.RunnerArgument",
    "$intrinsic.RunnerIo",
    "$intrinsic.RunnerTasks",
    "$intrinsic.RunnerReply",
    "$intrinsic.RunnerGrants",
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
    "arg.len",
    "arg.path",
    "arg.text",
    "array.len",
    "atomic.create",
    "atomic.load",
    "atomic.store",
    "bytes.len",
    "bytes.from_list",
    "bytes.append",
    "bytes.append_from_history",
    "bytes.json_number_end",
    "bytes.integer",
    "bytes.decimal_word",
    "bytes.find_any",
    "bytes.base64_encode",
    "bytes.base64_decode",
    "bytes.sha1",
    "bytes.sha224",
    "bytes.sha256",
    "bytes.sha384",
    "bytes.sha512",
    "bytes.sha512_256",
    "bytes.replace_range",
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
    "dir.read_only",
    "file.append",
    "file.commit",
    "file.map",
    "mapped.with",
    "file.watch",
    "watcher.next",
    "file.open_append",
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
    "file.set_permissions",
    "dir.remove_all",
    "file.remove",
    "file.seek",
    "file.size",
    "file.walk",
    "file.write",
    "file.write_all",
    "file.write_bytes",
    "file.sync",
    "file.put_new",
    "f32.bits",
    "f32.from_bits",
    "f32.from_f64",
    "f64.bits",
    "f64.from_bits",
    "f64.from_f32",
    "char.from_u32",
    "char.to_u32",
    "float.abs",
    "float.ceil",
    "float.floor",
    "float.from_int",
    "float.max",
    "float.max_number",
    "float.min",
    "float.min_number",
    "float.mul_add",
    "float.round",
    "float.sqrt",
    "float.to_i32",
    "float.trunc",
    "forget",
    "int.checked_add",
    "int.checked_mul",
    "int.checked_sub",
    "int.count_ones",
    "int.div_exact",
    "int.div_floor",
    "int.leading_zeros",
    "int.mod",
    "int.narrow",
    "int.reverse_bits",
    "int.saturating_add",
    "int.saturating_sub",
    "int.swap_bytes",
    "int.trailing_zeros",
    "int.truncate",
    "int.try_narrow",
    "int.unchecked_add",
    "int.widen",
    "int.wrapping_add",
    "int.wrapping_mul",
    "int.wrapping_sub",
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
    "io.signals",
    "signal.wait",
    "stdin.read",
    "err.print",
    "err.write",
    "env.get",
    "env.narrow",
    "io.stdout",
    "io.tasks",
    "io.process",
    "proc.run",
    "proc.pipeline",
    "random.bytes",
    "io.telemetry",
    "capture.begin",
    "capture.context",
    "capture.event",
    "capture.finish",
    "capture.log",
    "capture.measure",
    "clock.sleep_until",
    "instant.checked_add",
    "test.process",
    "proc.spawn",
    "proc.narrow",
    "child.write",
    "child.close_stdin",
    "child.read_stdout",
    "child.read_stderr",
    "child.wait",
    "child.kill",
    "proc.spawn_pipeline",
    "group.write",
    "group.close_stdin",
    "group.read_stdout",
    "proc.wait_group",
    "group.kill",
    "iter.from",
    "iter.next",
    "list.len",
    "list.push",
    "list.truncate",
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
    "map.upsert_with",
    "map.update_with",
    "map.update_value_with",
    "mutex.create",
    "mutex.with",
    "out.print",
    "out.write",
    "path.of",
    "region.with",
    "sender.dup",
    "set.contains",
    "set.empty",
    "set.len",
    "set.insert",
    "set.remove",
    "shared.freeze",
    "shared.thaw",
    "shared.with",
    "simd.add",
    "simd.splat",
    "slots.empty",
    "slots.get",
    "slots.insert",
    "slots.remove",
    "sorted_map.empty",
    "sorted_map.len",
    "sorted_map.insert",
    "sorted_map.get",
    "sorted_map.remove",
    "string.byte_len",
    "string.from_bytes",
    "string.positions",
    "string.scalars",
    "string.slice",
    "task.add",
    "task.async",
    "task.await",
    "task.await_any",
    "task.await_at",
    "task.cancel",
    "task.cancel_all",
    "task.check_canceled",
    "task.detach",
    "task.detach_all",
    "task.isolate",
    "task.scope",
    "task.scope_until",
    "task.set",
    "test.clock",
    "test.check",
    "test.check_case",
    "test.crash",
    "test.dir",
    "test.dir_event",
    "test.expect_snapshot",
    "test.fail",
    "test.log",
    "test.net",
    "test.seed",
    "test.tasks",
    // CT-3: compile-time only; no run-time code holds a TypeShape or Body.
    "type_shape.info",
    "codegen.body",
    // RUN-1..14: runner grants and calls (`hirsel-rt/src/runners.rs`).
    "io.runner",
    "runner.grants",
    "runner.narrow",
    "runner.only",
    "runner.entry_path",
    "runner.compute",
    "runner.call",
    "runner.spawn",
    "test.runner",
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
