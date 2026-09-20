//! Bounded, inert four-level nested page tables: the resident `IdentityNpt`
//! (RWX identity with one excluded pool) and its stopped-CPU low-memory copy
//! `LowMemoryNptStorage`.
//!
//! AMD APM vol. 2 rev. 3.44 sections 5.3.5, 5.4 and 15.25: every present
//! entry has U/S=1 (15.25.5 treats every nested access as a user access),
//! parent entries permit writes/execution, leaf entries restrict them, and
//! 1-GByte/2-MByte leaves keep bits 29:13 / 20:13 and the bit-12 PAT zero.
//! NX requires supported NX and host EFER.NXE=1 (15.25.7). The caller supplies
//! evidence of host four-level long mode, because the nested walk uses the
//! paging mode the host had at VMRUN (15.25.3); GMET, SSS and encrypted modes
//! must be disabled. PWT/PCD/PAT are zero, so the nested type is entry 0 of
//! the *host* PAT register at VMRUN (15.25.8, Table 15-19). Guest PAT writes
//! reach only the replicated guest copy (15.25.2), so the host PAT captured at
//! arm stays the nested PAT; index zero alone does not establish WB memory.
//! Allocation, actual copying to assigned physical addresses, ownership, PAT/
//! MTRR validation, guest state, TLB invalidation and enabling NPT are external.
//! No builder here may edit tables used by a running CPU.

pub use crate::memory::npt::{
    identity::{
        IdentityNpt, IdentityNptError, IdentityTranslation, LowMemoryNptStorage,
        identity_protection_range, restore_identity_write_range,
    },
    table::{NptEvidence, TABLE_COUNT, TableStorage, TableView},
};

mod identity;
mod table;
