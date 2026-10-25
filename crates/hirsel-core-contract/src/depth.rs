//! Versioned logical depth units. Physical stack and operational budgets are separate.
use hirsel_syntax::DefPath;

pub const SEMANTICS: &str = "hirsel.depth.invocations.v2";
pub const FOREGROUND_LIMIT: u16 = 1024;
pub const CLEANUP_LIMIT: u16 = 64;
pub const STRUCTURAL_LIMIT: u64 = 1088;

/// LANG-10 leaf operations, part one: every operation of these core scalar
/// modules. Each reads borrowed scalars and returns a value; none is adapter-backed,
/// takes a callback or runs language code (no finalizer, check or host call).
pub const LEAF_MODULES: &[&str] = &["int", "float", "f32", "f64", "char"];

/// LANG-10 leaf operations, part two: the check primitive, the size queries of
/// core collections, the list operations that only append or reserve, and the
/// byte-list conversion. None releases a value that can have a finalizer.
pub const LEAF_OPERATIONS: &[&str] = &[
    "failure.check",
    "list.len",
    "array.len",
    "bytes.len",
    "map.len",
    "set.len",
    "string.byte_len",
    "list.push",
    "list.reserve",
    "bytes.from_list",
];

/// Whether a direct call of `callee` adds an activation (LANG-10). Like operators,
/// a leaf operation adds none. Indirect calls always do: the callable invocation is
/// counted whatever it names. Every engine asks this and no other table.
pub fn adds_activation(callee: &DefPath) -> bool {
    let leaf = callee.package_is("core", "1")
        && match callee.0.modules.as_slice() {
            [module] => LEAF_MODULES.contains(&module.as_str()),
            _ => false,
        };
    !(leaf || LEAF_OPERATIONS.iter().any(|name| callee.is_core(name)))
}

/// Admission returns a linear retirement token, including which allowance it used.
/// A rejected admission creates no token and changes neither count.
#[repr(transparent)]
#[derive(Debug)]
pub struct Activation {
    cleanup: bool,
}

/// One task's foreground and implicit-cleanup allowances (LANG-10, FAIL-3).
#[repr(C)]
#[derive(Debug, Default)]
pub struct Activations {
    foreground: u16,
    cleanup: u16,
}

// Native fixed leaf operations access these two counters and the token's mode.
const _: () = {
    assert!(std::mem::offset_of!(Activations, foreground) == 0);
    assert!(std::mem::offset_of!(Activations, cleanup) == 2);
    assert!(std::mem::size_of::<Activation>() == 1);
};

impl Activations {
    #[inline(always)]
    pub fn admit(&mut self) -> Option<Activation> {
        self.admit_in(self.cleanup != 0)
    }

    #[inline(always)]
    pub fn admit_finalizer(&mut self) -> Option<Activation> {
        self.admit_in(true)
    }

    #[inline(always)]
    fn admit_in(&mut self, cleanup: bool) -> Option<Activation> {
        let (count, limit) = if cleanup {
            (&mut self.cleanup, CLEANUP_LIMIT)
        } else {
            (&mut self.foreground, FOREGROUND_LIMIT)
        };
        if *count >= limit {
            return None;
        }
        *count = count.wrapping_add(1);
        Some(Activation { cleanup })
    }

    pub fn retire(&mut self, activation: Activation) {
        let count = if activation.cleanup {
            &mut self.cleanup
        } else {
            &mut self.foreground
        };
        *count = count
            .checked_sub(1)
            .expect("activation retired without admission");
    }

    /// Retire a token at a validated native invocation boundary without a fault path.
    ///
    /// # Safety
    /// The token came from this counter's successful admission and has not been
    /// retired. Native strict C proves the matching entry and exit on every edge.
    #[inline(always)]
    pub unsafe fn retire_unchecked(&mut self, activation: Activation) {
        let count = if activation.cleanup {
            &mut self.cleanup
        } else {
            &mut self.foreground
        };
        *count = count.wrapping_sub(1);
    }

    pub fn live(&self) -> (u16, u16) {
        (self.foreground, self.cleanup)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exhausted_foreground_does_not_spend_or_reset_cleanup() {
        let mut counts = Activations::default();
        let foreground: Vec<_> = (0..FOREGROUND_LIMIT)
            .map(|_| counts.admit().unwrap())
            .collect();
        assert!(counts.admit().is_none());
        let first = counts.admit_finalizer().unwrap();
        let nested: Vec<_> = (1..CLEANUP_LIMIT)
            .map(|_| counts.admit().unwrap())
            .collect();
        assert!(counts.admit().is_none());
        assert!(counts.admit_finalizer().is_none());
        assert_eq!(counts.live(), (FOREGROUND_LIMIT, CLEANUP_LIMIT));
        for invocation in nested.into_iter().rev() {
            counts.retire(invocation);
        }
        counts.retire(first);
        // A sibling may run after retirement, even while foreground stays full.
        let sibling = counts.admit_finalizer().unwrap();
        counts.retire(sibling);
        assert!(counts.admit().is_none());
        for invocation in foreground.into_iter().rev() {
            counts.retire(invocation);
        }
        assert_eq!(counts.live(), (0, 0));
    }
}
