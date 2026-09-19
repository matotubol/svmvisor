//! Inert card payload format. Integrity identifies a reviewed package; it is
//! not signature verification or authorization to execute its contents.

use sha2::{Digest, Sha256};
use svmvisor_card_abi::{
    envelope::{
        DIGEST_BYTES, DIGEST_OFFSET, FLAGS_OFFSET, FLAGS_PACKAGE, HEADER_BYTES,
        HEADER_BYTES_OFFSET, MIN_PACKAGE_BYTES, PACKAGE_MAGIC, PACKAGE_RESERVED_OFFSET,
        PAYLOAD_BYTES_OFFSET, PAYLOAD_OFFSET_OFFSET, SLOT_BYTES, SLOT_BYTES_OFFSET, VERSION,
        VERSION_OFFSET,
    },
    package::{Package, PackageError},
};

pub const JOURNAL_SUCCESS: u32 = 0x00050010;
pub const JOURNAL_FAILURE: u32 = 0x0005001f;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Manifest {
    package_bytes: usize,
    digest: [u8; 32],
}

impl Manifest {
    pub fn parse(header: &[u8], pinned: &[u8; 32]) -> Result<Self, CardError> {
        if header.len() != HEADER_BYTES
            || &header[..8] != &PACKAGE_MAGIC
            || &header[VERSION_OFFSET..VERSION_OFFSET + 4] != VERSION.to_le_bytes().as_slice()
            || &header[HEADER_BYTES_OFFSET..HEADER_BYTES_OFFSET + 4]
                != (HEADER_BYTES as u32).to_le_bytes().as_slice()
            || u64::from_le_bytes(
                header[SLOT_BYTES_OFFSET..SLOT_BYTES_OFFSET + 8].try_into().unwrap(),
            ) != SLOT_BYTES as u64
            || u64::from_le_bytes(
                header[PAYLOAD_OFFSET_OFFSET..PAYLOAD_OFFSET_OFFSET + 8].try_into().unwrap(),
            ) != HEADER_BYTES as u64
            || u64::from_le_bytes(header[FLAGS_OFFSET..FLAGS_OFFSET + 8].try_into().unwrap())
                != FLAGS_PACKAGE
            || header[PACKAGE_RESERVED_OFFSET..].iter().any(|&b| b != 0)
        {
            return Err(CardError::Header);
        }
        let size = u64::from_le_bytes(
            header[PAYLOAD_BYTES_OFFSET..PAYLOAD_BYTES_OFFSET + 8].try_into().unwrap(),
        );
        if size < MIN_PACKAGE_BYTES as u64 || size > (SLOT_BYTES - HEADER_BYTES) as u64 {
            return Err(CardError::Bounds);
        }
        let digest: [u8; DIGEST_BYTES] =
            header[DIGEST_OFFSET..DIGEST_OFFSET + DIGEST_BYTES].try_into().unwrap();
        if &digest != pinned {
            return Err(CardError::Digest);
        }
        Ok(Self { package_bytes: size as usize, digest })
    }

    pub const fn package_bytes(self) -> usize {
        self.package_bytes
    }

    pub fn package<'a>(&self, bytes: &'a [u8]) -> Result<Package<'a>, CardError> {
        if bytes.len() != self.package_bytes || bytes.len() < MIN_PACKAGE_BYTES {
            return Err(CardError::Bounds);
        }
        if Sha256::digest(bytes).as_slice() != self.digest {
            return Err(CardError::Digest);
        }
        let entry = u64::from_le_bytes(bytes[40..48].try_into().unwrap());
        let entry = usize::try_from(entry).map_err(|_| CardError::Bounds)?;
        Package::parse(bytes, entry).map_err(CardError::Package)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CardError {
    Header,
    Bounds,
    Digest,
    Package(PackageError),
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
