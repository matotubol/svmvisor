//! Inert card payload format. Integrity identifies a reviewed package; it is
//! not signature verification or authorization to execute its contents.
use sha2::{Digest, Sha256};
use svmvisor_firmware_handoff::layout::{LayoutError, Payload};

pub const HEADER_BYTES: usize = 128;
pub const SLOT_BYTES: usize = 0x100000;
pub const JOURNAL_SUCCESS: u32 = 0x00050010;
pub const JOURNAL_FAILURE: u32 = 0x0005001f;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CardError {
    Header,
    Bounds,
    Digest,
    Package(LayoutError),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Manifest {
    package_bytes: usize,
    digest: [u8; 32],
}

impl Manifest {
    pub const fn package_bytes(self) -> usize {
        self.package_bytes
    }
    pub fn parse(header: &[u8], pinned: &[u8; 32]) -> Result<Self, CardError> {
        if header.len() != HEADER_BYTES
            || &header[..8] != b"SVMCRD01"
            || &header[8..12] != 1u32.to_le_bytes().as_slice()
            || &header[12..16] != (HEADER_BYTES as u32).to_le_bytes().as_slice()
            || u64::from_le_bytes(header[24..32].try_into().unwrap()) != SLOT_BYTES as u64
            || u64::from_le_bytes(header[32..40].try_into().unwrap()) != HEADER_BYTES as u64
            || u64::from_le_bytes(header[40..48].try_into().unwrap()) != 1
            || header[80..].iter().any(|&b| b != 0)
        {
            return Err(CardError::Header);
        }
        let size = u64::from_le_bytes(header[16..24].try_into().unwrap());
        if size < 64 || size > (SLOT_BYTES - HEADER_BYTES) as u64 {
            return Err(CardError::Bounds);
        }
        let digest: [u8; 32] = header[48..80].try_into().unwrap();
        if &digest != pinned {
            return Err(CardError::Digest);
        }
        Ok(Self { package_bytes: size as usize, digest })
    }

    pub fn package<'a>(&self, bytes: &'a [u8]) -> Result<Payload<'a>, CardError> {
        if bytes.len() != self.package_bytes || bytes.len() < 64 {
            return Err(CardError::Bounds);
        }
        if Sha256::digest(bytes).as_slice() != self.digest {
            return Err(CardError::Digest);
        }
        let entry = u64::from_le_bytes(bytes[40..48].try_into().unwrap());
        let entry = usize::try_from(entry).map_err(|_| CardError::Bounds)?;
        Payload::parse(bytes, entry).map_err(CardError::Package)
    }
}

/// Decode the build's exact digest. Missing/malformed pins are never accepted.
pub fn parse_pin(text: &str) -> Result<[u8; 32], CardError> {
    if text.len() != 64 {
        return Err(CardError::Digest);
    }
    let mut out = [0; 32];
    for (i, pair) in text.as_bytes().chunks_exact(2).enumerate() {
        fn nibble(b: u8) -> Result<u8, CardError> {
            match b {
                b'0'..=b'9' => Ok(b - b'0'),
                b'a'..=b'f' => Ok(b - b'a' + 10),
                b'A'..=b'F' => Ok(b - b'A' + 10),
                _ => Err(CardError::Digest),
            }
        }
        out[i] = (nibble(pair[0])? << 4) | nibble(pair[1])?;
    }
    Ok(out)
}
