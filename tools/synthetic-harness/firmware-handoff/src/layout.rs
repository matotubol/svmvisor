//! Checked executable package parsing and memory ownership, independent of firmware.
pub const ARENA_BYTES: usize = 0x100000;
pub const HANDOFF_OFFSET: usize = 0xff000;
const HEADER_BYTES: usize = 64;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LayoutError {
    Header,
    Bounds,
    Entry,
    Relocation,
    Arena,
}

pub struct Payload<'a> {
    bytes: &'a [u8],
    linked_base: u64,
    image_bytes: usize,
    memory_bytes: usize,
    entry_offset: usize,
    relocation_count: usize,
}

fn word(bytes: &[u8], offset: usize) -> u64 {
    u64::from_le_bytes(bytes[offset..offset + 8].try_into().unwrap())
}

/// An arena is wholly owned, below the bootstrap's 1 GiB map limit, and cannot
/// cross a 2 MiB page-directory window. The header owns its final 4 KiB page.
pub fn valid_arena(base: u64) -> bool {
    base >= 0x100000
        && base & 0xfff == 0
        && base
            .checked_add(ARENA_BYTES as u64)
            .is_some_and(|end| end <= 0x40000000 && base >> 21 == (end - 1) >> 21)
}

impl<'a> Payload<'a> {
    /// Validate every byte range before firmware allocates or writes memory.
    pub fn parse(bytes: &'a [u8], entry_offset: usize) -> Result<Self, LayoutError> {
        if bytes.len() < HEADER_BYTES
            || &bytes[..8] != b"SVMRELO1"
            || word(bytes, 16) != ARENA_BYTES as u64
            || word(bytes, 56) != 0
        {
            return Err(LayoutError::Header);
        }
        let size = |offset| usize::try_from(word(bytes, offset)).map_err(|_| LayoutError::Bounds);
        let payload = Self {
            bytes,
            linked_base: word(bytes, 8),
            image_bytes: size(24)?,
            memory_bytes: size(32)?,
            entry_offset: size(40)?,
            relocation_count: size(48)?,
        };
        if payload.linked_base != 0x100000
            || payload.image_bytes == 0
            || payload.image_bytes > payload.memory_bytes
            || payload.memory_bytes > HANDOFF_OFFSET
        {
            return Err(LayoutError::Bounds);
        }
        if payload.entry_offset != entry_offset || entry_offset >= payload.image_bytes {
            return Err(LayoutError::Entry);
        }
        let total = payload
            .relocation_count
            .checked_mul(16)
            .and_then(|n| n.checked_add(HEADER_BYTES))
            .and_then(|n| n.checked_add(payload.image_bytes))
            .ok_or(LayoutError::Bounds)?;
        if total != bytes.len() {
            return Err(LayoutError::Bounds);
        }
        let mut previous_end = 0;
        for i in 0..payload.relocation_count {
            let (offset, width) = payload.relocation(i)?;
            let end = offset.checked_add(width).ok_or(LayoutError::Relocation)?;
            if offset < previous_end || end > payload.image_bytes {
                return Err(LayoutError::Relocation);
            }
            previous_end = end;
            let value = payload.value(offset, width);
            if value < payload.linked_base
                || value > payload.linked_base + payload.memory_bytes as u64
            {
                return Err(LayoutError::Relocation);
            }
        }
        Ok(payload)
    }

    fn relocation(&self, index: usize) -> Result<(usize, usize), LayoutError> {
        let record = HEADER_BYTES + self.image_bytes + index * 16;
        let offset =
            usize::try_from(word(self.bytes, record)).map_err(|_| LayoutError::Relocation)?;
        let width = word(self.bytes, record + 8);
        if width != 4 && width != 8 {
            return Err(LayoutError::Relocation);
        }
        Ok((offset, width as usize))
    }

    fn value(&self, offset: usize, width: usize) -> u64 {
        let offset = HEADER_BYTES + offset;
        if width == 8 {
            word(self.bytes, offset)
        } else {
            u32::from_le_bytes(self.bytes[offset..offset + 4].try_into().unwrap()) as u64
        }
    }

    /// Initialize only the caller's exact owned arena. Validation precedes all
    /// writes; no firmware pointers or allocation side effects are needed here.
    pub fn load(&self, arena: &mut [u8], base: u64) -> Result<(), LayoutError> {
        if !valid_arena(base) || arena.len() != ARENA_BYTES {
            return Err(LayoutError::Arena);
        }
        arena.fill(0);
        arena[..self.image_bytes]
            .copy_from_slice(&self.bytes[HEADER_BYTES..HEADER_BYTES + self.image_bytes]);
        for i in 0..self.relocation_count {
            // Parsing already validated every record, and the <1GiB arena
            // restriction guarantees both unsigned and signed32 target fit.
            let (offset, width) = self.relocation(i).unwrap();
            let value = base + (self.value(offset, width) - self.linked_base);
            arena[offset..offset + width].copy_from_slice(&value.to_le_bytes()[..width]);
        }
        let header = &mut arena[HANDOFF_OFFSET..HANDOFF_OFFSET + 48];
        header[..8].copy_from_slice(b"SVMUEFI2");
        for (index, value) in [
            base,
            ARENA_BYTES as u64,
            self.image_bytes as u64,
            self.memory_bytes as u64,
            self.entry_offset as u64,
        ]
        .iter()
        .enumerate()
        {
            header[8 + index * 8..16 + index * 8].copy_from_slice(&value.to_le_bytes());
        }
        Ok(())
    }
}
