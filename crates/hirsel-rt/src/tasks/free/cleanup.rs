//! Detached object images belong to the task, not to a finalizer's Rust frame.
//! A stop or replay cut abandons native frames without unwinding them. The
//! task context still drops, returning every outstanding image allocation.
use crate::channels::Ring;

#[derive(Default)]
pub(crate) struct Images {
    rings: Vec<Ring>,
    #[cfg(test)]
    pub(crate) reclaimed: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
}

impl Images {
    pub(super) fn keep(&mut self, image: Ring) -> usize {
        let index = self.rings.len();
        self.rings.push(image);
        index
    }

    pub(super) fn at(&self, image: usize, slot: usize) -> *mut u8 {
        let image = &self.rings[image];
        assert!(slot < image.len());
        image.at(slot)
    }

    pub(super) fn release(&mut self, index: usize) {
        // Cleanup is nested in call order. A finalizer may close another
        // object and grow this Vec; the images' backing pointers stay stable.
        assert_eq!(index + 1, self.rings.len());
        self.rings.pop();
    }

    #[cfg(test)]
    pub(super) fn len(&self) -> usize {
        self.rings.len()
    }
}

#[cfg(test)]
impl Drop for Images {
    fn drop(&mut self) {
        self.rings.clear();
        if let Some(reclaimed) = &self.reclaimed {
            reclaimed.store(true, std::sync::atomic::Ordering::Release);
        }
    }
}
