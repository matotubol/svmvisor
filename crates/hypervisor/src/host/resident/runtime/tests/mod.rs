//! Unit tests of the resident runtime's private behavior.

#[cfg(test)]
mod raw_capture;
#[cfg(all(test, not(feature = "resident-runtime-test")))]
mod terminal_return;
/// Host models of the x2AVIC exit glue. Hardware effects go through the
/// owners' `PhysicalX2Apic` seam; nothing here reaches `stop` or `debug`.
#[cfg(test)]
mod x2avic_glue;
