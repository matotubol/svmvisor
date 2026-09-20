//! Exact loaded PE section extents for the separate current-permission walker.
//! PE characteristics request access; they never prove current permissions.

use uefi_raw::Status;
#[cfg(target_os = "uefi")]
use uefi_raw::{Handle, protocol::loaded_image::LoadedImageProtocol, table::boot::BootServices};

use crate::native_tables::{BorrowedAccess, BorrowedSpan};

pub const MAX_SPANS: usize = 24;

pub struct ImageSpans {
    spans: [BorrowedSpan; MAX_SPANS],
    count: usize,
}

impl ImageSpans {
    pub fn spans(&self) -> Result<&[BorrowedSpan], Status> {
        self.spans.get(..self.count).ok_or(Status::COMPROMISED_DATA)
    }

    pub fn push(&mut self, span: BorrowedSpan) -> Result<(), Status> {
        *self.spans.get_mut(self.count).ok_or(Status::OUT_OF_RESOURCES)? = span;
        self.count += 1;
        Ok(())
    }
}

/// The live firmware LoadedImage contract supplies its readable mapped PE
/// headers. No header-derived pointer is followed until bounded by that image.
/// The caller retains this image and all borrowed spans through restoration.
#[cfg(target_os = "uefi")]
pub unsafe fn collect(image: Handle, services: &BootServices) -> Result<ImageSpans, Status> {
    let mut raw = core::ptr::null_mut();
    let status = unsafe {
        (services.open_protocol)(
            image,
            &LoadedImageProtocol::GUID,
            &mut raw,
            image,
            core::ptr::null_mut(),
            2,
        )
    };
    if status != Status::SUCCESS {
        return Err(status);
    }
    let result = (|| {
        let loaded =
            unsafe { raw.cast::<LoadedImageProtocol>().as_ref() }.ok_or(Status::DEVICE_ERROR)?;
        if loaded.image_base.is_null() || loaded.image_size < 4096 {
            return Err(Status::COMPROMISED_DATA);
        }
        let header = unsafe { core::slice::from_raw_parts(loaded.image_base.cast::<u8>(), 4096) };
        parse(loaded.image_base as u64, loaded.image_size, header)
    })();
    let closed = unsafe {
        (services.close_protocol)(image, &LoadedImageProtocol::GUID, image, core::ptr::null_mut())
    };
    if closed != Status::SUCCESS {
        return Err(closed);
    }
    result
}

fn parse(base: u64, image_bytes: u64, header: &[u8]) -> Result<ImageSpans, Status> {
    let bad = Status::COMPROMISED_DATA;
    if base == 0
        || base & 4095 != 0
        || !(4096..=16 * 1024 * 1024).contains(&image_bytes)
        || image_bytes & 4095 != 0
        || base.checked_add(image_bytes).is_none()
        || u16_at(header, 0)? != 0x5a4d
    {
        return Err(bad);
    }
    let pe = u32_at(header, 0x3c)? as usize;
    if !(64..=1024).contains(&pe)
        || u32_at(header, pe)? != 0x4550
        || u16_at(header, pe + 4)? != 0x8664
        || u16_at(header, pe + 20)? != 240
    {
        return Err(bad);
    }
    let count = usize::from(u16_at(header, pe + 6)?);
    let optional = pe + 24;
    let headers = u32_at(header, optional + 60)? as usize;
    if count == 0
        || count > 16
        || headers == 0
        || headers > 4096
        || optional + 240 + count * 40 > headers
        || header.len() < headers
        || u16_at(header, optional)? != 0x20b
        || u16_at(header, optional + 68)? != 11
        || u32_at(header, optional + 32)? != 4096
        || u64::from(u32_at(header, optional + 56)?) != image_bytes
    {
        return Err(bad);
    }
    let mut result = ImageSpans {
        spans: [BorrowedSpan { base: 0, bytes: 0, access: BorrowedAccess::Read }; MAX_SPANS],
        count: 0,
    };
    result.push(BorrowedSpan { base, bytes: headers as u64, access: BorrowedAccess::Read })?;
    let entry = u64::from(u32_at(header, optional + 16)?);
    let mut entry_covered = false;
    let mut previous_end = 4096u64;
    for section in 0..count {
        let offset = optional + 240 + section * 40;
        let virtual_bytes = u64::from(u32_at(header, offset + 8)?);
        let start = u64::from(u32_at(header, offset + 12)?);
        let bytes = virtual_bytes.max(u64::from(u32_at(header, offset + 16)?));
        let characteristics = u32_at(header, offset + 36)?;
        let end = start.checked_add(bytes).ok_or(bad)?;
        if bytes == 0
            || start & 4095 != 0
            || start < previous_end
            || end > image_bytes
            || characteristics & 0x40000000 == 0
            || characteristics & 0xa0000000 == 0xa0000000
        {
            return Err(bad);
        }
        let access = if characteristics & 0x20000000 != 0 {
            BorrowedAccess::ReadExecute
        } else if characteristics & 0x80000000 != 0 {
            BorrowedAccess::ReadWrite
        } else {
            BorrowedAccess::Read
        };
        if start <= entry && entry < end {
            if access != BorrowedAccess::ReadExecute || entry - start >= virtual_bytes {
                return Err(bad);
            }
            entry_covered = true;
        }
        result.push(BorrowedSpan { base: base + start, bytes, access })?;
        previous_end = end;
    }
    if !entry_covered {
        return Err(bad);
    }
    Ok(result)
}

fn u16_at(bytes: &[u8], offset: usize) -> Result<u16, Status> {
    let end = offset.checked_add(2).ok_or(Status::COMPROMISED_DATA)?;
    let &[a, b] = bytes.get(offset..end).ok_or(Status::COMPROMISED_DATA)? else {
        return Err(Status::COMPROMISED_DATA);
    };
    Ok(u16::from_le_bytes([a, b]))
}

fn u32_at(bytes: &[u8], offset: usize) -> Result<u32, Status> {
    let end = offset.checked_add(4).ok_or(Status::COMPROMISED_DATA)?;
    let &[a, b, c, d] = bytes.get(offset..end).ok_or(Status::COMPROMISED_DATA)? else {
        return Err(Status::COMPROMISED_DATA);
    };
    Ok(u32::from_le_bytes([a, b, c, d]))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn header() -> [u8; 512] {
        let mut bytes = [0; 512];
        for (offset, value) in
            [(0, 0x5a4du16), (0x84, 0x8664), (0x86, 2), (0x94, 240), (0x98, 0x20b), (0xdc, 11)]
        {
            bytes[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
        }
        for (offset, value) in [
            (0x3c, 0x80u32),
            (0x80, 0x4550),
            (0xa8, 0x1100),
            (0xb8, 4096),
            (0xd0, 0x4000),
            (0xd4, 512),
            (0x190, 0x1300),
            (0x194, 0x1000),
            (0x198, 0x1400),
            (0x1ac, 0x60000020),
            (0x1b8, 0x280),
            (0x1bc, 0x3000),
            (0x1c0, 0x400),
            (0x1d4, 0xc0000040),
        ] {
            bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
        }
        bytes
    }
    #[test]
    fn complete_image_sections_request_distinct_access_without_claiming_permissions() {
        let spans = parse(0x200000, 0x4000, &header()).unwrap();
        assert_eq!(
            spans.spans().unwrap(),
            &[
                BorrowedSpan { base: 0x200000, bytes: 512, access: BorrowedAccess::Read },
                BorrowedSpan { base: 0x201000, bytes: 0x1400, access: BorrowedAccess::ReadExecute },
                BorrowedSpan { base: 0x203000, bytes: 0x400, access: BorrowedAccess::ReadWrite },
            ]
        );
    }
    #[test]
    fn malformed_or_uncovered_image_operands_are_not_retained() {
        for (offset, value) in [
            (0x3c, 0xffff_ffffu32),
            (0x1ac, 0xe0000020),
            (0x1bc, 0x2000),
            (0x1b8, 0x2000),
            (0xa8, 0x3500),
            (0x1d4, 0x80000040),
            (0xd4, 4097),
        ] {
            let mut bytes = header();
            bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
            assert!(parse(0x200000, 0x4000, &bytes).is_err(), "offset {offset:x}");
        }
    }
}
