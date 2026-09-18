#![cfg(feature = "native-preflight")]

// Compile the finite observation logic on the host. Native pointer accesses
// and privileged capture remain UEFI-only. Actual efiapi lookup/early refusal
// tests use BootServices stubs and must stop before privileged initialization.
#[allow(dead_code, unused_imports)]
#[path = "../src/native/resources/tables/mod.rs"]
mod native_tables;
