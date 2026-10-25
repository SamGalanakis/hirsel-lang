use super::storage::{Mapping, Stage, layer_path};
use super::{
    Entry, Key, KeyKind, Location, MAX_BYTES, MAX_LAYERS, MAX_ROWS, MAX_SCRATCH, admission, input,
    invalid,
};
use hirsel_syntax::ContentHash;
use std::{
    fs::{self, File},
    io::{self, Read},
    path::Path,
    sync::atomic::{AtomicU8, AtomicU64, Ordering},
};

const BLOCK: usize = 4096;
const STRIDE: usize = 4101;
const UNKNOWN: u8 = 0;
const GOOD: u8 = 1;
const BAD: u8 = 2;

// LevelDB util/crc32c.h:25-42 (BSD-3-Clause); see third-party/NOTICES.md.
pub(super) fn mask(crc: u32) -> u32 {
    crc.rotate_right(15).wrapping_add(0xa282ead8)
}
fn crc(payload: &[u8], plane: usize, block: usize) -> u32 {
    let mut suffix = [0; 13];
    suffix[0] = (plane + 1) as u8;
    suffix[1..5].copy_from_slice(&(plane as u32).to_le_bytes());
    suffix[5..9].copy_from_slice(&(block as u32).to_le_bytes());
    suffix[9..13].copy_from_slice(&(payload.len() as u32).to_le_bytes());
    mask(crc32c::crc32c_append(crc32c::crc32c(payload), &suffix))
}

#[cfg(test)]
#[path = "format_tests.rs"]
mod tests;
fn u32_at(bytes: &[u8], offset: usize) -> Option<u32> {
    Some(u32::from_le_bytes(
        bytes.get(offset..offset.checked_add(4)?)?.try_into().ok()?,
    ))
}
fn u64_at(bytes: &[u8], offset: usize) -> Option<u64> {
    Some(u64::from_le_bytes(
        bytes.get(offset..offset.checked_add(8)?)?.try_into().ok()?,
    ))
}
fn put32(bytes: &mut [u8], offset: usize, n: u32) {
    bytes[offset..offset + 4].copy_from_slice(&n.to_le_bytes());
}
fn put64(bytes: &mut [u8], offset: usize, n: u64) {
    bytes[offset..offset + 8].copy_from_slice(&n.to_le_bytes());
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct Descriptor {
    pub(super) id: ContentHash,
    pub(super) rows: u64,
    pub(super) len: u64,
}
#[derive(Default)]
pub(super) struct Manifest {
    pub(super) number: u64,
    pub(super) layers: Vec<Descriptor>,
}
impl Manifest {
    pub(super) fn decode(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < 36
            || bytes.len() > 420
            || bytes.get(..8)? != b"HIRIM036"
            || u32_at(bytes, 8)? != 1
            || u32_at(bytes, 12)? != 32
            || u32_at(bytes, 28)? != 0
        {
            return None;
        }
        let number = u64_at(bytes, 16)?;
        let count = u32_at(bytes, 24)? as usize;
        if number == 0
            || count > MAX_LAYERS
            || bytes.len() != 36 + count * 48
            || u32_at(bytes, bytes.len() - 4)? != mask(crc32c::crc32c(&bytes[..bytes.len() - 4]))
        {
            return None;
        }
        let mut layers = Vec::with_capacity(count);
        for i in 0..count {
            let offset = 32 + i * 48;
            let descriptor = Descriptor {
                id: ContentHash(bytes.get(offset..offset + 32)?.try_into().ok()?),
                rows: u64_at(bytes, offset + 32)?,
                len: u64_at(bytes, offset + 40)?,
            };
            if descriptor.rows == 0
                || descriptor.rows > MAX_ROWS as u64
                || !(128..=MAX_BYTES as u64).contains(&descriptor.len)
                || layers.iter().any(|d: &Descriptor| d.id == descriptor.id)
            {
                return None;
            }
            layers.push(descriptor);
        }
        if admission(&layers).is_some() {
            return None;
        }
        Some(Self { number, layers })
    }
    pub(super) fn encode(&self) -> io::Result<Vec<u8>> {
        if self.number == 0 || admission(&self.layers).is_some() {
            return Err(invalid());
        }
        let mut bytes = vec![0; 36 + 48 * self.layers.len()];
        bytes[..8].copy_from_slice(b"HIRIM036");
        put32(&mut bytes, 8, 1);
        put32(&mut bytes, 12, 32);
        put64(&mut bytes, 16, self.number);
        put32(&mut bytes, 24, self.layers.len() as u32);
        for (i, d) in self.layers.iter().enumerate() {
            let offset = 32 + i * 48;
            bytes[offset..offset + 32].copy_from_slice(&d.id.0);
            put64(&mut bytes, offset + 32, d.rows);
            put64(&mut bytes, offset + 40, d.len);
        }
        let end = bytes.len() - 4;
        let crc = mask(crc32c::crc32c(&bytes[..end]));
        put32(&mut bytes, end, crc);
        Ok(bytes)
    }
    pub(super) fn read(root: &Path) -> io::Result<Self> {
        Self::read_with_status(root).map(|(manifest, _)| manifest)
    }
    pub(super) fn read_with_status(root: &Path) -> io::Result<(Self, bool)> {
        let file = match File::open(root.join("index/MANIFEST")) {
            Ok(file) => file,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok((Self::default(), false)),
            Err(e) => return Err(e),
        };
        let mut bytes = Vec::with_capacity(421);
        file.take(421).read_to_end(&mut bytes)?;
        Ok(match Self::decode(&bytes) {
            Some(manifest) => (manifest, false),
            None => (Self::default(), true),
        })
    }
    pub(super) fn publish(&mut self, root: &Path, stage: &Stage) -> io::Result<ContentHash> {
        self.number = self.number.checked_add(1).ok_or_else(invalid)?;
        let bytes = self.encode()?;
        let id = ContentHash::of_parts("hirsel.artifact.index-manifest.v36", [&bytes[..]]);
        stage.write("manifest", &bytes)?;
        fs::rename(stage.path.join("manifest"), root.join("index/MANIFEST"))?;
        Ok(id)
    }
}

fn radix(rows: usize) -> u8 {
    (usize::BITS - (rows.div_ceil(16) - 1).leading_zeros()).clamp(1, 16) as u8
}
fn bucket(key: Key, r: u8) -> usize {
    ((key.kind as usize) << r)
        + (u16::from_be_bytes([key.digest.0[0], key.digest.0[1]]) as usize >> (16 - r))
}
fn key_bytes(key: Key) -> [u8; 33] {
    let mut bytes = [0; 33];
    bytes[0] = key.kind as u8;
    bytes[1..].copy_from_slice(&key.digest.0);
    bytes
}
fn decode_key(bytes: [u8; 33]) -> Option<Key> {
    let kind = match bytes[0] {
        0 => KeyKind::Content,
        1 => KeyKind::Association,
        _ => return None,
    };
    Some(Key {
        kind,
        digest: ContentHash(bytes[1..].try_into().ok()?),
    })
}
fn valid_location(location: Location) -> bool {
    location.offset >= 64
        && location.offset.is_multiple_of(8)
        && (1..=4160).contains(&location.len)
        && location
            .offset
            .checked_add(location.len)
            .is_some_and(|end| end <= 1 << 20)
}

#[derive(Clone, Copy, Default)]
struct Plane {
    start: usize,
    len: usize,
    status: usize,
}
impl Plane {
    fn blocks(self) -> usize {
        self.len.div_ceil(BLOCK)
    }
    fn physical(self) -> Option<usize> {
        self.len.checked_add(self.blocks().checked_mul(5)?)
    }
    fn at(self, logical: usize) -> usize {
        self.start + (logical / BLOCK) * STRIDE + logical % BLOCK
    }
    #[inline]
    fn write(self, output: &mut [u8], mut offset: usize, mut bytes: &[u8]) {
        while !bytes.is_empty() {
            let n = bytes.len().min(BLOCK - offset % BLOCK);
            let physical = self.at(offset);
            output[physical..physical + n].copy_from_slice(&bytes[..n]);
            offset += n;
            bytes = &bytes[n..];
        }
    }
}
fn plane_sizes(rows: usize, segments: usize) -> Option<[usize; 5]> {
    Some([
        4usize.checked_mul((2usize << radix(rows)) + 1)?,
        rows.checked_add(15)? & !15,
        rows.checked_mul(33)?,
        rows.checked_mul(12)?,
        segments.checked_mul(32)?,
    ])
}

pub(super) fn construction_scratch(rows: usize) -> Option<usize> {
    if rows == 0 || rows > MAX_ROWS {
        return None;
    }
    let physical = plane_sizes(rows, rows)?
        .into_iter()
        .try_fold(128usize, |n, len| {
            n.checked_add(len)?
                .checked_add(len.div_ceil(BLOCK).checked_mul(5)?)
        })?;
    // An output exceeding MAX_BYTES is rejected before its allocation.
    physical
        .min(MAX_BYTES)
        .checked_add(rows.checked_mul(size_of::<ContentHash>())?)
}

pub(super) fn validation_scratch(rows: usize, len: usize) -> usize {
    // Directory, bucket status, block status (at most five partial blocks).
    5 * (2usize << radix(rows)) + 4 + len.div_ceil(BLOCK) + 5 + size_of::<Layer>()
}

fn layout(rows: usize, segments: usize) -> Option<([Plane; 5], usize, usize)> {
    if rows == 0 || rows > MAX_ROWS || segments == 0 || segments > rows {
        return None;
    }
    let sizes = plane_sizes(rows, segments)?;
    let mut planes = [Plane::default(); 5];
    let mut end = 128usize;
    let mut blocks = 0;
    for (plane, len) in planes.iter_mut().zip(sizes) {
        *plane = Plane {
            start: end,
            len,
            status: blocks,
        };
        end = end.checked_add(plane.physical()?)?;
        blocks = blocks.checked_add(plane.blocks())?;
    }
    if end > MAX_BYTES {
        return None;
    }
    Some((planes, end, blocks))
}

pub(super) fn encode_layer(entries: &[Entry]) -> io::Result<Vec<u8>> {
    let rows = entries.len();
    if rows == 0 || rows > MAX_ROWS || rows.checked_mul(32).is_none_or(|n| n > MAX_SCRATCH) {
        return Err(input());
    }
    encode_replayed(rows, || Ok(entries.iter().copied().map(Ok))).map(|(bytes, _)| bytes)
}

/// Replay immutable sorted streams instead of retaining a merged Entry per row.
/// The first pass retains only segment IDs; the second fills physical planes
/// directly, including cross-block rows. Sorting uses no auxiliary allocation.
pub(super) fn encode_replayed<I: Iterator<Item = io::Result<Entry>>>(
    row_limit: usize,
    replay: impl Fn() -> io::Result<I>,
) -> io::Result<(Vec<u8>, usize)> {
    let bound = construction_scratch(row_limit).ok_or_else(input)?;
    let mut dictionary = Vec::new();
    dictionary
        .try_reserve_exact(row_limit)
        .map_err(|_| input())?;
    let mut rows = 0;
    let mut previous = None;
    for entry in replay()? {
        let entry = entry?;
        if rows == row_limit
            || previous.is_some_and(|k| k >= entry.key)
            || !valid_location(entry.location)
        {
            return Err(input());
        }
        previous = Some(entry.key);
        rows += 1;
        if dictionary.last() != Some(&entry.location.segment) {
            dictionary.push(entry.location.segment);
        }
    }
    dictionary.sort_unstable();
    dictionary.dedup();
    let (planes, len, blocks) = layout(rows, dictionary.len()).ok_or_else(input)?;
    if len + dictionary.capacity() * size_of::<ContentHash>() > bound {
        return Err(input());
    }
    let mut bytes = Vec::new();
    bytes.try_reserve_exact(len).map_err(|_| input())?;
    bytes.resize(len, 0);
    bytes[..8].copy_from_slice(b"HIRIL036");
    put32(&mut bytes, 8, 1);
    put32(&mut bytes, 12, 128);
    put32(&mut bytes, 16, rows as u32);
    put32(&mut bytes, 20, dictionary.len() as u32);
    bytes[24] = 2;
    bytes[25] = radix(rows);
    put32(&mut bytes, 28, BLOCK as u32);
    for (j, plane) in planes.iter().enumerate() {
        put32(&mut bytes, 32 + j * 8, plane.start as u32);
        put32(&mut bytes, 36 + j * 8, plane.len as u32);
    }
    put32(&mut bytes, 72, len as u32);
    put32(&mut bytes, 76, blocks as u32);
    let mut next_bucket = 0;
    let mut last_segment = None;
    #[cfg(test)]
    super::construction_observation(
        dictionary.capacity() * size_of::<ContentHash>() + bytes.capacity(),
    );
    let mut written = 0;
    for (i, entry) in replay()?.enumerate() {
        let entry = entry?;
        if i >= rows {
            return Err(input());
        }
        written += 1;
        let b = bucket(entry.key, bytes[25]);
        while next_bucket <= b {
            planes[0].write(&mut bytes, next_bucket * 4, &(i as u32).to_le_bytes());
            next_bucket += 1;
        }
        planes[1].write(&mut bytes, i, &[entry.key.digest.0[2]]);
        planes[2].write(&mut bytes, i * 33, &key_bytes(entry.key));
        let mut locator = [0; 12];
        let segment = match last_segment {
            Some((id, index)) if id == entry.location.segment => index,
            _ => dictionary
                .binary_search(&entry.location.segment)
                .map_err(|_| input())?,
        };
        last_segment = Some((entry.location.segment, segment));
        put32(&mut locator, 0, segment as u32);
        put32(&mut locator, 4, entry.location.offset / 8);
        put32(&mut locator, 8, entry.location.len);
        planes[3].write(&mut bytes, i * 12, &locator);
    }
    if written != rows {
        return Err(input());
    }
    while next_bucket < planes[0].len / 4 {
        planes[0].write(&mut bytes, next_bucket * 4, &(rows as u32).to_le_bytes());
        next_bucket += 1;
    }
    for (i, segment) in dictionary.iter().enumerate() {
        planes[4].write(&mut bytes, i * 32, &segment.0);
    }
    for (j, plane) in planes.iter().enumerate() {
        for k in 0..plane.blocks() {
            let start = plane.start + k * STRIDE;
            let end = start + (plane.len - k * BLOCK).min(BLOCK);
            let checksum = crc(&bytes[start..end], j, k);
            bytes[end] = (j + 1) as u8;
            put32(&mut bytes, end + 1, checksum);
        }
    }
    let checksum = mask(crc32c::crc32c(&bytes[..124]));
    put32(&mut bytes, 124, checksum);
    Ok((bytes, rows))
}

pub(super) struct Layer {
    backing: Mapping,
    planes: [Plane; 5],
    directory: Vec<u32>,
    blocks: Vec<AtomicU8>,
    buckets: Vec<AtomicU8>,
    bad_blocks: AtomicU64,
    bad_buckets: AtomicU64,
    rows: usize,
    segments: usize,
    radix: u8,
}
impl Layer {
    pub(super) fn open(root: &Path, descriptor: Descriptor) -> io::Result<Self> {
        let backing = Mapping::open(&layer_path(root, descriptor), descriptor.len)?;
        Self::parse(backing, descriptor)
    }
    fn parse(backing: Mapping, descriptor: Descriptor) -> io::Result<Self> {
        let b = backing.bytes();
        let header = b.get(..128).ok_or_else(invalid)?;
        if &header[..8] != b"HIRIL036"
            || u32_at(header, 8) != Some(1)
            || u32_at(header, 12) != Some(128)
            || header[24] != 2
            || header[26..28] != [0; 2]
            || u32_at(header, 28) != Some(BLOCK as u32)
            || header[80..124] != [0; 44]
            || u32_at(header, 124) != Some(mask(crc32c::crc32c(&header[..124])))
        {
            return Err(invalid());
        }
        let rows = u32_at(header, 16).ok_or_else(invalid)? as usize;
        let segments = u32_at(header, 20).ok_or_else(invalid)? as usize;
        let (planes, len, blocks) = layout(rows, segments).ok_or_else(invalid)?;
        if descriptor.rows != rows as u64
            || b.len() != len
            || header[25] != radix(rows)
            || u32_at(header, 72) != Some(len as u32)
            || u32_at(header, 76) != Some(blocks as u32)
        {
            return Err(invalid());
        }
        for (j, plane) in planes.iter().enumerate() {
            if u32_at(header, 32 + j * 8) != Some(plane.start as u32)
                || u32_at(header, 36 + j * 8) != Some(plane.len as u32)
            {
                return Err(invalid());
            }
        }
        let radix = header[25];
        let count = (2usize << radix) + 1;
        // All capacities derive from capped canonical layout, never raw offsets.
        let mut layer = Self {
            backing,
            planes,
            directory: Vec::with_capacity(count),
            blocks: (0..blocks).map(|_| AtomicU8::new(UNKNOWN)).collect(),
            buckets: (0..count - 1).map(|_| AtomicU8::new(UNKNOWN)).collect(),
            bad_blocks: AtomicU64::new(0),
            bad_buckets: AtomicU64::new(0),
            rows,
            segments,
            radix,
        };
        let mut previous = 0;
        for i in 0..count {
            let value = u32::from_le_bytes(layer.read::<4>(0, i * 4).ok_or_else(invalid)?);
            if value < previous
                || value as usize > rows
                || (i == 0 && value != 0)
                || (i == count - 1 && value as usize != rows)
            {
                return Err(invalid());
            }
            layer.directory.push(value);
            previous = value;
        }
        Ok(layer)
    }
    pub(super) fn retained_bytes(&self) -> u64 {
        (self.backing.bytes().len()
            + self.directory.capacity() * 4
            + self.blocks.capacity()
            + self.buckets.capacity()) as u64
    }
    pub(super) fn rows(&self) -> usize {
        self.rows
    }
    pub(super) fn diagnostics(&self) -> (u64, u64) {
        (
            self.bad_blocks.load(Ordering::Relaxed),
            self.bad_buckets.load(Ordering::Relaxed),
        )
    }
    #[inline]
    fn check_block(&self, j: usize, k: usize) -> bool {
        let plane = self.planes[j];
        let Some(status) = self.blocks.get(plane.status + k) else {
            return false;
        };
        match status.load(Ordering::Acquire) {
            GOOD => return true,
            BAD => return false,
            _ => {}
        }
        let start = plane.start + k * STRIDE;
        let length = (plane.len - k * BLOCK).min(BLOCK);
        let end = start + length;
        let bytes = self.backing.bytes();
        let good = bytes.get(end) == Some(&((j + 1) as u8))
            && u32_at(bytes, end + 1) == Some(crc(&bytes[start..end], j, k))
            && (j != 1
                || k + 1 != plane.blocks()
                || self.rows == plane.len
                || bytes[plane.at(self.rows)..end].iter().all(|b| *b == 0));
        let result = if good { GOOD } else { BAD };
        // Never replace a previously observed Bad, even if validation races.
        let previous = status
            .compare_exchange(UNKNOWN, result, Ordering::AcqRel, Ordering::Acquire)
            .unwrap_or_else(|old| old);
        if previous == UNKNOWN && !good {
            self.bad_blocks.fetch_add(1, Ordering::Relaxed);
        }
        previous != BAD && good
    }
    #[inline]
    fn read<const N: usize>(&self, j: usize, mut offset: usize) -> Option<[u8; N]> {
        let plane = self.planes[j];
        if offset.checked_add(N)? > plane.len {
            return None;
        }
        // Fixed-size copies are the common path. A row that crosses a trailer
        // still checks both CRC blocks and gathers below.
        if N <= BLOCK - offset % BLOCK {
            if !self.check_block(j, offset / BLOCK) {
                return None;
            }
            let physical = plane.at(offset);
            return self
                .backing
                .bytes()
                .get(physical..physical + N)?
                .try_into()
                .ok();
        }
        let mut out = [0; N];
        let mut filled = 0;
        while filled < N {
            if !self.check_block(j, offset / BLOCK) {
                return None;
            }
            let take = (N - filled).min(BLOCK - offset % BLOCK);
            let physical = plane.at(offset);
            out[filled..filled + take]
                .copy_from_slice(self.backing.bytes().get(physical..physical + take)?);
            filled += take;
            offset += take;
        }
        Some(out)
    }
    #[inline]
    pub(super) fn key(&self, row: usize) -> Option<Key> {
        if row >= self.rows {
            return None;
        }
        decode_key(self.read(2, row.checked_mul(33)?)?)
    }
    fn validate_bucket(&self, b: usize) -> bool {
        let status = &self.buckets[b];
        match status.load(Ordering::Acquire) {
            GOOD => return true,
            BAD => return false,
            _ => {}
        }
        let mut previous = None;
        let mut good = true;
        for row in self.directory[b] as usize..self.directory[b + 1] as usize {
            let Some(key) = self.key(row) else {
                good = false;
                break;
            };
            if bucket(key, self.radix) != b || previous.is_some_and(|k| k >= key) {
                good = false;
                break;
            }
            previous = Some(key);
        }
        let result = if good { GOOD } else { BAD };
        let previous = status
            .compare_exchange(UNKNOWN, result, Ordering::AcqRel, Ordering::Acquire)
            .unwrap_or_else(|old| old);
        if previous == UNKNOWN && !good {
            self.bad_buckets.fetch_add(1, Ordering::Relaxed);
        }
        previous != BAD && good
    }
    pub(super) fn entry(&self, row: usize) -> Option<Entry> {
        let key = self.key(row)?;
        Some(Entry {
            key,
            location: self.location(row)?,
        })
    }
    fn location(&self, row: usize) -> Option<Location> {
        if row >= self.rows {
            return None;
        }
        let bytes = self.read::<12>(3, row.checked_mul(12)?)?;
        let segment = u32_at(&bytes, 0)? as usize;
        if segment >= self.segments {
            return None;
        }
        let location = Location {
            segment: ContentHash(self.read(4, segment.checked_mul(32)?)?),
            offset: u32_at(&bytes, 4)?.checked_mul(8)?,
            len: u32_at(&bytes, 8)?,
        };
        valid_location(location).then_some(location)
    }
    pub(super) fn find(&self, key: Key) -> Option<Location> {
        let b = bucket(key, self.radix);
        let lo = self.directory[b] as usize;
        let hi = self.directory[b + 1] as usize;
        if lo == hi || !self.validate_bucket(b) {
            return None;
        }
        for start in ((lo & !15)..hi).step_by(16) {
            let tags = self.read::<16>(1, start)?;
            let mut matches = crate::group::matches(&tags, key.digest.0[2]);
            matches &= u16::MAX << lo.saturating_sub(start);
            let end = (hi - start).min(16);
            matches &= (u16::MAX as u32 >> (16 - end)) as u16;
            while matches != 0 {
                let row = start + matches.trailing_zeros() as usize;
                if self.key(row)? == key {
                    return self.location(row);
                }
                matches &= matches - 1;
            }
        }
        None
    }
    pub(super) fn validate_all(&self) -> io::Result<()> {
        for (j, plane) in self.planes.iter().enumerate() {
            for k in 0..plane.blocks() {
                if !self.check_block(j, k) {
                    return Err(invalid());
                }
            }
        }
        for b in 0..self.buckets.len() {
            if !self.validate_bucket(b) {
                return Err(invalid());
            }
        }
        let mut previous = None;
        for i in 0..self.segments {
            let segment = self.read::<32>(4, i * 32).ok_or_else(invalid)?;
            if previous.is_some_and(|p| p >= segment) {
                return Err(invalid());
            }
            previous = Some(segment);
        }
        for row in 0..self.rows {
            let entry = self.entry(row).ok_or_else(invalid)?;
            if self.read::<1>(1, row) != Some([entry.key.digest.0[2]]) {
                return Err(invalid());
            }
        }
        Ok(())
    }
}
