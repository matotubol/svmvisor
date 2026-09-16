//! Scalar PCI configuration I/O for the diagnostic endpoint lifetime owner.
//! APM2 rev3.44 15.10.1-.3/Figure15-2: hardware supplies scalar width,
//! direction, port and following RIP. No instruction-memory read is required.
//! No hardware operation occurs here. The runtime serializes selector/data
//! access with live publication, revokes publication before configuration data writes, and
//! then faithfully executes the admitted native IN/OUT. Unsupported encodings
//! are explicit stopped refusals, never fabricated CPU exceptions.
use super::{events::ExternalInterruptError, exit::{IoDecodeError, IoIntercept, IoWidth,
    ResumeCandidate, ResumeError}, vmcb::Vmcb};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConfigError {
    Decode(IoDecodeError), UnsupportedMode, StringOrRep, PortOrWidth,
    Pending(ExternalInterruptError), Continuation(ResumeError),
}

pub struct PreparedIo<'a> {
    vmcb: &'a mut Vmcb,
    next: ResumeCandidate,
    io: IoIntercept,
    revoke: bool,
}
impl PreparedIo<'_> {
    pub fn port(&self) -> u16 { self.io.port() }
    pub fn width(&self) -> IoWidth { self.io.width() }
    pub fn width_bytes(&self) -> u8 { self.io.width_bytes() }
    pub fn input(&self) -> bool { self.io.input() }
    pub fn revoke(&self) -> bool { self.revoke }
    pub fn output_value(&self) -> u32 { self.vmcb.guest_rax() as u32 }
    /// PPR57896 rev3.00 p203 HWCR.IoCfgGpFault: scalar accesses touching
    /// CF8..CFF fault when set. Caller sampled that bit on the owning CPU;
    /// no physical port access or publication revocation may precede this.
    pub fn fault_if_disabled(self) -> Result<(), ExternalInterruptError> {
        self.vmcb.queue_native_general_protection()
    }
    /// Only after the admitted hardware operation completed. OUT preserves all
    /// GPRs; IN AL/AX preserves higher bits, IN EAX clears the upper32 bits.
    pub fn commit(self, input_value: u32) {
        let old = self.vmcb.guest_rax();
        let rax = if !self.io.input() { old } else { match self.io.width() {
            IoWidth::Byte => (old & !0xff) | u64::from(input_value & 0xff),
            IoWidth::Word => (old & !0xffff) | u64::from(input_value & 0xffff),
            IoWidth::Dword => u64::from(input_value),
        }};
        self.vmcb.commit_emulated_instruction(rax, self.next);
        self.vmcb.complete_native_instruction_state();
    }
}

/// Prepare an actual stopped native IOIO operation. Dropping the token leaves
/// the entire VMCB unchanged. Supported scope: CPL0 long64 scalar B/W/D port
/// accesses overlapping the intercepted CF8..CFF range, including CF9 reset.
/// APM2 15.10.2 checks every operand byte; an access may start before the range.
/// Forward the original port/width, with no assumed chipset register semantics.
/// Revoke publication before every OUT except the standard CF8 DWORD selector.
/// The caller must check HWCR.IoCfgGpFault before any native I/O.
pub fn prepare_io(vmcb: &mut Vmcb)
    -> Result<PreparedIo<'_>, ConfigError>
{
    let io = vmcb.exit_snapshot().ioio().map_err(ConfigError::Decode)?;
    if !vmcb.guest_in_64_bit_code() || vmcb.bytes()[0x4cb] != 0
        || vmcb.guest_rflags() & ((1 << 8) | (1 << 17)) != 0 {
        return Err(ConfigError::UnsupportedMode);
    }
    if io.string() || io.rep() { return Err(ConfigError::StringOrRep); }
    let width = u16::from(io.width_bytes());
    let selector_port = io.port() == 0xcf8 && width == 4;
    let overlaps = io.port() <= 0xcff && io.last_port().is_some_and(|p| p >= 0xcf8);
    if !overlaps { return Err(ConfigError::PortOrWidth); }
    vmcb.validate_external_interrupt_conflicts().map_err(ConfigError::Pending)?;
    vmcb.validate_virtual_interrupt_controls().map_err(ConfigError::Pending)?;
    let next = vmcb.exit_snapshot().ioio_continuation().map_err(ConfigError::Continuation)?;
    let revoke = !selector_port && !io.input();
    Ok(PreparedIo { vmcb, next, io, revoke })
}
