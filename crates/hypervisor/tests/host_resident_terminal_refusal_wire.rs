use svmvisor_hypervisor::host::resident::terminal;

/// Startup-target records through the three-DWORD export; the retired
/// startup-route record (kind 14) is never exported, so its vectors are gone.
#[test]
fn refusal_wire_preserves_evidence_and_exports_decoder_vectors() {
    let mut vectors = Vec::new();
    for value in [0x100, u32::MAX as u64, 0x1_0000_0000, u64::MAX] {
        let (tag, v) = terminal::startup_failure(4, 0x10, value, 16);
        let w = terminal::stop_words(2, 0x63, 0, tag, v).unwrap();
        assert_eq!((w[0] >> 24) & 15, 15);
        vectors.push(format!("{{\"name\":\"target\",\"words\":{:?},\"value\":{}}}", w, value));
    }
    // A startup-router refusal exports as an unhandled exit with the guest
    // RIP; its route evidence stays in the stop record.
    let (tag, v) = terminal::startup_route_refusal(None, 0x10_0000_0500);
    let w = terminal::stop_words(3, 0x401, 0xffff_f800_0000_1000, tag, v).unwrap();
    assert_eq!(
        ((w[0] >> 24) & 15, (w[0] >> 13) & 0x7ff, w[1], w[2]),
        (0, 0x401, 0x1000, 0xffff_f800)
    );
    // A stalled physical-NMI drain (F113h, `info2` = consecutive misses) is
    // likewise an unhandled exit 61h with the guest RIP.
    let w = terminal::stop_words(3, 0x61, 0xffff_f800_0000_1000, 0xf113, 3).unwrap();
    assert_eq!(
        ((w[0] >> 24) & 15, (w[0] >> 13) & 0x7ff, w[1], w[2]),
        (0, 0x61, 0x1000, 0xffff_f800)
    );
    if let Some(path) = std::env::var_os("SVMVISOR_REFUSAL_VECTORS") {
        std::fs::write(path, format!("[{}]\n", vectors.join(",\n"))).unwrap();
    }
}
