use svmvisor_hypervisor::{host::resident::terminal,
    svm::ipi::{NativeRouteFailure, NativeRouteRecipient, NativeRoutePredicate as P,
        NativeDestinationMode as M, NativeDestinationCause as H}};

#[test]
fn refusal_wire_preserves_evidence_and_exports_decoder_vectors() {
    let mut vectors = Vec::new();
    for x2 in [false, true] {
        for count in [0, 1, 30, 31, 32, u32::MAX] {
            let f = NativeRouteFailure { value: 0x10_0000_c500, source: 8, predicate: P::ForeignMatch,
                recipient: Some(NativeRouteRecipient { identity: 32, mode: Some(M::ExtendedXApic4),
                    init_count: count, cause: H::GuestInit }) };
            let (tag, v) = terminal::route_failure(f, x2);
            let exit = if x2 { 0x7c } else { 0x400 };
            let w = terminal::stop_words(3, exit, 0, tag, v).unwrap();
            assert_eq!((w[0] >> 24) & 15, 14);
            assert_eq!((w[0] >> 19) & 31, count.min(31));
            assert_eq!(v, 0x1220_0810_0000_c500);
            assert!(terminal::stop_words(3, if x2 {0x400} else {0x7c}, 0, tag, v).is_none());
            vectors.push(format!("{{\"name\":\"foreign\",\"words\":{:?},\"count\":{},\"x2\":{}}}", w, count.min(31), x2));
        }
    }
    for f in [
        NativeRouteFailure { value: 0xfedc_ba98_0000_c500, source: 8, predicate: P::DestinationUnassigned, recipient: None },
        NativeRouteFailure { value: 0x10_0000_c500, source: 256, predicate: P::ForeignMatch,
            recipient: Some(NativeRouteRecipient { identity: 256, mode: Some(M::X2Apic), init_count: 1, cause: H::GuestInit }) },
    ] {
        let (tag, v) = terminal::route_failure(f, true);
        assert_eq!(v, f.value);
        let w = terminal::stop_words(31, 0x7c, 0, tag, v).unwrap();
        assert_ne!(w[0] & (32 << 13), 0);
        vectors.push(format!("{{\"name\":\"wide\",\"words\":{:?},\"icr\":{}}}", w, v));
    }
    for value in [0x100, u32::MAX as u64, 0x1_0000_0000, u64::MAX] {
        let (tag, v) = terminal::startup_failure(4, 0x10, value, 16);
        let w = terminal::stop_words(2, 0x63, 0, tag, v).unwrap();
        assert_eq!((w[0] >> 24) & 15, 15);
        vectors.push(format!("{{\"name\":\"target\",\"words\":{:?},\"value\":{}}}", w, value));
    }
    // An old capture must retain its old interpretation.
    let (tag, v) = terminal::apic_context(143, 0x10_0000_c500);
    let w = terminal::stop_words(0, 0x400, 0, tag, v).unwrap();
    assert_eq!((w[0] >> 24) & 15, 13);
    vectors.push(format!("{{\"name\":\"legacy\",\"words\":{:?}}}", w));
    if let Some(path) = std::env::var_os("SVMVISOR_REFUSAL_VECTORS") {
        std::fs::write(path, format!("[{}]\n", vectors.join(",\n"))).unwrap();
    }
}
