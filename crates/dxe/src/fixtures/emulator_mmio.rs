//! QEMU pci-testdev BAR0 diagnostic, compiled only for the PCI emulator mode.
//! This is the device's no-eventfd test, not an FPGA journal implementation.
use crate::pci_io::Bar0;
use svmvisor_dxe::journal::JournalIo;
use uefi_raw::Status;

pub fn verify(io: &mut Bar0) -> Result<(), Status> {
    // QEMU splits these DWORD accesses into its byte-wide register operations.
    // Select test zero, then verify its header before using the reported port.
    io.write(0, 0)?;
    if io.read(0)? != 0x100 || io.read(4)? != 0x800 || io.read(8)? != 0xfa || io.read(12)? != 0 {
        return Err(Status::DEVICE_ERROR);
    }
    // The no-eventfd mode increments only for a matching byte at offset 0x800.
    io.write(0x800, 0x55)?;
    if io.read(12)? != 0 {
        return Err(Status::DEVICE_ERROR);
    }
    for expected in 1..=16 {
        io.write(0x800, 0xfa)?;
        if io.read(12)? != expected {
            return Err(Status::DEVICE_ERROR);
        }
    }
    io.write(0x800, 0xfb)?;
    io.write(0x804, 0xfa)?;
    if io.read(12)? != 16 {
        return Err(Status::DEVICE_ERROR);
    }
    io.write(0, 0xff)?;
    io.write(0x800, 0xfa)?;
    if io.read(0)? != 0 || io.read(12)? != 0 {
        return Err(Status::DEVICE_ERROR);
    }
    // Reset the test before returning, including the intentional rejection case.
    io.write(0, 0)?;
    if io.read(12)? != 0 {
        return Err(Status::DEVICE_ERROR);
    }
    if cfg!(feature = "emulator-reject-mmio") {
        svmvisor_firmware_handoff::debug("pci-mmio-rejected-for-test\n");
        return Err(Status::DEVICE_ERROR);
    }
    svmvisor_firmware_handoff::debug("PASS pci-mmio-count=16 reset=0\n");
    Ok(())
}
