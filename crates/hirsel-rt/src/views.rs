//! SEM-3 Unicode 16 borrowed loop cursors at the Hirsel ABI.
//!
//! The cursor retains the complete borrowed slice. Its relative byte position
//! preserves StrPos provenance even when the slice starts inside another slice.
use crate::{Header, STATUS_OK, TaskCtx, alloc::payload, fault};

/// The physical String/Bytes/view value; the owner credit is held by the caller.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct Slice {
    pub owner: *mut Header,
    pub start: u64,
    pub length: u64,
}

/// `$intrinsic.ViewCursor[T]`: an uncounted borrowed view and next byte position.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct ViewCursor {
    pub view: Slice,
    pub position: u64,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct ScalarOption {
    pub tag: u32,
    pub value: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct PositionOption {
    pub tag: u32,
    pub padding: u32,
    pub value: u64,
}

impl Slice {
    /// # Safety
    /// The owner is live and the slice bounds describe initialized UTF-8 bytes.
    pub(crate) unsafe fn text<'a>(self) -> &'a str {
        let start = usize::try_from(self.start).unwrap_or_else(|_| fault("view start overflow"));
        let length = usize::try_from(self.length).unwrap_or_else(|_| fault("view length overflow"));
        self.start
            .checked_add(self.length)
            .unwrap_or_else(|| fault("view end overflow"));
        // Slice bytes begin past the payload's length prefix; this also admits
        // a FlatView into a flat block.
        let bytes =
            unsafe { std::slice::from_raw_parts(payload(self.owner).add(8).add(start), length) };
        // SAFETY: String validity is the input contract; rescanning the whole
        // slice at each step would make scalar/position traversal quadratic.
        unsafe { std::str::from_utf8_unchecked(bytes) }
    }
}

macro_rules! open {
    ($name:ident) => {
        /// Borrow the source slice without incrementing its owner.
        /// # Safety
        /// `ctx` is live, `out` is writable, and the source view stays live
        /// until the cursor's last use. The destination is the unused ABI word.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn $name(
            _ctx: *mut TaskCtx,
            _destination: *mut std::ffi::c_void,
            out: *mut ViewCursor,
            owner: *mut Header,
            start: u64,
            length: u64,
        ) -> u8 {
            unsafe {
                out.write(ViewCursor {
                    view: Slice {
                        owner,
                        start,
                        length,
                    },
                    position: 0,
                })
            };
            STATUS_OK
        }
    };
}
open!(hirsel_rt_v2_unicode_scalars_open);
open!(hirsel_rt_v2_scalar_positions_open);

/// # Safety
/// Context, output and exclusive cursor address are live; its borrowed view is live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_unicode_scalars_next(
    _ctx: *mut TaskCtx,
    _destination: *mut std::ffi::c_void,
    out: *mut ScalarOption,
    cursor: *mut ViewCursor,
) -> u8 {
    let cursor = unsafe { &mut *cursor };
    let mut result = ScalarOption::default();
    if cursor.position < cursor.view.length {
        let text = unsafe { cursor.view.text() };
        let scalar = text[cursor.position as usize..].chars().next().unwrap();
        cursor.position += scalar.len_utf8() as u64;
        result = ScalarOption {
            tag: 1,
            value: u32::from(scalar),
        };
    }
    unsafe { out.write(result) };
    STATUS_OK
}

/// Yield every scalar start followed by the terminal position, even for empty slices.
/// # Safety
/// Context, output and exclusive cursor address are live; its borrowed view is live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hirsel_rt_v2_scalar_positions_next(
    _ctx: *mut TaskCtx,
    _destination: *mut std::ffi::c_void,
    out: *mut PositionOption,
    cursor: *mut ViewCursor,
) -> u8 {
    let cursor = unsafe { &mut *cursor };
    let mut result = PositionOption::default();
    if cursor.position <= cursor.view.length {
        result = PositionOption {
            tag: 1,
            padding: 0,
            value: cursor.position,
        };
        if cursor.position == cursor.view.length {
            cursor.position = cursor
                .position
                .checked_add(1)
                .unwrap_or_else(|| fault("view position overflow"));
        } else {
            let text = unsafe { cursor.view.text() };
            cursor.position += text[cursor.position as usize..]
                .chars()
                .next()
                .unwrap()
                .len_utf8() as u64;
        }
    }
    unsafe { out.write(result) };
    STATUS_OK
}
