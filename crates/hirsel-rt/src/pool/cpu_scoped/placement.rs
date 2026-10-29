//! Linux sched_getaffinity/sched_setaffinity, with an explicit saved mask for
//! the caller that returns to the ordinary executor after its scoped drain.
use super::Refusal;

fn mask() -> Result<Vec<libc::c_ulong>, Refusal> {
    let mut words =
        vec![0; std::mem::size_of::<libc::cpu_set_t>() / std::mem::size_of::<libc::c_ulong>()];
    loop {
        // Linux accepts a larger mask and zeroes words beyond its CPU set.
        if unsafe {
            libc::sched_getaffinity(
                0,
                std::mem::size_of_val(words.as_slice()),
                words.as_mut_ptr().cast(),
            )
        } == 0
        {
            return Ok(words);
        }
        if std::io::Error::last_os_error().raw_os_error() != Some(libc::EINVAL) {
            return Err(Refusal::ResourceLimit);
        }
        let size = words.len().checked_mul(2).ok_or(Refusal::ResourceLimit)?;
        words
            .try_reserve(size - words.len())
            .map_err(|_| Refusal::ResourceLimit)?;
        words.resize(size, 0);
    }
}

pub(super) fn cpus() -> Result<Vec<usize>, Refusal> {
    let mask = mask()?;
    let cpus: Vec<_> = mask
        .iter()
        .enumerate()
        .flat_map(|(word, bits)| {
            (0..libc::c_ulong::BITS as usize).filter_map(move |bit| {
                (bits & (1 << bit) != 0).then_some(word * libc::c_ulong::BITS as usize + bit)
            })
        })
        .collect();
    if cpus.is_empty() {
        return Err(Refusal::ResourceLimit);
    }
    Ok(cpus)
}

pub(super) struct Pin {
    tid: libc::pid_t,
    previous: Vec<libc::c_ulong>,
}
impl Pin {
    pub(super) fn new(cpu: usize) -> Result<Self, Refusal> {
        let previous = mask()?;
        let mut selected: Vec<libc::c_ulong> = vec![0; previous.len()];
        let word = cpu / libc::c_ulong::BITS as usize;
        let Some(bits) = selected.get_mut(word) else {
            return Err(Refusal::ResourceLimit);
        };
        *bits = 1 << (cpu % libc::c_ulong::BITS as usize);
        let tid = unsafe { libc::syscall(libc::SYS_gettid) } as libc::pid_t;
        if unsafe {
            libc::sched_setaffinity(
                tid,
                std::mem::size_of_val(selected.as_slice()),
                selected.as_ptr().cast(),
            )
        } != 0
        {
            return Err(Refusal::ResourceLimit);
        }
        Ok(Self { tid, previous })
    }
    pub(super) fn tid(&self) -> libc::pid_t {
        self.tid
    }
}
impl Drop for Pin {
    fn drop(&mut self) {
        if unsafe {
            libc::sched_setaffinity(
                self.tid,
                std::mem::size_of_val(self.previous.as_slice()),
                self.previous.as_ptr().cast(),
            )
        } != 0
        {
            crate::environmental("a scoped CPU caller's placement could not be restored");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_slot_pin_restores_the_callers_original_mask_after_its_drain() {
        let before = mask().unwrap();
        let cpus = cpus().unwrap();
        {
            let pin = Pin::new(*cpus.last().unwrap()).unwrap();
            assert_eq!(self::cpus().unwrap(), vec![*cpus.last().unwrap()]);
            assert_eq!(pin.tid(), unsafe { libc::syscall(libc::SYS_gettid) }
                as libc::pid_t);
        }
        assert_eq!(mask().unwrap(), before);
    }
}
