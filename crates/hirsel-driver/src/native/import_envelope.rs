//! Private stored-import framing. The semantic section codecs live in import_cache.
//!
//! Adapted from Go ebe2feefc400f813aefdc8a036e66d2619da6649:
//! cmd/go/internal/cache/cache.go:200-255 (version/header/complete-entry checks),
//! encoding/binary/varint.go:32-86 (bounded unsigned lengths).
//! Hirsel uses five ordered tags, canonical varints and borrowed sections; Go's
//! action identity/checksum remains the responsibility of the existing object store.
//!
//! Copyright 2009 The Go Authors.
//! Copyright 2011 The Go Authors. All rights reserved.
//! Copyright 2017 The Go Authors. All rights reserved.
//!
//! Redistribution and use in source and binary forms, with or without
//! modification, are permitted provided that the following conditions are met:
//! * Redistributions of source code must retain the above copyright notice,
//!   this list of conditions and the following disclaimer.
//! * Redistributions in binary form must reproduce the above copyright notice,
//!   this list of conditions and the following disclaimer in the documentation
//!   and/or other materials provided with the distribution.
//! * Neither the name of Google LLC nor the names of its contributors may be
//!   used to endorse or promote products derived from this software without
//!   specific prior written permission.
//!
//! THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS "AS IS"
//! AND ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE
//! IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE
//! ARE DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT OWNER OR CONTRIBUTORS BE
//! LIABLE FOR ANY DIRECT, INDIRECT, INCIDENTAL, SPECIAL, EXEMPLARY, OR
//! CONSEQUENTIAL DAMAGES (INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF
//! SUBSTITUTE GOODS OR SERVICES; LOSS OF USE, DATA, OR PROFITS; OR BUSINESS
//! INTERRUPTION) HOWEVER CAUSED AND ON ANY THEORY OF LIABILITY, WHETHER IN
//! CONTRACT, STRICT LIABILITY, OR TORT (INCLUDING NEGLIGENCE OR OTHERWISE)
//! ARISING IN ANY WAY OUT OF THE USE OF THIS SOFTWARE, EVEN IF ADVISED OF THE
//! POSSIBILITY OF SUCH DAMAGE.

// Version 2 splits the old effects (TypeTable, Vec<u8>) JSON into table JSON
// and raw effects JSON. As in serde_json 20572a6d src/raw.rs:16-76, preserve
// nested JSON bytes verbatim. Length framing avoids even a RawValue scan before
// the one semantic parse of each section; no serde_json source is copied.
// No legacy JSON or version-1 fallback: an old stored entry is an ordinary import-cache miss.
const MAGIC: &[u8; 6] = b"HIRIMP";
const VERSION: u8 = 2;
const SECTIONS: u8 = 5;

fn length(out: &mut Vec<u8>, mut value: u64) {
    while value >= 0x80 {
        out.push(value as u8 | 0x80);
        value >>= 7;
    }
    out.push(value as u8);
}

fn read_length(bytes: &[u8], cursor: &mut usize) -> Option<usize> {
    let mut value = 0u64;
    for i in 0..10 {
        let byte = *bytes.get(*cursor)?;
        *cursor = cursor.checked_add(1)?;
        if i == 9 && byte > 1 {
            return None;
        }
        value |= u64::from(byte & 0x7f) << (7 * i);
        if byte < 0x80 {
            // Unlike Go, reject alternate encodings of the same length.
            if i > 0 && byte == 0 {
                return None;
            }
            return usize::try_from(value).ok();
        }
    }
    None
}

pub(super) fn encode(sections: [&[u8]; 5]) -> Option<Vec<u8>> {
    let mut header = Vec::with_capacity(63);
    header.extend_from_slice(MAGIC);
    header.extend_from_slice(&[VERSION, SECTIONS]);
    let mut total = 0usize;
    for (tag, section) in sections.iter().enumerate() {
        header.push(tag as u8);
        length(&mut header, u64::try_from(section.len()).ok()?);
        total = total.checked_add(section.len())?;
    }
    let mut out = Vec::new();
    out.try_reserve_exact(header.len().checked_add(total)?)
        .ok()?;
    out.extend_from_slice(&header);
    for section in sections {
        out.extend_from_slice(section);
    }
    Some(out)
}

pub(super) fn decode(bytes: &[u8]) -> Option<[&[u8]; 5]> {
    if bytes.get(..6)? != MAGIC || *bytes.get(6)? != VERSION || *bytes.get(7)? != SECTIONS {
        return None;
    }
    let mut cursor = 8usize;
    let mut lengths = [0usize; 5];
    let mut total = 0usize;
    for (tag, size) in lengths.iter_mut().enumerate() {
        if *bytes.get(cursor)? != tag as u8 {
            return None;
        }
        cursor = cursor.checked_add(1)?;
        *size = read_length(bytes, &mut cursor)?;
        total = total.checked_add(*size)?;
    }
    // Validate every length and complete consumption before any payload slicing.
    if cursor.checked_add(total)? != bytes.len() {
        return None;
    }
    let mut sections = [&[][..]; 5];
    for (section, size) in sections.iter_mut().zip(lengths) {
        let end = cursor.checked_add(size)?;
        *section = bytes.get(cursor..end)?;
        cursor = end;
    }
    Some(sections)
}
