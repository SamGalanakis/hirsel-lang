//! Count allocation/release churn separated by ordinary worker collection points.
hirsel_guard::install!();

use hirsel_rt::{Audit, Config, DOMAIN_HEAP, LAYOUT_MAGIC, LAYOUT_VERSION, Layout};
use std::{hint::black_box, ptr};

fn main() {
    let rounds: usize = std::env::args()
        .nth(1)
        .unwrap_or("10".into())
        .parse()
        .unwrap();
    let leaf = Layout {
        magic: LAYOUT_MAGIC,
        version: LAYOUT_VERSION,
        size: 0,
        align: 8,
        finalizer: None,
        steps: ptr::null(),
        step_count: 0,
        field_count: 0,
        flags: 0,
        logical: 0,
        logical_count: ptr::null(),
        scan: ptr::null(),
        scan_words: u64::MAX,
    };
    let layouts = [&leaf as *const Layout];
    let config = Config {
        layouts: layouts.as_ptr(),
        layout_count: 1,
        ..Config::new()
    };
    unsafe {
        let mut ctx = ptr::null_mut();
        assert_eq!(
            hirsel_rt::context::hirsel_rt_v2_ctx_create(&config, &mut ctx),
            0
        );
        for _ in 0..rounds {
            let blocks: Vec<_> = (0..12000)
                .map(|_| hirsel_rt::alloc::hirsel_rt_v2_alloc(ctx, 2, 1024, 8, DOMAIN_HEAP, 0))
                .collect();
            for block in blocks {
                black_box(block.cast::<u8>().add(size_of::<hirsel_rt::Header>())).write(7);
                assert_eq!(
                    hirsel_rt::release::hirsel_rt_v2_release(ctx, block, 2, ptr::null(), 0, 0),
                    0
                );
            }
            hirsel_rt::context::hirsel_rt_v2_worker_collect(ctx);
        }
        let mut audit = Audit::default();
        hirsel_rt::context::hirsel_rt_v2_task_exit(ctx, &mut audit);
        hirsel_rt::context::hirsel_rt_v2_ctx_destroy(ctx);
    }
    println!("{rounds}");
}
