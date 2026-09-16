//! Pure parsing of captured firmware GDT state for a CPL0 64-bit return.
//!
//! APM vol.2 rev.3.44 sections 4.5/4.8 and 15.6: VMEXIT can reload host
//! CS/SS/DS/ES from a present, writable GDT while guest LDTR remains active.
//! This parser proves properties of supplied bytes only. It neither captures
//! hardware nor proves that active hidden segment state matches those bytes.
//! CPU ownership, stable tables, mapping capture and hidden-state correspondence
//! remain explicit adapter obligations. No descriptor is rewritten or loaded.
use crate::memory::address::is_canonical_48;
use crate::arch::x86_64::descriptors::SegmentState;
use crate::host::descriptors::HostTablePointer;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FirmwareSegment {
    Cs,
    Ss,
    Ds,
    Es,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FirmwareSelectors {
    pub cs: u16,
    pub ss: u16,
    pub ds: u16,
    pub es: u16,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FirmwareDescriptorError {
    NonCanonicalTable,
    WrongCaptureLength,
    LdtSelector(FirmwareSegment),
    NullCode,
    SelectorOutsideTable(FirmwareSegment),
    NotPresent(FirmwareSegment),
    SystemDescriptor(FirmwareSegment),
    InvalidCode,
    InvalidStack,
    InvalidData(FirmwareSegment),
    MappingCoverage,
    MappingNotPresent,
    MappingNotWritable,
}

/// A null selector has no descriptor and must not be decoded from GDT entry0.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CapturedSegment {
    Null { selector: u16 },
    Descriptor { raw: u64, decoded: SegmentState },
}

/// Bounds of the captured GDT, not proof that it is mapped or stable.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GdtRange {
    pub first: u64,
    pub last: u64,
}

/// Effective host translation permissions for one linear 4KiB page, supplied
/// by the adapter's page-table walk. For large pages it must expand coverage
/// into these units. Writable means the AND of all paging-level RW bits; do not
/// substitute CR0.WP=0 or a successful read for writable mapping evidence.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CapturedGdtPage {
    pub linear_page: u64,
    pub present: bool,
    pub writable: bool,
}

/// Parsed snapshot retaining exact original bytes for independent recapture.
/// This is not a native launch permission or a hidden segment-state snapshot.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParsedFirmwareGdt<'a> {
    table: HostTablePointer,
    selectors: FirmwareSelectors,
    bytes: &'a [u8],
    segments: [CapturedSegment; 4],
    range: GdtRange,
}

pub fn parse_firmware_gdt(
    table: HostTablePointer,
    selectors: FirmwareSelectors,
    bytes: &[u8],
) -> Result<ParsedFirmwareGdt<'_>, FirmwareDescriptorError> {
    use FirmwareDescriptorError as E;
    let last = table
        .base
        .checked_add(u64::from(table.limit))
        .ok_or(E::NonCanonicalTable)?;
    if !is_canonical_48(table.base) || !is_canonical_48(last) {
        return Err(E::NonCanonicalTable);
    }
    if bytes.len() != usize::from(table.limit) + 1 {
        return Err(E::WrongCaptureLength);
    }
    let mut segments = [CapturedSegment::Null { selector: 0 }; 4];
    for (index, (kind, selector)) in [
        (FirmwareSegment::Cs, selectors.cs),
        (FirmwareSegment::Ss, selectors.ss),
        (FirmwareSegment::Ds, selectors.ds),
        (FirmwareSegment::Es, selectors.es),
    ]
    .into_iter()
    .enumerate()
    {
        if selector & 4 != 0 {
            return Err(E::LdtSelector(kind));
        }
        let offset = usize::from(selector & !7);
        if offset == 0 {
            if kind == FirmwareSegment::Cs {
                return Err(E::NullCode);
            }
            // APM 4.5 permits null SS in 64-bit mode at CPL0, but its RPL
            // must match the restricted CPL0 return. DS/ES null RPL is inert.
            if kind == FirmwareSegment::Ss && selector & 3 != 0 {
                return Err(E::InvalidStack);
            }
            segments[index] = CapturedSegment::Null { selector };
            continue;
        }
        let raw_bytes: [u8; 8] = bytes
            .get(offset..offset + 8)
            .ok_or(E::SelectorOutsideTable(kind))?
            .try_into()
            .unwrap();
        let raw = u64::from_le_bytes(raw_bytes);
        let access = ((raw >> 40) & 0xff) as u8;
        let flags = ((raw >> 52) & 0xf) as u8;
        if access & 0x80 == 0 {
            return Err(E::NotPresent(kind));
        }
        if access & 0x10 == 0 {
            return Err(E::SystemDescriptor(kind));
        }
        let code = access & 8 != 0;
        let readable_or_writable = access & 2 != 0;
        let conforming = code && access & 4 != 0;
        let dpl = (access >> 5) & 3;
        let rpl = (selector & 3) as u8;
        match kind {
            FirmwareSegment::Cs => {
                if !code || rpl != 0 || dpl != 0 || flags & 6 != 2 {
                    return Err(E::InvalidCode);
                }
            }
            FirmwareSegment::Ss => {
                if code || !readable_or_writable || rpl != 0 || dpl != 0 || flags & 2 != 0 {
                    return Err(E::InvalidStack);
                }
            }
            FirmwareSegment::Ds | FirmwareSegment::Es => {
                if (code && (!readable_or_writable || flags & 6 == 6))
                    || (!code && flags & 2 != 0)
                    || (!conforming && rpl > dpl)
                    || (conforming && dpl != 0)
                {
                    return Err(E::InvalidData(kind));
                }
            }
        }
        let limit = ((raw & 0xffff) | ((raw >> 32) & 0xf0000)) as u32;
        let limit = if flags & 8 != 0 {
            (limit << 12) | 0xfff
        } else {
            limit
        };
        let base = ((raw >> 16) & 0xffff) | ((raw >> 32) & 0xff) << 16 | ((raw >> 56) & 0xff) << 24;
        segments[index] = CapturedSegment::Descriptor {
            raw,
            decoded: SegmentState {
                selector,
                attributes: u16::from(access) | (u16::from(flags) << 8),
                limit,
                base,
            },
        };
    }
    Ok(ParsedFirmwareGdt {
        table,
        selectors,
        bytes,
        segments,
        range: GdtRange {
            first: table.base,
            last,
        },
    })
}

impl ParsedFirmwareGdt<'_> {
    pub const fn table(&self) -> HostTablePointer {
        self.table
    }
    pub const fn selectors(&self) -> FirmwareSelectors {
        self.selectors
    }
    pub const fn required_mapping(&self) -> GdtRange {
        self.range
    }
    pub fn original_bytes(&self) -> &[u8] {
        self.bytes
    }
    pub const fn segments(&self) -> &[CapturedSegment; 4] {
        &self.segments
    }

    /// Validate separately supplied mapping observations for the full GDT.
    /// Exact ascending page coverage forbids gaps, duplicates and stale range
    /// substitution. The caller still authenticates the walk and CPU/CR3 lease.
    pub fn validate_mapping_capture(
        &self,
        pages: &[CapturedGdtPage],
    ) -> Result<(), FirmwareDescriptorError> {
        use FirmwareDescriptorError as E;
        let first = self.range.first & !4095;
        let last = self.range.last & !4095;
        let count = ((last - first) / 4096 + 1) as usize;
        if pages.len() != count {
            return Err(E::MappingCoverage);
        }
        for (index, page) in pages.iter().enumerate() {
            if page.linear_page != first + index as u64 * 4096 {
                return Err(E::MappingCoverage);
            }
            if !page.present {
                return Err(E::MappingNotPresent);
            }
            if !page.writable {
                return Err(E::MappingNotWritable);
            }
        }
        Ok(())
    }
}
