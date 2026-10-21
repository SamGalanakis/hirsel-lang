//! Scalar SC images for the runtime's atomic cells. Native 64-bit words carry
//! images up to eight bytes. Wider images use a striped lock, with SC fences
//! before and after access, ported from portable-atomic's ScFenceGuard and
//! lock fallback (src/imp/fallback/mod.rs:80-119,354-384 at
//! a253dda1c21fe8d5c47e6d2818b3200197b8726c).
//!
//! Hirsel needs only load/store, not RMW. The fallback omits optimistic reads
//! and uses Mutex in place of the reference's stamped spin lock; locked reads
//! avoid torn byte copies and have the same SC fence envelope. Stripes belong
//! to one run, are allocated only for a wide image, and use the reference's
//! prime stripe count. Acquiring a stripe happens before opening an op section.

use crate::sync::{Arc, AtomicU64, Cell, Mutex, MutexGuard, sc_fence, sc_load64, sc_store64};

const STRIPES: usize = 67;

#[repr(align(128))]
struct Stripe(Mutex<()>);

pub struct Stripes([Stripe; STRIPES]);

impl Default for Stripes {
    fn default() -> Self {
        Self(std::array::from_fn(|_| Stripe(Mutex::new(()))))
    }
}

enum Value {
    Word(AtomicU64),
    Wide {
        bytes: Cell<Box<[u8]>>,
        stripes: Arc<Stripes>,
    },
}

pub struct Image {
    size: usize,
    value: Value,
}

impl Image {
    /// The lazy stripe factory is never called for a native word.
    pub fn new(image: &[u8], stripes: impl FnOnce() -> Arc<Stripes>) -> Self {
        assert!(!image.is_empty(), "a scalar atomic image is nonempty");
        Self {
            size: image.len(),
            value: if image.len() <= 8 {
                let mut bytes = [0; 8];
                bytes[..image.len()].copy_from_slice(image);
                Value::Word(AtomicU64::new(u64::from_le_bytes(bytes)))
            } else {
                Value::Wide {
                    bytes: Cell::new(image.into()),
                    stripes: stripes(),
                }
            },
        }
    }

    pub fn size(&self) -> usize {
        self.size
    }

    /// Hold this access through action and recording. The wide stripe is
    /// acquired before the section and released only after its publication.
    pub fn access(&self) -> Access<'_> {
        let stripe = match &self.value {
            Value::Word(_) => None,
            Value::Wide { stripes, .. } => {
                let address = self as *const Self as usize;
                Some(stripes.0[address % STRIPES].0.lock().unwrap())
            }
        };
        Access {
            image: self,
            _stripe: stripe,
            _owner: std::marker::PhantomData,
        }
    }
}

pub struct Access<'a> {
    image: &'a Image,
    _stripe: Option<MutexGuard<'a, ()>>,
    // A held stripe authorizes just this access, not concurrent re-entry
    // through shared references to the same guard.
    _owner: std::marker::PhantomData<std::cell::Cell<()>>,
}

// Fences surround the byte access, inside the section, even though the
// stripe was acquired earlier. Thus their SC order also includes native words.
struct Sc;
impl Sc {
    fn new() -> Self {
        sc_fence();
        Self
    }
}
impl Drop for Sc {
    fn drop(&mut self) {
        sc_fence();
    }
}

impl Access<'_> {
    pub fn load(&self, out: &mut [u8]) {
        assert_eq!(
            out.len(),
            self.image.size,
            "an atomic image has a fixed width"
        );
        match &self.image.value {
            Value::Word(word) => {
                out.copy_from_slice(&sc_load64(word).to_le_bytes()[..out.len()]);
            }
            Value::Wide { bytes, .. } => {
                let _sc = Sc::new();
                // SAFETY: this access holds the image's stripe, excluding
                // every other read or write of its bytes.
                unsafe { bytes.with(|bytes| out.copy_from_slice(bytes)) };
            }
        }
    }

    pub fn store(&self, value: &[u8]) {
        assert_eq!(
            value.len(),
            self.image.size,
            "an atomic image has a fixed width"
        );
        match &self.image.value {
            Value::Word(word) => {
                let mut bytes = [0; 8];
                bytes[..value.len()].copy_from_slice(value);
                sc_store64(word, u64::from_le_bytes(bytes));
            }
            Value::Wide { bytes, .. } => {
                let _sc = Sc::new();
                // SAFETY: this access holds the same stripe as every accessor.
                unsafe { bytes.with_mut(|bytes| bytes.copy_from_slice(value)) };
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn images_preserve_every_byte_and_allocate_stripes_only_for_wide_values() {
        crate::sync::model(|| {
            for size in [1, 2, 4, 8, 16, 32] {
                let mut striped = false;
                let image = Image::new(&vec![0; size], || {
                    striped = true;
                    Arc::new(Stripes::default())
                });
                assert_eq!(striped, size > 8);
                let value: Vec<_> = (0..size).map(|i| (i * 7) as u8).collect();
                let mut read = vec![0; size];
                image.access().store(&value);
                image.access().load(&mut read);
                assert_eq!(read, value);
            }
        });
    }
}
