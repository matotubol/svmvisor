//! Private host descriptor images for a future long-mode execution harness.
//!
//! AMD APM vol. 2 rev. 3.44 sections 4.8.3–4.8.4 and 12.2.2/12.2.5 define
//! these layouts. The TSS is available (type 9) for a future LTR; loading it
//! requires writable GDT backing because LTR marks it busy. CET must be off.
//! Every vector is a ring-0 interrupt gate; the default profile gives only double fault IST1. The native terminal profile
//! gives every non-returning gate IST1 and leaves the private #SX gate on IST0.
//! No handler instructions, memory backing, mappings, stack space, table loads,
//! fault recovery or hardware access are supplied or established by this module.

use crate::{
    arch::x86_64::descriptors::{CODE_SELECTOR, DATA_SELECTOR, GDT_BYTES, TSS_BYTES, TSS_SELECTOR},
    memory::address::is_canonical_48,
};

pub const IDT_ENTRIES: usize = 256;
pub const IDT_GATE_BYTES: usize = 16;
pub const IDT_BYTES: usize = IDT_ENTRIES * IDT_GATE_BYTES;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostDescriptorRequest {
    pub gdt_base: u64,
    pub tss_base: u64,
    pub idt_base: u64,
    pub rsp0: u64,
    pub ist1: u64,
    /// IST2 stack top for the returning host NMI gate (vector 2). Its own
    /// stack keeps a platform NMI from corrupting a partly-entered terminal
    /// fault frame on IST1 (APM2 rev3.44 8.9.4; 15.21.10 p536 re-presents it).
    pub ist2: u64,
    pub handlers: [u64; IDT_ENTRIES],
}

impl HostDescriptorRequest {
    /// Native non-returning exception profile. The caller must load this TSS
    /// before LIDT, provide retained IST backing, and prevent recovery through
    /// a nested fault. Vector30 is the separately owned returning #SX gate.
    pub fn validate_terminal_ist(self) -> Result<ValidatedHostDescriptors, HostDescriptorError> {
        let mut image = self.validate()?;
        for (vector, gate) in image.idt.chunks_exact_mut(IDT_GATE_BYTES).enumerate() {
            // Every non-returning gate uses terminal IST1; the returning #SX
            // gate (30) keeps IST0; the returning NMI gate (2) uses IST2.
            gate[4] = match vector {
                2 => 2,
                30 => 0,
                _ => 1,
            };
        }
        Ok(image)
    }

    pub fn validate(self) -> Result<ValidatedHostDescriptors, HostDescriptorError> {
        let gdt_last =
            canonical_last(self.gdt_base, GDT_BYTES).ok_or(HostDescriptorError::InvalidGdtRange)?;
        let tss_last =
            canonical_last(self.tss_base, TSS_BYTES).ok_or(HostDescriptorError::InvalidTssRange)?;
        let idt_last =
            canonical_last(self.idt_base, IDT_BYTES).ok_or(HostDescriptorError::InvalidIdtRange)?;
        let ranges =
            [(self.gdt_base, gdt_last), (self.tss_base, tss_last), (self.idt_base, idt_last)];
        for i in 0..ranges.len() {
            for j in i + 1..ranges.len() {
                if ranges[i].0 <= ranges[j].1 && ranges[j].0 <= ranges[i].1 {
                    return Err(HostDescriptorError::OverlappingTables);
                }
            }
        }
        if !valid_pointer(self.rsp0) {
            return Err(HostDescriptorError::InvalidRsp0);
        }
        if !valid_pointer(self.ist1) {
            return Err(HostDescriptorError::InvalidIst1);
        }
        if !valid_pointer(self.ist2) {
            return Err(HostDescriptorError::InvalidIst2);
        }
        for (vector, handler) in self.handlers.iter().enumerate() {
            if !valid_pointer(*handler) {
                return Err(HostDescriptorError::InvalidHandler { vector: vector as u8 });
            }
        }
        let mut gdt = [0; GDT_BYTES];
        gdt[8..16].copy_from_slice(&0x00af_9b00_0000_ffff_u64.to_le_bytes());
        gdt[16..24].copy_from_slice(&0x00cf_9300_0000_ffff_u64.to_le_bytes());
        let base = self.tss_base;
        let low = 103
            | ((base & 0xffff) << 16)
            | (((base >> 16) & 0xff) << 32)
            | (0x89_u64 << 40)
            | (((base >> 24) & 0xff) << 56);
        gdt[24..32].copy_from_slice(&low.to_le_bytes());
        gdt[32..36].copy_from_slice(&((base >> 32) as u32).to_le_bytes());
        let mut tss = [0; TSS_BYTES];
        tss[4..12].copy_from_slice(&self.rsp0.to_le_bytes());
        // TSS IST1 at offset 36, IST2 at offset 44 (APM2 rev3.44 12.2.5).
        tss[36..44].copy_from_slice(&self.ist1.to_le_bytes());
        tss[44..52].copy_from_slice(&self.ist2.to_le_bytes());
        // No TSS I/O bitmap. This does not deny ring-0 I/O.
        tss[102..104].copy_from_slice(&(TSS_BYTES as u16).to_le_bytes());
        let mut idt = [0; IDT_BYTES];
        for (vector, gate) in idt.chunks_exact_mut(IDT_GATE_BYTES).enumerate() {
            let handler = self.handlers[vector];
            gate[0..2].copy_from_slice(&(handler as u16).to_le_bytes());
            gate[2..4].copy_from_slice(&CODE_SELECTOR.to_le_bytes());
            gate[4] = u8::from(vector == 8);
            gate[5] = 0x8e;
            gate[6..8].copy_from_slice(&((handler >> 16) as u16).to_le_bytes());
            gate[8..12].copy_from_slice(&((handler >> 32) as u32).to_le_bytes());
        }
        Ok(ValidatedHostDescriptors {
            gdtr: HostTablePointer { base: self.gdt_base, limit: GDT_BYTES as u16 - 1 },
            idtr: HostTablePointer { base: self.idt_base, limit: IDT_BYTES as u16 - 1 },
            gdt,
            tss,
            idt,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ValidatedHostDescriptors {
    gdtr: HostTablePointer,
    idtr: HostTablePointer,
    gdt: [u8; GDT_BYTES],
    tss: [u8; TSS_BYTES],
    idt: [u8; IDT_BYTES],
}

impl ValidatedHostDescriptors {
    pub const fn gdt(&self) -> &[u8; GDT_BYTES] {
        &self.gdt
    }
    pub const fn tss(&self) -> &[u8; TSS_BYTES] {
        &self.tss
    }
    pub const fn idt(&self) -> &[u8; IDT_BYTES] {
        &self.idt
    }
    pub const fn gdtr(&self) -> HostTablePointer {
        self.gdtr
    }
    pub const fn idtr(&self) -> HostTablePointer {
        self.idtr
    }
    pub const fn code_selector(&self) -> u16 {
        CODE_SELECTOR
    }
    pub const fn data_selector(&self) -> u16 {
        DATA_SELECTOR
    }
    pub const fn tss_selector(&self) -> u16 {
        TSS_SELECTOR
    }
}

/// Semantic table pointer; this Rust structure is not an LGDT/LIDT operand.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HostTablePointer {
    pub base: u64,
    pub limit: u16,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HostDescriptorError {
    InvalidGdtRange,
    InvalidTssRange,
    InvalidIdtRange,
    OverlappingTables,
    InvalidRsp0,
    InvalidIst1,
    InvalidIst2,
    InvalidHandler { vector: u8 },
}

fn canonical_last(base: u64, len: usize) -> Option<u64> {
    let last = base.checked_add(len as u64 - 1)?;
    (is_canonical_48(base) && is_canonical_48(last)).then_some(last)
}

fn valid_pointer(pointer: u64) -> bool {
    pointer != 0 && is_canonical_48(pointer)
}
