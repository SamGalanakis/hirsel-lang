/* Monomorphic internal library ABI v2. Declaration schemas remain registry-owned.
 * A symbol family's {F} template is resolved per call to f32/f64 (hirsel_lir::abi::resolve_symbol).
 * All lengths are u64. No Rust enum, Vec or host usize appears here.
 */
#ifndef HIRSEL_NATIVE_LIBRARY_H
#define HIRSEL_NATIVE_LIBRARY_H
#include "../../crates/hirsel-rt/include/hirsel_rt_v2.h"
#ifdef __cplusplus
extern "C" {
#endif

/* Hirsel ABI: ctx, hidden destination (where declared), result, arguments.
 * Expected Result errors return status zero. Taken inputs are consumed by the entry. */
typedef struct { uint32_t tag, padding; uint64_t value; } hirsel_rt_v2_handle_result;
typedef struct { uint32_t tag, error, code; } hirsel_rt_v2_unit_result;
/* Metadata Result: tag at 0, Metadata at 16, Option[i128] at 32; 64 bytes. */
typedef struct { _Alignas(16) uint64_t words[10]; } hirsel_rt_v2_metadata_result;
typedef uint8_t (*hirsel_rt_v2_test_entry)(hirsel_rt_v2_ctx*, void*, void*, const uint64_t*);
int32_t hirsel_rt_v2_launch_test(hirsel_rt_v2_test_entry, uint64_t, const hirsel_rt_v2_layout *const *, uint32_t, const hirsel_rt_v2_launch_config *);
uint8_t hirsel_rt_v2_io_close(hirsel_rt_v2_ctx*, uint64_t, uint64_t, uint64_t);
uint8_t hirsel_rt_v2_io_stdout(hirsel_rt_v2_ctx*, void*, void*, const uint64_t*);
uint8_t hirsel_rt_v2_out_print(hirsel_rt_v2_ctx*, void*, hirsel_rt_v2_unit_result*, const uint64_t*, hirsel_rt_v2_header*, uint64_t, uint64_t);
uint8_t hirsel_rt_v2_io_dir(hirsel_rt_v2_ctx*, void*, hirsel_rt_v2_handle_result*, const uint64_t*, const uint64_t*);
uint8_t hirsel_rt_v2_dir_narrow(hirsel_rt_v2_ctx*, void*, hirsel_rt_v2_handle_result*, const uint64_t*, const uint64_t*);
uint8_t hirsel_rt_v2_file_open(hirsel_rt_v2_ctx*, void*, hirsel_rt_v2_handle_result*, const uint64_t*, const uint64_t*);
uint8_t hirsel_rt_v2_file_create(hirsel_rt_v2_ctx*, void*, hirsel_rt_v2_handle_result*, const uint64_t*, const uint64_t*);
uint8_t hirsel_rt_v2_file_read(hirsel_rt_v2_ctx*, void*, hirsel_rt_v2_handle_result*, const uint64_t*, uint64_t*, hirsel_rt_v2_header**, uint64_t);
uint8_t hirsel_rt_v2_file_seek(hirsel_rt_v2_ctx*, void*, hirsel_rt_v2_unit_result*, const uint64_t*, uint64_t*, uint64_t);
uint8_t hirsel_rt_v2_file_write(hirsel_rt_v2_ctx*, void*, hirsel_rt_v2_handle_result*, const uint64_t*, uint64_t*, hirsel_rt_v2_header*, uint64_t, uint64_t);
uint8_t hirsel_rt_v2_file_append(hirsel_rt_v2_ctx*, void*, hirsel_rt_v2_unit_result*, const uint64_t*, uint64_t*, hirsel_rt_v2_header*, uint64_t, uint64_t);
uint8_t hirsel_rt_v2_file_create_atomic(hirsel_rt_v2_ctx*, void*, hirsel_rt_v2_handle_result*, const uint64_t*, const uint64_t*);
uint8_t hirsel_rt_v2_file_size(hirsel_rt_v2_ctx*, void*, hirsel_rt_v2_handle_result*, const uint64_t*, const uint64_t*);
uint8_t hirsel_rt_v2_file_read_dir(hirsel_rt_v2_ctx*, void*, hirsel_rt_v2_handle_result*, const uint64_t*, const uint64_t*);
uint8_t hirsel_rt_v2_file_metadata(hirsel_rt_v2_ctx*, void*, hirsel_rt_v2_metadata_result*, const uint64_t*, const uint64_t*);
uint8_t hirsel_rt_v2_file_mkdir(hirsel_rt_v2_ctx*, void*, hirsel_rt_v2_unit_result*, const uint64_t*, const uint64_t*);
uint8_t hirsel_rt_v2_file_rename(hirsel_rt_v2_ctx*, void*, hirsel_rt_v2_unit_result*, const uint64_t*, const uint64_t*, const uint64_t*);
uint8_t hirsel_rt_v2_dir_remove_all(hirsel_rt_v2_ctx*, void*, hirsel_rt_v2_unit_result*, const uint64_t*, const uint64_t*);
uint8_t hirsel_rt_v2_file_set_permissions(hirsel_rt_v2_ctx*, void*, hirsel_rt_v2_unit_result*, const uint64_t*, const uint64_t*, uint32_t);
uint8_t hirsel_rt_v2_file_remove_dir(hirsel_rt_v2_ctx*, void*, hirsel_rt_v2_unit_result*, const uint64_t*, const uint64_t*);
uint8_t hirsel_rt_v2_file_remove(hirsel_rt_v2_ctx*, void*, hirsel_rt_v2_unit_result*, const uint64_t*, const uint64_t*);
uint8_t hirsel_rt_v2_file_narrow(hirsel_rt_v2_ctx*, void*, hirsel_rt_v2_handle_result*, const uint64_t*, uint64_t*);
uint8_t hirsel_rt_v2_file_commit(hirsel_rt_v2_ctx*, void*, hirsel_rt_v2_unit_result*, const uint64_t*, uint64_t*);
uint8_t hirsel_rt_v2_file_write_all(hirsel_rt_v2_ctx*, void*, hirsel_rt_v2_unit_result*, const uint64_t*, uint64_t*, hirsel_rt_v2_header*, uint64_t, uint64_t);
uint8_t hirsel_rt_v2_file_write_bytes(hirsel_rt_v2_ctx*, void*, hirsel_rt_v2_unit_result*, const uint64_t*, const uint64_t*, hirsel_rt_v2_header*, uint64_t, uint64_t);
uint8_t hirsel_rt_v2_path_of(hirsel_rt_v2_ctx*, void*, uint64_t*, hirsel_rt_v2_header*, uint64_t, uint64_t);
uint8_t hirsel_rt_v2_arg_path(hirsel_rt_v2_ctx*, void*, hirsel_rt_v2_handle_result*, const uint64_t*, uint64_t);

uint8_t hirsel_rt_v2_io_monotonic_clock(hirsel_rt_v2_ctx*, void*, void*, const uint64_t*);
uint8_t hirsel_rt_v2_io_wall_clock(hirsel_rt_v2_ctx*, void*, void*, const uint64_t*);
uint8_t hirsel_rt_v2_clock_monotonic(hirsel_rt_v2_ctx*, void*, void*, const uint64_t*);
uint8_t hirsel_rt_v2_clock_wall(hirsel_rt_v2_ctx*, void*, void*, const uint64_t*);
uint8_t hirsel_rt_v2_instant_ticks(hirsel_rt_v2_ctx*, uint64_t*, const uint64_t*);
/* Option[Instant]: tag 1 carries the advanced ticks at 8. */
typedef struct { uint32_t tag, padding; uint64_t ticks; } hirsel_rt_v2_instant_option;
uint8_t hirsel_rt_v2_instant_checked_add(hirsel_rt_v2_ctx*, void*, hirsel_rt_v2_instant_option*, const uint64_t*, const uint64_t*, uint64_t);
/* Result[(), WaitError]: error 0 is Clock(ClockError), 1 is Canceled(CancelCause); the payload ordinal follows. */
uint8_t hirsel_rt_v2_clock_sleep_until(hirsel_rt_v2_ctx*, void*, void*, const uint64_t*, const uint64_t*);
uint8_t hirsel_rt_v2_io_random(hirsel_rt_v2_ctx*, void*, void*, const uint64_t*);
uint8_t hirsel_rt_v2_test_check(hirsel_rt_v2_ctx*, const uint64_t*, uint8_t);
uint8_t hirsel_rt_v2_test_check_case(hirsel_rt_v2_ctx*, const uint64_t*, uint8_t, uint64_t);
uint8_t hirsel_rt_v2_test_fail(hirsel_rt_v2_ctx*, const uint64_t*);
uint8_t hirsel_rt_v2_test_seed(hirsel_rt_v2_ctx*, uint64_t*, const uint64_t*);
uint8_t hirsel_rt_v2_test_log(hirsel_rt_v2_ctx*, const uint64_t*, hirsel_rt_v2_header*, uint64_t, uint64_t);
uint8_t hirsel_rt_v2_test_expect_snapshot(hirsel_rt_v2_ctx*, const uint64_t*, hirsel_rt_v2_header*, uint64_t, uint64_t, hirsel_rt_v2_header*, uint64_t, uint64_t);
uint8_t hirsel_rt_v2_test_dir(hirsel_rt_v2_ctx*, void*, uint64_t*, const uint64_t*, hirsel_rt_v2_header*);
uint8_t hirsel_rt_v2_test_net(hirsel_rt_v2_ctx*, void*, uint64_t*, const uint64_t*, hirsel_rt_v2_header*);
uint8_t hirsel_rt_v2_test_process(hirsel_rt_v2_ctx*, void*, uint64_t*, const uint64_t*, hirsel_rt_v2_header*, hirsel_rt_v2_header*);


/* Unicode 16 borrowed cursors retain the slice origin; position is slice-relative.
 * Open borrows without a count. */
typedef struct { hirsel_rt_v2_header *owner; uint64_t start, length; } hirsel_rt_v2_slice;
/* Text results use one static Sequence[u8] descriptor; from_bytes transfers the
 * same owner on both Result arms. SEM-3 status 1 leaves output unwritten and
 * generated caller glue records the rule at the call site before cleanup. */
extern const hirsel_rt_v2_layout hirsel_rt_v2_bytes_descriptor;
typedef struct { uint32_t tag, padding; hirsel_rt_v2_slice value; } hirsel_rt_v2_string_result;
typedef struct { uint32_t tag, padding, option, option_padding; hirsel_rt_v2_slice value; } hirsel_rt_v2_env_result;
uint8_t hirsel_rt_v2_io_stdin(hirsel_rt_v2_ctx*, void*, hirsel_rt_v2_handle_result*, const uint64_t*);
uint8_t hirsel_rt_v2_io_stderr(hirsel_rt_v2_ctx*, void*, hirsel_rt_v2_handle_result*, const uint64_t*);
uint8_t hirsel_rt_v2_stdin_read(hirsel_rt_v2_ctx*, void*, hirsel_rt_v2_handle_result*, const uint64_t*, hirsel_rt_v2_header**, uint64_t);
uint8_t hirsel_rt_v2_err_print(hirsel_rt_v2_ctx*, void*, hirsel_rt_v2_unit_result*, const uint64_t*, hirsel_rt_v2_header*, uint64_t, uint64_t);
uint8_t hirsel_rt_v2_env_get(hirsel_rt_v2_ctx*, void*, hirsel_rt_v2_env_result*, const uint64_t*, hirsel_rt_v2_header*, uint64_t, uint64_t);
uint8_t hirsel_rt_v2_arg_len(hirsel_rt_v2_ctx*, uint64_t*, const uint64_t*);
uint8_t hirsel_rt_v2_arg_text(hirsel_rt_v2_ctx*, void*, hirsel_rt_v2_string_result*, const uint64_t*, uint64_t);
uint8_t hirsel_rt_v2_string_from_bytes(hirsel_rt_v2_ctx*, void*, hirsel_rt_v2_string_result*, hirsel_rt_v2_header*, uint64_t, uint64_t);
uint8_t hirsel_rt_v2_string_slice(hirsel_rt_v2_ctx*, void*, hirsel_rt_v2_slice*, hirsel_rt_v2_header*, uint64_t, uint64_t, const uint64_t*, const uint64_t*);
typedef struct { uint32_t tag, padding; hirsel_rt_v2_slice value; } hirsel_rt_v2_bytes_result;
typedef struct { hirsel_rt_v2_header *environment; const void *code; const void *drop; } hirsel_rt_v2_walk_iterator;
typedef struct { uint32_t tag, padding; hirsel_rt_v2_walk_iterator value; } hirsel_rt_v2_walk_result;
typedef struct { uint32_t tag, padding; hirsel_rt_v2_handle_result value; } hirsel_rt_v2_walk_option;
extern const hirsel_rt_v2_layout hirsel_rt_v2_file_walk_descriptor;
uint8_t hirsel_rt_v2_file_read_bytes(hirsel_rt_v2_ctx*, void*, hirsel_rt_v2_bytes_result*, const uint64_t*, const uint64_t*);
uint8_t hirsel_rt_v2_random_bytes(hirsel_rt_v2_ctx*, void*, hirsel_rt_v2_bytes_result*, const uint64_t*, uint64_t);
uint8_t hirsel_rt_v2_file_walk(hirsel_rt_v2_ctx*, void*, hirsel_rt_v2_walk_result*, const uint64_t*);
uint8_t hirsel_rt_v2_file_walk_step(hirsel_rt_v2_ctx*, hirsel_rt_v2_header*, void*, hirsel_rt_v2_walk_option*);
typedef struct { hirsel_rt_v2_slice view; uint64_t position; } hirsel_rt_v2_view_cursor;
typedef struct { uint32_t tag, value; } hirsel_rt_v2_scalar_option;
typedef struct { uint32_t tag, padding; uint64_t value; } hirsel_rt_v2_position_option;
uint8_t hirsel_rt_v2_unicode_scalars_open(hirsel_rt_v2_ctx*, void*, hirsel_rt_v2_view_cursor*, hirsel_rt_v2_header*, uint64_t, uint64_t);
uint8_t hirsel_rt_v2_unicode_scalars_next(hirsel_rt_v2_ctx*, void*, hirsel_rt_v2_scalar_option*, hirsel_rt_v2_view_cursor*);
uint8_t hirsel_rt_v2_scalar_positions_open(hirsel_rt_v2_ctx*, void*, hirsel_rt_v2_view_cursor*, hirsel_rt_v2_header*, uint64_t, uint64_t);
uint8_t hirsel_rt_v2_scalar_positions_next(hirsel_rt_v2_ctx*, void*, hirsel_rt_v2_position_option*, hirsel_rt_v2_view_cursor*);

/* The scalar operations are LIR instructions (LIR schema revision 6), emitted natively.
 * Numeric algorithms now execute in ordinary std source. */
/* Private structural Path order; IDs name the bound run-context table. */
int32_t hirsel_rt_v2_path_compare(hirsel_rt_v2_ctx *, uint64_t, uint64_t);
/* Read-only byte bridges, std-wave1. */
uint8_t hirsel_rt_v2_string_as_bytes(hirsel_rt_v2_ctx*, void*, hirsel_rt_v2_slice*, hirsel_rt_v2_header*, uint64_t, uint64_t);
uint8_t hirsel_rt_v2_bytes_slice(hirsel_rt_v2_ctx*, void*, hirsel_rt_v2_slice*, hirsel_rt_v2_header*, uint64_t, uint64_t, uint64_t, uint64_t);
uint8_t hirsel_rt_v2_string_position(hirsel_rt_v2_ctx*, void*, void*, hirsel_rt_v2_header*, uint64_t, uint64_t, uint64_t);
uint8_t hirsel_rt_v2_string_byte_offset(hirsel_rt_v2_ctx*, uint64_t*, hirsel_rt_v2_header*, uint64_t, uint64_t, const uint64_t*);
uint8_t hirsel_rt_v2_path_text(hirsel_rt_v2_ctx*, void*, hirsel_rt_v2_slice*, const uint64_t*);
/* Network sockets (hirsel-rt src/net_abi.rs). NetError is a tag
   and a 12-byte payload at 4; SocketAddr is IpAddr (tag, octets at 4) then the port at 20. */
typedef struct { uint32_t tag, word, extra; } hirsel_rt_v2_net_error;
typedef struct { uint32_t tag, padding; uint64_t value, rest; } hirsel_rt_v2_net_word_result;
typedef struct { uint32_t tag; hirsel_rt_v2_net_error error; } hirsel_rt_v2_net_unit_result;
typedef struct { uint32_t ip_tag; uint8_t octets[16]; uint16_t port, padding; } hirsel_rt_v2_socket_addr;
typedef struct { uint32_t tag; hirsel_rt_v2_socket_addr value; } hirsel_rt_v2_net_addr_result;
typedef struct { uint32_t tag, padding; uint64_t stream; hirsel_rt_v2_socket_addr address; } hirsel_rt_v2_net_accepted_result;
typedef struct { uint32_t tag, padding; uint64_t count; hirsel_rt_v2_socket_addr peer; uint8_t truncated, tail[7]; } hirsel_rt_v2_udp_received_result;
typedef struct { uint32_t tag, padding; uint64_t nanos; } hirsel_rt_v2_net_timeout;
uint8_t hirsel_rt_v2_net_narrow(hirsel_rt_v2_ctx*, void*, hirsel_rt_v2_net_word_result*, const uint64_t*, hirsel_rt_v2_header*);
uint8_t hirsel_rt_v2_net_resolve(hirsel_rt_v2_ctx*, void*, hirsel_rt_v2_net_word_result*, const uint64_t*, hirsel_rt_v2_header*, uint64_t, uint64_t, uint16_t);
uint8_t hirsel_rt_v2_net_connect(hirsel_rt_v2_ctx*, void*, hirsel_rt_v2_net_word_result*, const uint64_t*, hirsel_rt_v2_header*, uint64_t, uint64_t, uint16_t, const hirsel_rt_v2_net_timeout*);
uint8_t hirsel_rt_v2_net_connect_addr(hirsel_rt_v2_ctx*, void*, hirsel_rt_v2_net_word_result*, const uint64_t*, const hirsel_rt_v2_socket_addr*, const hirsel_rt_v2_net_timeout*);
uint8_t hirsel_rt_v2_net_read(hirsel_rt_v2_ctx*, void*, hirsel_rt_v2_net_word_result*, const uint64_t*, uint64_t*, hirsel_rt_v2_header**, uint64_t);
uint8_t hirsel_rt_v2_net_read_into(hirsel_rt_v2_ctx*, void*, hirsel_rt_v2_net_word_result*, const uint64_t*, uint64_t*, uint8_t*, uint64_t);
uint8_t hirsel_rt_v2_net_write(hirsel_rt_v2_ctx*, void*, hirsel_rt_v2_net_word_result*, const uint64_t*, uint64_t*, hirsel_rt_v2_header*, uint64_t, uint64_t);
uint8_t hirsel_rt_v2_net_write_all(hirsel_rt_v2_ctx*, void*, hirsel_rt_v2_net_unit_result*, const uint64_t*, uint64_t*, hirsel_rt_v2_header*, uint64_t, uint64_t);
uint8_t hirsel_rt_v2_net_shutdown(hirsel_rt_v2_ctx*, void*, hirsel_rt_v2_net_unit_result*, const uint64_t*, uint64_t*, const uint32_t*);
uint8_t hirsel_rt_v2_net_set_read_timeout(hirsel_rt_v2_ctx*, void*, hirsel_rt_v2_net_unit_result*, const uint64_t*, uint64_t*, const hirsel_rt_v2_net_timeout*);
uint8_t hirsel_rt_v2_net_set_write_timeout(hirsel_rt_v2_ctx*, void*, hirsel_rt_v2_net_unit_result*, const uint64_t*, uint64_t*, const hirsel_rt_v2_net_timeout*);
uint8_t hirsel_rt_v2_net_listen(hirsel_rt_v2_ctx*, void*, hirsel_rt_v2_net_word_result*, const uint64_t*, const hirsel_rt_v2_socket_addr*);
uint8_t hirsel_rt_v2_net_local_addr(hirsel_rt_v2_ctx*, void*, hirsel_rt_v2_net_addr_result*, const uint64_t*, const uint64_t*);
uint8_t hirsel_rt_v2_net_accept(hirsel_rt_v2_ctx*, void*, hirsel_rt_v2_net_accepted_result*, const uint64_t*, const uint64_t*);
uint8_t hirsel_rt_v2_net_peer_addr(hirsel_rt_v2_ctx*, void*, hirsel_rt_v2_net_addr_result*, const uint64_t*, const uint64_t*);
uint8_t hirsel_rt_v2_net_close(hirsel_rt_v2_ctx*, const uint64_t*, uint64_t*);
uint8_t hirsel_rt_v2_net_unix_connect(hirsel_rt_v2_ctx*, void*, hirsel_rt_v2_net_word_result*, const uint64_t*, const uint64_t*);
uint8_t hirsel_rt_v2_net_unix_listen(hirsel_rt_v2_ctx*, void*, hirsel_rt_v2_net_word_result*, const uint64_t*, const uint64_t*);
uint8_t hirsel_rt_v2_net_udp_bind(hirsel_rt_v2_ctx*, void*, hirsel_rt_v2_net_word_result*, const uint64_t*, const hirsel_rt_v2_socket_addr*);
uint8_t hirsel_rt_v2_udp_send_to(hirsel_rt_v2_ctx*, void*, hirsel_rt_v2_net_word_result*, const uint64_t*, uint64_t*, hirsel_rt_v2_header*, uint64_t, uint64_t, const hirsel_rt_v2_socket_addr*);
uint8_t hirsel_rt_v2_udp_recv_from(hirsel_rt_v2_ctx*, void*, hirsel_rt_v2_udp_received_result*, const uint64_t*, uint64_t*, hirsel_rt_v2_header**, uint64_t);
uint8_t hirsel_rt_v2_udp_close(hirsel_rt_v2_ctx*, const uint64_t*, uint64_t*);
/* TLS roots use the ordinary WebPki/Pem sum; only the active Pem slice is read. */
typedef struct { uint32_t tag, padding; hirsel_rt_v2_slice pem; } hirsel_rt_v2_tls_roots;
uint8_t hirsel_rt_v2_tls_connect(hirsel_rt_v2_ctx*, void*, hirsel_rt_v2_net_word_result*, const uint64_t*, uint64_t*, hirsel_rt_v2_header*, uint64_t, uint64_t, const hirsel_rt_v2_tls_roots*);
/* A TLS identity is the ordinary { chain: Bytes, key: Bytes } record of two slices. */
typedef struct { hirsel_rt_v2_slice chain, key; } hirsel_rt_v2_tls_identity;
uint8_t hirsel_rt_v2_tls_accept(hirsel_rt_v2_ctx*, void*, hirsel_rt_v2_net_word_result*, const uint64_t*, uint64_t*, const hirsel_rt_v2_tls_identity*);
uint8_t hirsel_rt_v2_tls_close(hirsel_rt_v2_ctx*, void*, hirsel_rt_v2_net_unit_result*, const uint64_t*, uint64_t*);
/* Structured tasks (hirsel-rt src/tasks.rs). Scope and Task are run-local handle words; a
   task's code is a compiled `once fn() -> ()` closure; results cross as whole Option images. */
uint8_t hirsel_rt_v2_task_scope_enter(hirsel_rt_v2_ctx*, void*, uint64_t*, const uint64_t*);
uint8_t hirsel_rt_v2_task_scope_exit(hirsel_rt_v2_ctx*, const uint64_t*, uint8_t);
/* task.scope_until (CONC-3): the deadline is the clock handle, then the Instant's ticks; the
   exit writes 1 when the deadline fired. */
uint8_t hirsel_rt_v2_task_scope_enter_until(hirsel_rt_v2_ctx*, void*, uint64_t*, const uint64_t*, const uint64_t*);
uint8_t hirsel_rt_v2_task_scope_exit_until(hirsel_rt_v2_ctx*, uint8_t*, const uint64_t*, uint8_t);
uint8_t hirsel_rt_v2_task_spawn(hirsel_rt_v2_ctx*, uint64_t, uint64_t, void*, uint64_t*, const uint64_t*, uint8_t (*)(hirsel_rt_v2_ctx*, hirsel_rt_v2_header*, uint64_t, uint64_t), hirsel_rt_v2_header*);
uint8_t hirsel_rt_v2_task_finish(hirsel_rt_v2_ctx*, uint8_t*, const uint8_t*, uint64_t);
uint8_t hirsel_rt_v2_task_join(hirsel_rt_v2_ctx*, const uint64_t*, uint64_t*, uint8_t*, uint64_t);
uint8_t hirsel_rt_v2_task_cancel(hirsel_rt_v2_ctx*, const uint64_t*, const uint64_t*);
uint8_t hirsel_rt_v2_task_detach(hirsel_rt_v2_ctx*, const uint64_t*, uint64_t*);
/* Task sets (CONC-2): a set word is a run-local endpoint word; await_any writes an Option[u64]
   image (tag, then the index); await_at's compiled body checks and takes a member, then joins it. */
uint8_t hirsel_rt_v2_task_set(hirsel_rt_v2_ctx*, void*, uint64_t*, const uint64_t*);
uint8_t hirsel_rt_v2_task_add(hirsel_rt_v2_ctx*, const uint64_t*, uint64_t*, uint64_t*);
uint8_t hirsel_rt_v2_task_await_any(hirsel_rt_v2_ctx*, void*, uint64_t*, const uint64_t*, const uint64_t*);
uint8_t hirsel_rt_v2_task_set_has(hirsel_rt_v2_ctx*, uint8_t*, const uint64_t*, const uint64_t*, uint64_t);
uint8_t hirsel_rt_v2_task_set_take(hirsel_rt_v2_ctx*, void*, uint64_t*, const uint64_t*, uint64_t*, uint64_t);
uint8_t hirsel_rt_v2_task_cancel_all(hirsel_rt_v2_ctx*, const uint64_t*, uint64_t*);
uint8_t hirsel_rt_v2_task_detach_all(hirsel_rt_v2_ctx*, const uint64_t*, uint64_t*);
/* Channels (CONC-6). Sender and Receiver are run-local handle words; a message crosses as the
   block of a one-element list, moved out of and into Option[List[T]] slots. The status is 0
   sent or received, 1 closed, 2 + cause canceled. */
uint8_t hirsel_rt_v2_channel_open(hirsel_rt_v2_ctx*, void*, uint64_t*, const uint64_t*);
uint8_t hirsel_rt_v2_sender_dup(hirsel_rt_v2_ctx*, void*, uint64_t*, const uint64_t*, const uint64_t*);
uint8_t hirsel_rt_v2_channel_send(hirsel_rt_v2_ctx*, uint32_t*, uint64_t*, hirsel_rt_v2_header**);
uint8_t hirsel_rt_v2_channel_receive(hirsel_rt_v2_ctx*, uint32_t*, uint64_t*, hirsel_rt_v2_header**);
/* Mutexes and atomics (CONC-4, CONC-5, FAIL-4). Mutex and Atomic are run-local handle words.
   A mutex payload crosses as the block of a one-element list, moved out of and into
   Option[List[T]] carriers; an atomic's scalar payload crosses as the Some image of an Option
   of `size` bytes. MutexTake's status is 0 acquired, 1 dead. */
uint8_t hirsel_rt_v2_mutex_create(hirsel_rt_v2_ctx*, void*, uint64_t*, hirsel_rt_v2_header**);
uint8_t hirsel_rt_v2_mutex_take(hirsel_rt_v2_ctx*, uint32_t*, const uint64_t*, hirsel_rt_v2_header**);
uint8_t hirsel_rt_v2_mutex_put(hirsel_rt_v2_ctx*, const uint64_t*, hirsel_rt_v2_header**);
uint8_t hirsel_rt_v2_mutex_kill(hirsel_rt_v2_ctx*, const uint64_t*);
uint8_t hirsel_rt_v2_mutex_cause(hirsel_rt_v2_ctx*, const uint64_t*);
uint8_t hirsel_rt_v2_atomic_create(hirsel_rt_v2_ctx*, void*, uint64_t*, const uint8_t*, uint64_t);
uint8_t hirsel_rt_v2_atomic_load(hirsel_rt_v2_ctx*, const uint64_t*, uint8_t*, uint64_t);
uint8_t hirsel_rt_v2_atomic_store(hirsel_rt_v2_ctx*, const uint64_t*, const uint8_t*, uint64_t);
/* task.check_canceled (CONC-3): 0, or 1 + the CancelCause ordinal. */
uint8_t hirsel_rt_v2_task_canceled(hirsel_rt_v2_ctx*, uint32_t*, const uint64_t*);
/* task.isolate (FAIL-2): the caught chain, record by record; the link shim registers the
   program's failure table of site positions and rule IDs. */
typedef struct { uint64_t code; const uint8_t *definition; uint64_t definition_length, line, column; } hirsel_rt_v2_failure_site;
typedef struct { const uint8_t *name; uint64_t length; } hirsel_rt_v2_failure_rule;
void hirsel_rt_v2_failure_table(const hirsel_rt_v2_failure_site*, uint64_t, const hirsel_rt_v2_failure_rule*, uint64_t);
uint8_t hirsel_rt_v2_isolate_begin(hirsel_rt_v2_ctx*, uint64_t*, const uint64_t*);
uint8_t hirsel_rt_v2_isolate_count(hirsel_rt_v2_ctx*, uint64_t*, const uint64_t*);
uint8_t hirsel_rt_v2_isolate_kind(hirsel_rt_v2_ctx*, uint32_t*, const uint64_t*, uint64_t);
uint8_t hirsel_rt_v2_isolate_rule(hirsel_rt_v2_ctx*, void*, hirsel_rt_v2_slice*, const uint64_t*, uint64_t);
uint8_t hirsel_rt_v2_isolate_definition(hirsel_rt_v2_ctx*, void*, hirsel_rt_v2_slice*, const uint64_t*, uint64_t);
uint8_t hirsel_rt_v2_isolate_line(hirsel_rt_v2_ctx*, uint64_t*, const uint64_t*, uint64_t);
uint8_t hirsel_rt_v2_isolate_column(hirsel_rt_v2_ctx*, uint64_t*, const uint64_t*, uint64_t);
uint8_t hirsel_rt_v2_isolate_spawn(hirsel_rt_v2_ctx*, void*, hirsel_rt_v2_header**, const uint64_t*, uint64_t);
#ifdef __cplusplus
}
#endif

#endif

uint8_t hirsel_rt_v2_io_tasks(hirsel_rt_v2_ctx*, void*, void*, const uint64_t*);
uint8_t hirsel_rt_v2_io_net(hirsel_rt_v2_ctx*, void*, void*, const uint64_t*);
uint8_t hirsel_rt_v2_io_process(hirsel_rt_v2_ctx*, void*, void*, const uint64_t*);
/* Result[ProcessOutput, ProcessError]: ExitStatus or ProcessError at 8, stdout at 16, stderr at 40. */
typedef struct { uint32_t tag, padding, kind, code; hirsel_rt_v2_slice stdout_bytes, stderr_bytes; } hirsel_rt_v2_process_result;
/* The trailing operand is Option[Deadline]: its tag, then the clock at 8 and the Instant at 16. */
uint8_t hirsel_rt_v2_proc_run(hirsel_rt_v2_ctx*, void*, hirsel_rt_v2_process_result*, const uint64_t*, const void*, hirsel_rt_v2_header*, uint64_t, uint64_t, uint64_t, const void*);
/* Result[Child or u64, ProcessError]: the value, or ProcessError (tag, payload) at 8. */
typedef struct { uint32_t tag, padding; uint64_t value; } hirsel_rt_v2_child_word_result;
/* Result[() or ExitStatus, ProcessError]: the ExitStatus or ProcessError at 4. */
typedef struct { uint32_t tag, kind, code; } hirsel_rt_v2_child_pair_result;
uint8_t hirsel_rt_v2_proc_spawn(hirsel_rt_v2_ctx*, void*, hirsel_rt_v2_child_word_result*, const uint64_t*, const void*);
uint8_t hirsel_rt_v2_child_write(hirsel_rt_v2_ctx*, void*, hirsel_rt_v2_child_pair_result*, const uint64_t*, uint64_t*, hirsel_rt_v2_header*, uint64_t, uint64_t);
uint8_t hirsel_rt_v2_child_close_stdin(hirsel_rt_v2_ctx*, const uint64_t*, uint64_t*);
uint8_t hirsel_rt_v2_child_read_stdout(hirsel_rt_v2_ctx*, void*, hirsel_rt_v2_child_word_result*, const uint64_t*, uint64_t*, hirsel_rt_v2_header**, uint64_t);
uint8_t hirsel_rt_v2_child_read_stderr(hirsel_rt_v2_ctx*, void*, hirsel_rt_v2_child_word_result*, const uint64_t*, uint64_t*, hirsel_rt_v2_header**, uint64_t);
/* The Child word is transferred and zeroed; the trailing operand is Option[Deadline]. */
uint8_t hirsel_rt_v2_child_wait(hirsel_rt_v2_ctx*, void*, hirsel_rt_v2_child_pair_result*, const uint64_t*, uint64_t*, const void*);
uint8_t hirsel_rt_v2_child_kill(hirsel_rt_v2_ctx*, const uint64_t*, uint64_t*, const uint32_t*);
