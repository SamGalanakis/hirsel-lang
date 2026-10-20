# B1 multicore benchmark session

Status: **dependency-owned**. Instruction ratios are the verdict; time ratios apply only within this session.

Seed 20261019; 10 interleaved rounds; CPUs [31, 30, 29, 28, 27, 26, 25, 24]. Process-lifetime perf counts include launch and teardown. p99 requires operation latency telemetry; it is never inferred from ten process timings.

## Baselines

| Provider | Workload | Width | Mode | ns/op | Instructions/op | Peak RSS bytes | Throttled µs |
|---|---|---:|---|---:|---:|---:|---:|
| tasks.cooperative@1 | allocation | 1 | durable | 3987 | 4163 | 4.981e+06 | 0 |
| tasks.cooperative@1 | allocation | 1 | off | 667.8 | 2749 | 4.719e+06 | 0 |
| tasks.cooperative@1 | allocation | 1 | record | 970.8 | 4056 | 4.85e+06 | 0 |
| tasks.cooperative@1 | atomic | 1 | durable | 3351 | 2675 | 4.456e+06 | 0 |
| tasks.cooperative@1 | atomic | 1 | off | 537.8 | 1317 | 4.194e+06 | 0 |
| tasks.cooperative@1 | atomic | 1 | record | 982.3 | 2625 | 4.456e+06 | 0 |
| tasks.cooperative@1 | cancel_depth | 1 | durable | 4127 | 2716 | 4.456e+06 | 0 |
| tasks.cooperative@1 | cancel_depth | 1 | off | 511.5 | 1362 | 4.194e+06 | 0 |
| tasks.cooperative@1 | cancel_depth | 1 | record | 918.5 | 2660 | 4.588e+06 | 0 |
| tasks.cooperative@1 | channel | 1 | durable | 5957 | 5636 | 4.588e+06 | 0 |
| tasks.cooperative@1 | channel | 1 | off | 1128 | 2895 | 4.456e+06 | 0 |
| tasks.cooperative@1 | channel | 1 | record | 1924 | 5504 | 4.456e+06 | 0 |
| tasks.cooperative@1 | fork_join | 1 | durable | 1.793e+04 | 2.017e+04 | 4.456e+06 | 0 |
| tasks.cooperative@1 | fork_join | 1 | off | 3520 | 9139 | 4.194e+06 | 0 |
| tasks.cooperative@1 | fork_join | 1 | record | 7226 | 1.958e+04 | 4.456e+06 | 0 |
| tasks.cooperative@1 | independent_files | 1 | durable | 3.735e+05 | 1.257e+05 | 4.456e+06 | 0 |
| tasks.cooperative@1 | independent_files | 1 | off | 2635 | 6423 | 4.456e+06 | 0 |
| tasks.cooperative@1 | independent_files | 1 | record | 5419 | 1.555e+04 | 4.456e+06 | 0 |
| tasks.cooperative@1 | late_set | 1 | durable | 8590 | 1.226e+04 | 4.456e+06 | 0 |
| tasks.cooperative@1 | late_set | 1 | off | 2421 | 8572 | 4.456e+06 | 0 |
| tasks.cooperative@1 | late_set | 1 | record | 3022 | 1.199e+04 | 4.456e+06 | 0 |
| tasks.cooperative@1 | mutex | 1 | durable | 1795 | 2854 | 4.456e+06 | 0 |
| tasks.cooperative@1 | mutex | 1 | off | 447.3 | 1459 | 4.456e+06 | 0 |
| tasks.cooperative@1 | mutex | 1 | record | 873.8 | 2774 | 4.456e+06 | 0 |
| tasks.cooperative@1 | namespace_mix | 1 | durable | 7.86e+05 | 2.377e+05 | 4.981e+06 | 0 |
| tasks.cooperative@1 | namespace_mix | 1 | off | 3.29e+04 | 6.842e+04 | 4.719e+06 | 0 |
| tasks.cooperative@1 | namespace_mix | 1 | record | 3.847e+04 | 8.062e+04 | 4.588e+06 | 0 |
| tasks.cooperative@1 | roots | 1 | durable | 1655 | 2226 | 4.588e+06 | 0 |
| tasks.cooperative@1 | roots | 1 | off | 395.2 | 852.1 | 4.456e+06 | 0 |
| tasks.cooperative@1 | roots | 1 | record | 866.5 | 2155 | 4.456e+06 | 0 |
| tasks.cooperative@1 | yield | 1 | durable | 3710 | 2711 | 4.456e+06 | 0 |
| tasks.cooperative@1 | yield | 1 | off | 499.1 | 1302 | 4.194e+06 | 0 |
| tasks.cooperative@1 | yield | 1 | record | 866.9 | 2612 | 4.588e+06 | 0 |
| tokio | allocation | 1 | off | 593 | 2382 | 3.146e+06 | 0 |
| tokio | allocation | 2 | off | 863.5 | 2471 | 2.884e+06 | 0 |
| tokio | allocation | 4 | off | 778.8 | 3103 | 2.884e+06 | 0 |
| tokio | allocation | 8 | off | 880.8 | 3149 | 2.753e+06 | 0 |
| tokio | atomic | 1 | off | 409.1 | 558.2 | 3.146e+06 | 0 |
| tokio | atomic | 2 | off | 567.9 | 585.7 | 2.884e+06 | 0 |
| tokio | atomic | 4 | off | 683.4 | 677.4 | 2.884e+06 | 0 |
| tokio | atomic | 8 | off | 865.5 | 826 | 2.753e+06 | 0 |
| tokio | cancel_depth | 1 | off | 284.9 | 593.9 | 3.015e+06 | 0 |
| tokio | cancel_depth | 2 | off | 661.4 | 643.9 | 3.015e+06 | 0 |
| tokio | cancel_depth | 4 | off | 596 | 714.8 | 2.884e+06 | 0 |
| tokio | cancel_depth | 8 | off | 799.7 | 897.5 | 2.621e+06 | 0 |
| tokio | channel | 1 | off | 5128 | 8484 | 2.884e+06 | 0 |
| tokio | channel | 2 | off | 8173 | 1.211e+04 | 2.884e+06 | 0 |
| tokio | channel | 4 | off | 8196 | 1.227e+04 | 2.621e+06 | 0 |
| tokio | channel | 8 | off | 8155 | 1.195e+04 | 2.621e+06 | 0 |
| tokio | extern | 1 | off | 411.8 | 548.7 | 2.884e+06 | 0 |
| tokio | extern | 2 | off | 539.1 | 578.9 | 2.884e+06 | 0 |
| tokio | extern | 4 | off | 598.9 | 659.7 | 2.884e+06 | 0 |
| tokio | extern | 8 | off | 864.3 | 817.2 | 2.753e+06 | 0 |
| tokio | fork_join | 1 | off | 1.948e+04 | 3.405e+04 | 3.146e+06 | 0 |
| tokio | fork_join | 2 | off | 3.144e+04 | 4.954e+04 | 2.884e+06 | 0 |
| tokio | fork_join | 4 | off | 3.601e+04 | 5.259e+04 | 2.753e+06 | 0 |
| tokio | fork_join | 8 | off | 3.44e+04 | 5.254e+04 | 2.621e+06 | 0 |
| tokio | independent_files | 1 | off | 1692 | 3538 | 3.146e+06 | 0 |
| tokio | independent_files | 2 | off | 1472 | 3602 | 3.015e+06 | 0 |
| tokio | independent_files | 4 | off | 1476 | 3690 | 2.753e+06 | 0 |
| tokio | independent_files | 8 | off | 1481 | 3910 | 2.621e+06 | 0 |
| tokio | late_set | 1 | off | 1.68e+04 | 2.958e+04 | 2.884e+06 | 0 |
| tokio | late_set | 2 | off | 2.343e+04 | 3.546e+04 | 2.884e+06 | 0 |
| tokio | late_set | 4 | off | 2.384e+04 | 3.816e+04 | 2.884e+06 | 0 |
| tokio | late_set | 8 | off | 2.131e+04 | 3.827e+04 | 2.49e+06 | 0 |
| tokio | mutex | 1 | off | 351.5 | 768.7 | 3.146e+06 | 0 |
| tokio | mutex | 2 | off | 637.8 | 909.8 | 2.884e+06 | 0 |
| tokio | mutex | 4 | off | 715.5 | 1500 | 2.884e+06 | 0 |
| tokio | mutex | 8 | off | 688.9 | 1667 | 2.884e+06 | 0 |
| tokio | namespace_mix | 1 | off | 1.533e+04 | 3.572e+04 | 2.884e+06 | 0 |
| tokio | namespace_mix | 2 | off | 1.188e+04 | 3.959e+04 | 2.884e+06 | 0 |
| tokio | namespace_mix | 4 | off | 1.136e+04 | 4.616e+04 | 2.884e+06 | 0 |
| tokio | namespace_mix | 8 | off | 1.177e+04 | 5.089e+04 | 2.753e+06 | 0 |
| tokio | roots | 1 | off | 402.7 | 547.4 | 3.146e+06 | 0 |
| tokio | roots | 2 | off | 708.4 | 600.8 | 2.884e+06 | 0 |
| tokio | roots | 4 | off | 655.3 | 683.7 | 2.884e+06 | 0 |
| tokio | roots | 8 | off | 811.7 | 835 | 2.753e+06 | 0 |
| tokio | yield | 1 | off | 404.6 | 1132 | 3.015e+06 | 0 |
| tokio | yield | 2 | off | 610.1 | 1305 | 2.884e+06 | 0 |
| tokio | yield | 4 | off | 742.9 | 1399 | 2.884e+06 | 0 |
| tokio | yield | 8 | off | 838.9 | 1684 | 2.621e+06 | 0 |

## Width-1 cooperative/tokio gaps

| Workload | Throughput coop/tokio | Instructions coop/tokio | Time coop/tokio CI95 |
|---|---:|---:|---|
| yield | 0.8106 | 1.15 | [1.1114242568645647, 1.565286907471689] |
| cancel_depth | 0.557 | 2.294 | [1.3582485348115059, 2.645192092851361] |
| atomic | 0.7607 | 2.36 | [0.984286662859723, 1.654244684839517] |
| roots | 1.019 | 1.556 | [0.7632924746118035, 1.3789087747937863] |
| mutex | 0.7859 | 1.898 | [0.9725293005671077, 1.6636027449161135] |
| allocation | 0.8879 | 1.154 | [0.9610652851674578, 1.3880925111549023] |
| independent_files | 0.6423 | 1.815 | [1.390544659531841, 1.7117059362149978] |
| namespace_mix | 0.4659 | 1.915 | [1.9822550836548154, 2.221185076072996] |
| fork_join | 5.534 | 0.2684 | [0.16207273613448844, 0.24829865758586483] |
| late_set | 6.938 | 0.2898 | [0.08759273101255279, 0.1804812558354086] |
| channel | 4.548 | 0.3412 | [0.19343575010580116, 0.3097829825469965] |
| extern | — | — | — |

## Cooperative width-1 recording overhead

| Workload | Record/off throughput | Instructions record/off | Durable/off throughput | Tape bytes (record) |
|---|---:|---:|---:|---:|
| yield | 0.5758 | 2.006 | 0.1345 | 1387 |
| cancel_depth | 0.5569 | 1.952 | 0.1239 | 1387 |
| atomic | 0.5474 | 1.993 | 0.1605 | 1389 |
| roots | 0.456 | 2.53 | 0.2388 | 1387 |
| mutex | 0.5119 | 1.901 | 0.2492 | 1387 |
| allocation | 0.6879 | 1.475 | 0.1675 | 1387 |
| independent_files | 0.4862 | 2.421 | 0.007054 | 2.519e+05 |
| namespace_mix | 0.8551 | 1.178 | 0.04185 | 9.592e+05 |
| fork_join | 0.4871 | 2.142 | 0.1963 | 1391 |
| late_set | 0.8012 | 1.398 | 0.2819 | 1387 |
| channel | 0.586 | 1.901 | 0.1893 | 1391 |
| extern | — | — | — | — |

## Free-mode floor gates

| Workload | Width | Free/tokio throughput | Instructions free/tokio | Record/off | p99 ratio | B/event | Durable/coop-1 |
|---|---:|---:|---:|---:|---:|---:|---:|
| yield | 1 | — | — | — | — | — | — |
| yield | 2 | — | — | — | — | — | — |
| yield | 4 | — | — | — | — | — | — |
| yield | 8 | — | — | — | — | — | — |
| cancel_depth | 1 | — | — | — | — | — | — |
| cancel_depth | 2 | — | — | — | — | — | — |
| cancel_depth | 4 | — | — | — | — | — | — |
| cancel_depth | 8 | — | — | — | — | — | — |
| atomic | 1 | — | — | — | — | — | — |
| atomic | 2 | — | — | — | — | — | — |
| atomic | 4 | — | — | — | — | — | — |
| atomic | 8 | — | — | — | — | — | — |
| roots | 1 | — | — | — | — | — | — |
| roots | 2 | — | — | — | — | — | — |
| roots | 4 | — | — | — | — | — | — |
| roots | 8 | — | — | — | — | — | — |
| mutex | 1 | — | — | — | — | — | — |
| mutex | 2 | — | — | — | — | — | — |
| mutex | 4 | — | — | — | — | — | — |
| mutex | 8 | — | — | — | — | — | — |
| allocation | 1 | — | — | — | — | — | — |
| allocation | 2 | — | — | — | — | — | — |
| allocation | 4 | — | — | — | — | — | — |
| allocation | 8 | — | — | — | — | — | — |
| independent_files | 1 | — | — | — | — | — | — |
| independent_files | 2 | — | — | — | — | — | — |
| independent_files | 4 | — | — | — | — | — | — |
| independent_files | 8 | — | — | — | — | — | — |
| namespace_mix | 1 | — | — | — | — | — | — |
| namespace_mix | 2 | — | — | — | — | — | — |
| namespace_mix | 4 | — | — | — | — | — | — |
| namespace_mix | 8 | — | — | — | — | — | — |
| fork_join | 1 | — | — | — | — | — | — |
| fork_join | 2 | — | — | — | — | — | — |
| fork_join | 4 | — | — | — | — | — | — |
| fork_join | 8 | — | — | — | — | — | — |
| late_set | 1 | — | — | — | — | — | — |
| late_set | 2 | — | — | — | — | — | — |
| late_set | 4 | — | — | — | — | — | — |
| late_set | 8 | — | — | — | — | — | — |
| channel | 1 | — | — | — | — | — | — |
| channel | 2 | — | — | — | — | — | — |
| channel | 4 | — | — | — | — | — | — |
| channel | 8 | — | — | — | — | — | — |
| extern | 1 | — | — | — | — | — | — |
| extern | 2 | — | — | — | — | — | — |
| extern | 4 | — | — | — | — | — | — |
| extern | 8 | — | — | — | — | — | — |

## Per-row headroom gaps

| Row | Workload | Width | Metric | Candidate | Reference | Gap | Status |
|---|---|---:|---|---:|---:|---:|---|
| switch_queue | yield | 1 | switches_per_op | — | — | — | dependency-owned |
| switch_queue | yield | 1 | queue_ops_per_op | — | — | — | dependency-owned |
| migration | yield | 1 | migrations_per_s | — | — | — | dependency-owned |
| migration | yield | 1 | refresh_ns | — | — | — | dependency-owned |
| switch_queue | yield | 2 | switches_per_op | — | — | — | dependency-owned |
| switch_queue | yield | 2 | queue_ops_per_op | — | — | — | dependency-owned |
| migration | yield | 2 | migrations_per_s | — | — | — | dependency-owned |
| migration | yield | 2 | refresh_ns | — | — | — | dependency-owned |
| switch_queue | yield | 4 | switches_per_op | — | — | — | dependency-owned |
| switch_queue | yield | 4 | queue_ops_per_op | — | — | — | dependency-owned |
| migration | yield | 4 | migrations_per_s | — | — | — | dependency-owned |
| migration | yield | 4 | refresh_ns | — | — | — | dependency-owned |
| switch_queue | yield | 8 | switches_per_op | — | — | — | dependency-owned |
| switch_queue | yield | 8 | queue_ops_per_op | — | — | — | dependency-owned |
| migration | yield | 8 | migrations_per_s | — | — | — | dependency-owned |
| migration | yield | 8 | refresh_ns | — | — | — | dependency-owned |
| cancellation_ranks | cancel_depth | 1 | ancestor_depth | — | — | — | dependency-owned |
| cancellation_ranks | cancel_depth | 1 | rank_contention | — | — | — | dependency-owned |
| cancellation_ranks | cancel_depth | 2 | ancestor_depth | — | — | — | dependency-owned |
| cancellation_ranks | cancel_depth | 2 | rank_contention | — | — | — | dependency-owned |
| cancellation_ranks | cancel_depth | 4 | ancestor_depth | — | — | — | dependency-owned |
| cancellation_ranks | cancel_depth | 4 | rank_contention | — | — | — | dependency-owned |
| cancellation_ranks | cancel_depth | 8 | ancestor_depth | — | — | — | dependency-owned |
| cancellation_ranks | cancel_depth | 8 | rank_contention | — | — | — | dependency-owned |
| handles | atomic | 1 | lookup_ns_per_op | — | — | — | dependency-owned |
| handles | atomic | 2 | lookup_ns_per_op | — | — | — | dependency-owned |
| handles | atomic | 4 | lookup_ns_per_op | — | — | — | dependency-owned |
| handles | atomic | 8 | lookup_ns_per_op | — | — | — | dependency-owned |
| roots_remote | roots | 1 | atomics_per_op | — | — | — | dependency-owned |
| roots_remote | roots | 1 | remote_frees_per_s | — | — | — | dependency-owned |
| roots_remote | roots | 2 | atomics_per_op | — | — | — | dependency-owned |
| roots_remote | roots | 2 | remote_frees_per_s | — | — | — | dependency-owned |
| roots_remote | roots | 4 | atomics_per_op | — | — | — | dependency-owned |
| roots_remote | roots | 4 | remote_frees_per_s | — | — | — | dependency-owned |
| roots_remote | roots | 8 | atomics_per_op | — | — | — | dependency-owned |
| roots_remote | roots | 8 | remote_frees_per_s | — | — | — | dependency-owned |
| object_contention | mutex | 1 | object_ops_per_s | — | — | — | dependency-owned |
| object_contention | mutex | 2 | object_ops_per_s | — | — | — | dependency-owned |
| object_contention | mutex | 4 | object_ops_per_s | — | — | — | dependency-owned |
| object_contention | mutex | 8 | object_ops_per_s | — | — | — | dependency-owned |
| meter_permits | allocation | 1 | meter_ops_per_s | — | — | — | dependency-owned |
| meter_permits | allocation | 1 | allocations_per_s | — | — | — | dependency-owned |
| meter_permits | allocation | 1 | speculation_pauses | — | — | — | dependency-owned |
| meter_permits | allocation | 2 | meter_ops_per_s | — | — | — | dependency-owned |
| meter_permits | allocation | 2 | allocations_per_s | — | — | — | dependency-owned |
| meter_permits | allocation | 2 | speculation_pauses | — | — | — | dependency-owned |
| meter_permits | allocation | 4 | meter_ops_per_s | — | — | — | dependency-owned |
| meter_permits | allocation | 4 | allocations_per_s | — | — | — | dependency-owned |
| meter_permits | allocation | 4 | speculation_pauses | — | — | — | dependency-owned |
| meter_permits | allocation | 8 | meter_ops_per_s | — | — | — | dependency-owned |
| meter_permits | allocation | 8 | allocations_per_s | — | — | — | dependency-owned |
| meter_permits | allocation | 8 | speculation_pauses | — | — | — | dependency-owned |
| io_ordering | independent_files | 1 | syscall_serialisation | — | — | — | dependency-owned |
| io_ordering | independent_files | 1 | namespace_wait_ns | — | — | — | dependency-owned |
| io_ordering | independent_files | 1 | key_epoch_wait_ns | — | — | — | dependency-owned |
| io_ordering | independent_files | 1 | independent_file_overlap | — | — | — | dependency-owned |
| recording | independent_files | 1 | ns_per_op | — | — | — | dependency-owned |
| recording | independent_files | 1 | compressed_bytes_per_contended_event | — | — | — | dependency-owned |
| recording | independent_files | 1 | flush_cpu_ns | — | — | — | dependency-owned |
| recording | independent_files | 1 | cut_pause_ns | — | — | — | dependency-owned |
| recording | independent_files | 1 | flush_ns_per_prepared_request | — | — | — | dependency-owned |
| io_ordering | independent_files | 2 | syscall_serialisation | — | — | — | dependency-owned |
| io_ordering | independent_files | 2 | namespace_wait_ns | — | — | — | dependency-owned |
| io_ordering | independent_files | 2 | key_epoch_wait_ns | — | — | — | dependency-owned |
| io_ordering | independent_files | 2 | independent_file_overlap | — | — | — | dependency-owned |
| recording | independent_files | 2 | ns_per_op | — | — | — | dependency-owned |
| recording | independent_files | 2 | compressed_bytes_per_contended_event | — | — | — | dependency-owned |
| recording | independent_files | 2 | flush_cpu_ns | — | — | — | dependency-owned |
| recording | independent_files | 2 | cut_pause_ns | — | — | — | dependency-owned |
| recording | independent_files | 2 | flush_ns_per_prepared_request | — | — | — | dependency-owned |
| io_ordering | independent_files | 4 | syscall_serialisation | — | — | — | dependency-owned |
| io_ordering | independent_files | 4 | namespace_wait_ns | — | — | — | dependency-owned |
| io_ordering | independent_files | 4 | key_epoch_wait_ns | — | — | — | dependency-owned |
| io_ordering | independent_files | 4 | independent_file_overlap | — | — | — | dependency-owned |
| recording | independent_files | 4 | ns_per_op | — | — | — | dependency-owned |
| recording | independent_files | 4 | compressed_bytes_per_contended_event | — | — | — | dependency-owned |
| recording | independent_files | 4 | flush_cpu_ns | — | — | — | dependency-owned |
| recording | independent_files | 4 | cut_pause_ns | — | — | — | dependency-owned |
| recording | independent_files | 4 | flush_ns_per_prepared_request | — | — | — | dependency-owned |
| io_ordering | independent_files | 8 | syscall_serialisation | — | — | — | dependency-owned |
| io_ordering | independent_files | 8 | namespace_wait_ns | — | — | — | dependency-owned |
| io_ordering | independent_files | 8 | key_epoch_wait_ns | — | — | — | dependency-owned |
| io_ordering | independent_files | 8 | independent_file_overlap | — | — | — | dependency-owned |
| recording | independent_files | 8 | ns_per_op | — | — | — | dependency-owned |
| recording | independent_files | 8 | compressed_bytes_per_contended_event | — | — | — | dependency-owned |
| recording | independent_files | 8 | flush_cpu_ns | — | — | — | dependency-owned |
| recording | independent_files | 8 | cut_pause_ns | — | — | — | dependency-owned |
| recording | independent_files | 8 | flush_ns_per_prepared_request | — | — | — | dependency-owned |
| io_ordering | namespace_mix | 1 | syscall_serialisation | — | — | — | dependency-owned |
| io_ordering | namespace_mix | 1 | namespace_wait_ns | — | — | — | dependency-owned |
| io_ordering | namespace_mix | 1 | key_epoch_wait_ns | — | — | — | dependency-owned |
| io_ordering | namespace_mix | 1 | independent_file_overlap | — | — | — | dependency-owned |
| io_ordering | namespace_mix | 2 | syscall_serialisation | — | — | — | dependency-owned |
| io_ordering | namespace_mix | 2 | namespace_wait_ns | — | — | — | dependency-owned |
| io_ordering | namespace_mix | 2 | key_epoch_wait_ns | — | — | — | dependency-owned |
| io_ordering | namespace_mix | 2 | independent_file_overlap | — | — | — | dependency-owned |
| io_ordering | namespace_mix | 4 | syscall_serialisation | — | — | — | dependency-owned |
| io_ordering | namespace_mix | 4 | namespace_wait_ns | — | — | — | dependency-owned |
| io_ordering | namespace_mix | 4 | key_epoch_wait_ns | — | — | — | dependency-owned |
| io_ordering | namespace_mix | 4 | independent_file_overlap | — | — | — | dependency-owned |
| io_ordering | namespace_mix | 8 | syscall_serialisation | — | — | — | dependency-owned |
| io_ordering | namespace_mix | 8 | namespace_wait_ns | — | — | — | dependency-owned |
| io_ordering | namespace_mix | 8 | key_epoch_wait_ns | — | — | — | dependency-owned |
| io_ordering | namespace_mix | 8 | independent_file_overlap | — | — | — | dependency-owned |
| fiber_creation | fork_join | 1 | spawn_ns | — | — | — | dependency-owned |
| fiber_creation | fork_join | 1 | rss_bytes_per_task | — | — | — | dependency-owned |
| fiber_creation | fork_join | 2 | spawn_ns | — | — | — | dependency-owned |
| fiber_creation | fork_join | 2 | rss_bytes_per_task | — | — | — | dependency-owned |
| fiber_creation | fork_join | 4 | spawn_ns | — | — | — | dependency-owned |
| fiber_creation | fork_join | 4 | rss_bytes_per_task | — | — | — | dependency-owned |
| fiber_creation | fork_join | 8 | spawn_ns | — | — | — | dependency-owned |
| fiber_creation | fork_join | 8 | rss_bytes_per_task | — | — | — | dependency-owned |
| fiber_creation | late_set | 1 | spawn_ns | — | — | — | dependency-owned |
| fiber_creation | late_set | 1 | rss_bytes_per_task | — | — | — | dependency-owned |
| cancellation_ranks | late_set | 1 | ancestor_depth | — | — | — | dependency-owned |
| cancellation_ranks | late_set | 1 | rank_contention | — | — | — | dependency-owned |
| fiber_creation | late_set | 2 | spawn_ns | — | — | — | dependency-owned |
| fiber_creation | late_set | 2 | rss_bytes_per_task | — | — | — | dependency-owned |
| cancellation_ranks | late_set | 2 | ancestor_depth | — | — | — | dependency-owned |
| cancellation_ranks | late_set | 2 | rank_contention | — | — | — | dependency-owned |
| fiber_creation | late_set | 4 | spawn_ns | — | — | — | dependency-owned |
| fiber_creation | late_set | 4 | rss_bytes_per_task | — | — | — | dependency-owned |
| cancellation_ranks | late_set | 4 | ancestor_depth | — | — | — | dependency-owned |
| cancellation_ranks | late_set | 4 | rank_contention | — | — | — | dependency-owned |
| fiber_creation | late_set | 8 | spawn_ns | — | — | — | dependency-owned |
| fiber_creation | late_set | 8 | rss_bytes_per_task | — | — | — | dependency-owned |
| cancellation_ranks | late_set | 8 | ancestor_depth | — | — | — | dependency-owned |
| cancellation_ranks | late_set | 8 | rank_contention | — | — | — | dependency-owned |
| switch_queue | channel | 1 | switches_per_op | — | — | — | dependency-owned |
| switch_queue | channel | 1 | queue_ops_per_op | — | — | — | dependency-owned |
| object_contention | channel | 1 | object_ops_per_s | — | — | — | dependency-owned |
| recording | channel | 1 | ns_per_op | — | — | — | dependency-owned |
| recording | channel | 1 | compressed_bytes_per_contended_event | — | — | — | dependency-owned |
| recording | channel | 1 | flush_cpu_ns | — | — | — | dependency-owned |
| recording | channel | 1 | cut_pause_ns | — | — | — | dependency-owned |
| recording | channel | 1 | flush_ns_per_prepared_request | — | — | — | dependency-owned |
| switch_queue | channel | 2 | switches_per_op | — | — | — | dependency-owned |
| switch_queue | channel | 2 | queue_ops_per_op | — | — | — | dependency-owned |
| object_contention | channel | 2 | object_ops_per_s | — | — | — | dependency-owned |
| recording | channel | 2 | ns_per_op | — | — | — | dependency-owned |
| recording | channel | 2 | compressed_bytes_per_contended_event | — | — | — | dependency-owned |
| recording | channel | 2 | flush_cpu_ns | — | — | — | dependency-owned |
| recording | channel | 2 | cut_pause_ns | — | — | — | dependency-owned |
| recording | channel | 2 | flush_ns_per_prepared_request | — | — | — | dependency-owned |
| switch_queue | channel | 4 | switches_per_op | — | — | — | dependency-owned |
| switch_queue | channel | 4 | queue_ops_per_op | — | — | — | dependency-owned |
| object_contention | channel | 4 | object_ops_per_s | — | — | — | dependency-owned |
| recording | channel | 4 | ns_per_op | — | — | — | dependency-owned |
| recording | channel | 4 | compressed_bytes_per_contended_event | — | — | — | dependency-owned |
| recording | channel | 4 | flush_cpu_ns | — | — | — | dependency-owned |
| recording | channel | 4 | cut_pause_ns | — | — | — | dependency-owned |
| recording | channel | 4 | flush_ns_per_prepared_request | — | — | — | dependency-owned |
| switch_queue | channel | 8 | switches_per_op | — | — | — | dependency-owned |
| switch_queue | channel | 8 | queue_ops_per_op | — | — | — | dependency-owned |
| object_contention | channel | 8 | object_ops_per_s | — | — | — | dependency-owned |
| recording | channel | 8 | ns_per_op | — | — | — | dependency-owned |
| recording | channel | 8 | compressed_bytes_per_contended_event | — | — | — | dependency-owned |
| recording | channel | 8 | flush_cpu_ns | — | — | — | dependency-owned |
| recording | channel | 8 | cut_pause_ns | — | — | — | dependency-owned |
| recording | channel | 8 | flush_ns_per_prepared_request | — | — | — | dependency-owned |
| extern | extern | 1 | extern_ns | — | — | — | dependency-owned |
| extern | extern | 1 | pool_queue_ns | — | — | — | dependency-owned |
| extern | extern | 1 | gate_wait_ns | — | — | — | dependency-owned |
| extern | extern | 2 | extern_ns | — | — | — | dependency-owned |
| extern | extern | 2 | pool_queue_ns | — | — | — | dependency-owned |
| extern | extern | 2 | gate_wait_ns | — | — | — | dependency-owned |
| extern | extern | 4 | extern_ns | — | — | — | dependency-owned |
| extern | extern | 4 | pool_queue_ns | — | — | — | dependency-owned |
| extern | extern | 4 | gate_wait_ns | — | — | — | dependency-owned |
| extern | extern | 8 | extern_ns | — | — | — | dependency-owned |
| extern | extern | 8 | pool_queue_ns | — | — | — | dependency-owned |
| extern | extern | 8 | gate_wait_ns | — | — | — | dependency-owned |

## Dependency-owned evidence

- extern: R8: trusted threads declarations and gate telemetry; external participant required.
- free: C1/W1/F1: tasks.free@1 placement/executor/recording.
- metrics: R1-R8/W1/F1/K1: internal counters; null until supplied.
- server: tasks-rust: axum baseline + C1/W1/F1 server width integration.
