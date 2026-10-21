//! Production scalar words and striped fallback, including mixed SC orders.
use hirsel_free_loom::atomic::{Image, Stripes};
use loom::sync::Arc;

#[test]
fn native_and_striped_cells_share_one_sequentially_consistent_order() {
    for (left, right) in [(8, 8), (8, 16), (16, 16)] {
        loom::model(move || {
            let stripes = Arc::new(Stripes::default());
            let a = Arc::new(Image::new(&vec![0; left], || stripes.clone()));
            let b = Arc::new(Image::new(&vec![0; right], || stripes.clone()));
            let first = loom::thread::spawn({
                let (a, b) = (a.clone(), b.clone());
                move || {
                    a.access().store(&vec![1; left]);
                    let mut read = vec![0; right];
                    b.access().load(&mut read);
                    read.iter().all(|&b| b == 0)
                }
            });
            b.access().store(&vec![1; right]);
            let mut read = vec![0; left];
            a.access().load(&mut read);
            let second_zero = read.iter().all(|&b| b == 0);
            assert!(
                !(first.join().unwrap() && second_zero),
                "SC forbids both loads preceding the stores"
            );
        });
    }
}

#[test]
fn wide_loads_cannot_tear_across_another_threads_store() {
    for size in [16, 32] {
        loom::model(move || {
            let image = Arc::new(Image::new(&vec![0; size], || Arc::new(Stripes::default())));
            let writer = loom::thread::spawn({
                let image = image.clone();
                move || image.access().store(&vec![0x55; size])
            });
            let mut read = vec![0; size];
            image.access().load(&mut read);
            assert!(read.iter().all(|&b| b == 0) || read.iter().all(|&b| b == 0x55));
            writer.join().unwrap();
            image.access().load(&mut read);
            assert_eq!(read, vec![0x55; size]);
        });
    }
}
