//! Optional physical ahead-adoption evidence, separate from semantic events.
use std::{
    io::Write,
    sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
    },
};
static ENABLED: AtomicBool = AtomicBool::new(false);
static TRACE: Mutex<Option<std::fs::File>> = Mutex::new(None);
pub(crate) fn open(file: std::fs::File) {
    *TRACE.lock().unwrap_or_else(|e| e.into_inner()) = Some(file);
    ENABLED.store(true, Ordering::Relaxed);
}
pub(crate) fn adopted(slot: u32, generation: u32, pc: *mut u8) {
    if !ENABLED.load(Ordering::Relaxed) {
        return;
    }
    if let Some(file) = TRACE.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
        writeln!(file, "adopt {slot} {generation} {}", pc.addr())
            .unwrap_or_else(|_| crate::fault("writing ahead adoption trace"));
    }
}
