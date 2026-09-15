//! Physical ranges for the initial unencrypted AMD64 policy. These checks do
//! not establish allocation ownership, WB caching, mappings, or DMA isolation.
//! Width limit: pinned AMD APM vol. 2 rev. 3.44, chapter 5 (52-bit maximum).

/// Caller-supplied platform evidence; absence of evidence is not disabled SME.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EncryptionState {
    Unknown,
    Active,
    /// Addresses must not encode this bit, even when encryption is disabled.
    /// `None` asserts that the platform has no encryption address bit.
    Unencrypted {
        encryption_bit: Option<u8>,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AddressError {
    UnsupportedPhysicalWidth,
    UnknownEncryption,
    ActiveEncryptionUnsupported,
    InvalidEncryptionBit,
    EmptyRange,
    InvalidAlignment,
    Misaligned,
    Overflow,
    OutsidePhysicalWidth,
    EncryptionBitEncoded,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PhysicalRange {
    base: u64,
    len: u64,
}

impl PhysicalRange {
    pub const fn base(self) -> u64 {
        self.base
    }
    pub const fn len(self) -> u64 {
        self.len
    }
    pub const fn is_empty(self) -> bool {
        false
    }
    pub const fn last_byte(self) -> u64 {
        self.base + (self.len - 1)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AddressPolicy {
    physical_bits: u8,
    encryption_bit: Option<u8>,
}

impl AddressPolicy {
    /// The initial x64 implementation accepts physical widths 32 through 52.
    pub fn new(physical_bits: u8, encryption: EncryptionState) -> Result<Self, AddressError> {
        if !(32..=52).contains(&physical_bits) {
            return Err(AddressError::UnsupportedPhysicalWidth);
        }
        let encryption_bit = match encryption {
            EncryptionState::Unknown => return Err(AddressError::UnknownEncryption),
            EncryptionState::Active => return Err(AddressError::ActiveEncryptionUnsupported),
            EncryptionState::Unencrypted { encryption_bit } => encryption_bit,
        };
        if encryption_bit.is_some_and(|bit| bit >= 64) {
            return Err(AddressError::InvalidEncryptionBit);
        }
        Ok(Self {
            physical_bits,
            encryption_bit,
        })
    }

    pub const fn physical_bits(self) -> u8 {
        self.physical_bits
    }

    /// Validate every byte, including the inclusive last byte. Alignment is a
    /// nonzero power of two and applies to the base, not the range length.
    pub fn validate(
        self,
        base: u64,
        len: u64,
        alignment: u64,
    ) -> Result<PhysicalRange, AddressError> {
        if len == 0 {
            return Err(AddressError::EmptyRange);
        }
        if !alignment.is_power_of_two() {
            return Err(AddressError::InvalidAlignment);
        }
        if base & (alignment - 1) != 0 {
            return Err(AddressError::Misaligned);
        }
        let last = base.checked_add(len - 1).ok_or(AddressError::Overflow)?;
        if last >> self.physical_bits != 0 {
            return Err(AddressError::OutsidePhysicalWidth);
        }
        if let Some(bit) = self.encryption_bit {
            // A range cannot start with the bit set or cross its next toggle.
            // Checking only endpoint bits would miss a full intervening cycle.
            if base & (1u64 << bit) != 0 || base >> bit != last >> bit {
                return Err(AddressError::EncryptionBitEncoded);
            }
        }
        Ok(PhysicalRange { base, len })
    }
}

/// Numeric canonicality for the deliberately restricted four-level profile.
/// Whether an address is canonical for four-level x86-64 linear addressing.
pub const fn is_canonical_48(address: u64) -> bool {
    (((address << 16) as i64 >> 16) as u64) == address
}
