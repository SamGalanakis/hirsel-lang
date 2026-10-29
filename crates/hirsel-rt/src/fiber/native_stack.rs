//! A thread's creation attributes are stable. Cache their exact pthread
//! bounds rather than rediscovering the main mapping on every admission.
//! The rlimit remains live: a host limit change invalidates the cached size.
use crate::environmental;

thread_local! {
    static BOUNDS: std::cell::Cell<Option<(libc::rlim_t, u64)>> = const { std::cell::Cell::new(None) };
}

pub(super) fn bytes() -> u64 {
    let mut limit = std::mem::MaybeUninit::<libc::rlimit>::uninit();
    // SAFETY: limit is writable and getrlimit initializes it on success.
    let limit = unsafe {
        if libc::getrlimit(libc::RLIMIT_STACK, limit.as_mut_ptr()) != 0 {
            return pthread_bytes();
        }
        limit.assume_init().rlim_cur
    };
    BOUNDS.with(|cache| {
        if let Some((seen, bytes)) = cache.get()
            && seen == limit
        {
            return bytes;
        }
        let bytes = pthread_bytes();
        cache.set(Some((limit, bytes)));
        bytes
    })
}

fn pthread_bytes() -> u64 {
    let mut attributes = std::mem::MaybeUninit::<libc::pthread_attr_t>::uninit();
    let (mut stack, mut guard) = (0usize, 0usize);
    // SAFETY: attributes is initialized by pthread_getattr_np and destroyed once.
    unsafe {
        if libc::pthread_getattr_np(libc::pthread_self(), attributes.as_mut_ptr()) != 0 {
            environmental("the admission stack size could not be read");
        }
        let mut attributes = attributes.assume_init();
        let status = libc::pthread_attr_getstacksize(&attributes, &mut stack)
            | libc::pthread_attr_getguardsize(&attributes, &mut guard);
        libc::pthread_attr_destroy(&mut attributes);
        if status != 0 {
            environmental("the admission stack bounds could not be read");
        }
    }
    (stack as u64)
        .checked_add(guard as u64)
        .unwrap_or_else(|| environmental("the admission stack bounds overflowed"))
}

#[cfg(test)]
mod tests {
    #[test]
    fn cached_native_bounds_preserve_host_selected_worker_stacks_and_guards() {
        for size in [1 << 20, 3 << 20] {
            std::thread::Builder::new()
                .stack_size(size)
                .spawn(|| {
                    let expected = super::pthread_bytes();
                    assert_eq!(super::bytes(), expected);
                    assert_eq!(super::bytes(), expected);
                })
                .unwrap()
                .join()
                .unwrap();
        }
    }
}
