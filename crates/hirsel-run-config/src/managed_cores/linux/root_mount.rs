//! Linux v6.8 statmount selftest's statx -> unique mount ID -> statmount query
//! (tools/testing/selftests/filesystems/statmount/statmount_test.c:38-47,100-116).
//! Only a complete cgroup2 filesystem root can replace mount-table discovery.
use std::path::PathBuf;

#[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
pub(super) fn root_mount() -> Option<PathBuf> {
    use std::os::fd::AsRawFd;
    let directory = std::fs::File::open("/sys/fs/cgroup").ok()?;
    // SAFETY: zero is a valid statx output buffer. The descriptor is live and
    // AT_EMPTY_PATH selects it without resolving a second pathname.
    let mut stat: libc::statx = unsafe { std::mem::zeroed() };
    if unsafe {
        libc::statx(
            directory.as_raw_fd(),
            c"".as_ptr(),
            libc::AT_EMPTY_PATH,
            libc::STATX_MNT_ID_UNIQUE,
            &mut stat,
        )
    } != 0
        || stat.stx_mask & libc::STATX_MNT_ID_UNIQUE == 0
    {
        return None;
    }
    #[repr(C)]
    struct Request {
        size: u32,
        spare: u32,
        mount: u64,
        mask: u64,
    }
    let request = Request {
        size: 24,
        spare: 0,
        mount: stat.stx_mnt_id,
        mask: MASK,
    };
    let mut bytes = [0u8; 1024];
    // SAFETY: request matches Linux mnt_id_req v0. Both pointers remain live,
    // and the kernel may write only bytes.len() bytes to the output buffer.
    // statmount is syscall 457 on x86-64 and AArch64, since Linux 6.8.
    if unsafe {
        libc::syscall(
            457 as libc::c_long,
            &request,
            bytes.as_mut_ptr(),
            bytes.len(),
            0 as libc::c_uint,
        )
    } != 0
    {
        return None;
    }
    decode(&bytes, stat.stx_mnt_id)
}

#[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
pub(super) fn root_mount() -> Option<PathBuf> {
    None
}

// MNT_BASIC | MNT_ROOT | MNT_POINT | FS_TYPE, Linux UAPI mount.h.
const MASK: u64 = 0x3a;

fn decode(bytes: &[u8], mount: u64) -> Option<PathBuf> {
    let word = |at| Some(u32::from_ne_bytes(bytes.get(at..at + 4)?.try_into().ok()?));
    let wide = |at| Some(u64::from_ne_bytes(bytes.get(at..at + 8)?.try_into().ok()?));
    let size = usize::try_from(word(0)?).ok()?;
    let strings = bytes.get(512..size)?;
    let string = |at| {
        let offset = usize::try_from(word(at)?).ok()?;
        let tail = strings.get(offset..)?;
        let end = tail.iter().position(|b| *b == 0)?;
        std::str::from_utf8(&tail[..end]).ok()
    };
    if wide(8)? & MASK != MASK
        || wide(40)? != mount
        || string(36)? != "cgroup2"
        || string(104)? != "/"
        || string(108)? != "/sys/fs/cgroup"
    {
        return None;
    }
    Some(PathBuf::from("/sys/fs/cgroup"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> Vec<u8> {
        let mut bytes = vec![0; 512];
        bytes.extend_from_slice(b"cgroup2\0/\0/sys/fs/cgroup\0");
        let size = bytes.len() as u32;
        for (at, value) in [(0, size), (36, 0), (104, 8), (108, 10)] {
            bytes[at..at + 4].copy_from_slice(&value.to_ne_bytes());
        }
        bytes[8..16].copy_from_slice(&MASK.to_ne_bytes());
        bytes[40..48].copy_from_slice(&123u64.to_ne_bytes());
        bytes
    }

    #[test]
    fn only_a_complete_cgroup_root_replaces_discovery() {
        let bytes = fixture();
        assert_eq!(decode(&bytes, 123), Some(PathBuf::from("/sys/fs/cgroup")));
        assert_eq!(decode(&bytes, 124), None);
        for at in [8, 36, 104, 108, 512, 520, 522] {
            let mut bad = bytes.clone();
            bad[at] ^= 0xff;
            assert_eq!(decode(&bad, 123), None, "offset {at}");
        }
        for end in 0..bytes.len() {
            assert_eq!(decode(&bytes[..end], 123), None, "length {end}");
        }
    }
}
