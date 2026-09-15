"""Read-only USER2 reader. Default offline mode never opens a USB device."""
import argparse
import json
from pathlib import Path
import re
import struct
import subprocess
import uuid
import zlib


def apic_failure_diagnostic(code, value):
    """Kind13: fixed NPF exit, explicit operand selection, no invented GPA/RIP."""
    simple = {
        1: ("preflight_apic_mode", "physical_apic_base"),
        2: ("physical_icr_busy", "physical_apic_base"),
        3: ("apic_page_not_uc", "apic_page"),
        4: ("unexpected_exit", "raw_exit_code"),
        5: ("nested_paging_control", "nested_paging_control"),
        6: ("guest_lma_clear", "guest_efer"),
        7: ("guest_cs_not_long", "guest_cs_attributes"),
        8: ("guest_not_cpl0", "guest_cpl"),
        9: ("guest_tf_or_vm", "guest_rflags"),
        10: ("unsupported_cr4_controls", "guest_cr4"),
        11: ("invalid_apic_page", "apic_page"),
        12: ("user_operand", "operand_linear_address"),
        13: ("operand_crosses_page", "operand_linear_address"),
        14: ("operand_write_protected", "operand_linear_address"),
        15: ("operand_pat_type", "guest_pat"),
        0x18: ("operand_apic_range_or_alignment", "translated_operand_gpa"),
        0x19: ("nested_fault_information", "raw_exit_info1"),
        0x1a: ("nested_fault_address_mismatch", "translated_operand_gpa"),
    }
    pending = ("reserved_vector", "invalid_task_priority", "request_not_queued",
        "request_not_armed", "request_already_consumed", "pending_injection",
        "nested_delivery_unsupported", "pending_virtual_interrupt", "unsupported_control",
        "unsupported_nested_control", "control_mismatch", "invalid_entry", "guest_shutdown",
        "inconsistent_virtual_interrupt_exit")
    instruction = ("exit_does_not_permit_candidate", "nrip_not_established", "noncanonical_rip",
        "noncanonical_nrip", "invalid_instruction_length", "unsupported_instruction_bytes")
    registers = ("invalid_topology", "unsupported_mode", "unsupported_debug_state", "unsupported_msr",
        "unsupported_register", "apic_base_change", "mailbox_busy", "mailbox_not_ready",
        "mailbox_mismatch", "routing_busy", "unsupported_startup_encoding", "overlay_not_enabled",
        "overlay_already_enabled", "running_startup", "assigned_startup", "unowned_startup")
    result = {"code": code, "operand_value": value}
    if code in simple:
        result["predicate"], result["operand_kind"] = simple[code]
    elif 0x11 <= code <= 0x17:
        kept, total = code - 0x10, value >> 56
        if not kept <= total <= 15 or kept != min(total, 7) or ((value & ((1 << 56)-1)) >> (8*kept)):
            return None
        result.update(predicate="unsupported_instruction", operand_kind="fetched_instruction_prefix",
            fetched_bytes=list(value.to_bytes(8, "little")[:kept]), fetched_length=total,
            exported_length=kept, truncated=total > kept)
    elif 0x40 <= code <= 0x4d or 0xb0 <= code <= 0xbd or 0xd0 <= code <= 0xdd:
        sub = code & 15
        if value > 255 if sub in (0, 1) else False if sub in (8, 9) else value != 0:
            return None
        result.update(predicate=pending[sub], category="pending_state" if code < 0xb0 else "register_pending_state" if code < 0xd0 else "register_fault_state",
            operand_kind="vector" if sub == 0 else "priority" if sub == 1 else "control" if sub in (8, 9) else "none")
    elif 0x50 <= code <= 0x55 or 0xa0 <= code <= 0xa5 or 0xc0 <= code <= 0xc5:
        sub=code & 15
        result.update(predicate=instruction[sub], category="continuation" if code < 0xa0 else "register_instruction" if code < 0xc0 else "register_fault_instruction",
            operand_kind=("raw_exit_code", "raw_nrip", "guest_rip", "proposed_next_rip", "fetched_instruction_length", "proposed_next_rip")[sub] if code < 0xa0 else "register_access_context")
    elif 0x80 <= code <= 0x8f:
        result.update(predicate=registers[code-0x80], category="register", operand_kind="startup_destination_command" if code in (0x8d, 0x8e) else "startup_value" if code == 0x8f else "register_access_context")
        if code <= 0x8c:
            if value & 0xfffe0000 or (not (value & 0x10000) and value >> 32):
                return None
            result.update(register_offset=value & 0xffff, access="write" if value & 0x10000 else "read",
                write_value=value >> 32 if value & 0x10000 else None)
    elif 0x1b0 <= code <= 0x1ba:
        names=("memory_map", "address_policy", "monitor_range", "host_pat", "mtrr_capture", "read_shape",
            "ram_admission", "fixed_mtrr_control", "fixed_mtrr_range", "physical_memory_not_wb", "scratch_alias_occupied")
        result.update(predicate=names[code-0x1b0], category="reader_setup", operand_kind="guest_rip")
    elif 0x400 <= code <= 0x7ff:
        base, level = code & 63, (code & 0x1ff) >> 6
        names={1:"unsupported_exit",2:"unsupported_mode",3:"address_overflow",4:"unsupported_guest_cache_control",
            5:"not_executable",6:"privilege_mismatch",7:"instruction_pat_not_wb",8:"unreadable_instruction",9:"segment_limit",
            0x10:"unsupported_physical_width",0x11:"five_level_unsupported",0x12:"noncanonical_address",0x13:"invalid_cr3",
            0x14:"unreadable_table",0x15:"table_not_present",0x16:"reserved_table_entry",0x17:"unsupported_table_entry_bits",
            0x18:"one_gib_unsupported",0x19:"incomplete_walk"}
        if base not in names or not (1 <= level <= 4 if 0x14 <= base <= 0x17 else level == 0):
            return None
        result.update(predicate=names[base], category="operand_fetch" if code & 0x200 else "instruction_fetch",
            walk_level=level or None, operand_kind="physical_read_address" if base in (8, 0x14) else "operand_linear_address" if code & 0x200 else "instruction_linear_address")
    else:
        return None
    return result


def attribute_lookup_diagnostic(refusal: int) -> dict | None:
    """Decode the loss-aware lookup extension; retain unknown encodings raw."""
    if refusal & 0xFFFF != 0x4101:
        return None
    kind = refusal >> 28
    error = bool(refusal & (1 << 27))
    omitted = bool(refusal & (1 << 26))
    payload = (refusal >> 16) & 0x3FF
    if kind == 1:
        # SUCCESS cannot be a status failure. Omitted bits may hold the entire
        # nonzero status, so zero retained bits are valid only in that case.
        if not (error or omitted or payload):
            return None
        status = None if omitted else (int(error) << 63) | payload
        names = {
            0x800000000000000E: "EFI_NOT_FOUND",
            0x800000000000000F: "EFI_ACCESS_DENIED",
        }
        return {
            "kind": "efi_status",
            "status_error_bit": error,
            "status_low10": payload,
            "status_bits_omitted": omitted,
            "status_exact": not omitted,
            "efi_status": None if status is None else f"0x{status:016x}",
            "efi_status_name": names.get(status),
        }
    if kind == 2 and not (error or omitted or payload):
        return {"kind": "success_null_interface"}
    if kind == 3 and not (error or omitted) and 1 <= payload <= 7:
        return {"kind": "success_unaligned_interface", "alignment_remainder": payload}
    # This includes historical 0x4101 (no subtype), reserved future tags and
    # malformed payloads. The containing observation always retains raw words.
    return None


def f7_fallback_diagnostic(refusal: int) -> dict | None:
    """Decode local Get fallback failures without interpreting legacy codes."""
    phase = {0x4115: "construction", 0x4116: "query_or_handoff"}.get(refusal & 0xFFFF)
    reason = refusal >> 16
    if phase is None or not 1 <= reason <= 0xFFFF:
        return None
    names = {
        1: "privilege_or_flags", 2: "cpu_vendor", 3: "cpu_signature",
        4: "basic_cpu_features", 5: "extended_cpu_features", 6: "physical_address_width",
        7: "cpu_topology", 8: "sme_capability", 9: "multi_key_capability",
        10: "cr0", 11: "cr3", 12: "cr4", 13: "efer", 14: "syscfg",
        15: "sev_status", 16: "root_memory_metadata", 17: "table_memory_metadata",
        18: "cpu_context_changed", 19: "invalid_query", 20: "unsupported_mapping",
        21: "no_mapping", 22: "capacity", 23: "access_denied", 24: "device_error",
        0xFFFE: "reader_borrow_conflict", 0xFFFF: "missing_reader_invariant",
    }
    return {"phase": phase, "reason_code": reason, "reason": names.get(reason, "unknown")}


def cache_rendezvous_diagnostic(refusal: int) -> dict | None:
    """Decode recorded failures, without assigning details to legacy 0x400B."""
    stage = {0x400B: "before_transition", 0x4017: "after_transition"}.get(refusal & 0xFFFF)
    if stage is None or not 0 <= refusal <= 0xFFFFFFFF:
        return None
    code = (refusal >> 16) & 0xFF
    processor = refusal >> 24
    global_reasons = {
        0x01: "cpu_scope", 0x02: "allocation", 0x03: "layout", 0x04: "released",
        0x05: "bounds", 0x06: "inventory", 0x07: "reused_or_invalid_callback",
        0x08: "cleanup", 0x7F: "processor_out_of_range",
    }
    capture_reasons = {
        0x10: "output_address", 0x11: "privilege_or_flags", 0x12: "unsupported_cpu",
        0x13: "unsupported_features", 0x14: "address_encryption_active",
        0x15: "unsupported_mtrr_count", 0x16: "unexpected_status",
    }
    processor_reasons = {0x20: "capture_shape", 0x21: "incomplete", 0x22: "stale"}
    paging_reasons = {
        0x30: "not_captured", 0x31: "privilege_level", 0x32: "unexpected_status",
        0x33: "inconsistent_cr0", 0x34: "inconsistent_cr4", 0x35: "unsupported_flags",
        0x36: "inconsistent_flags", 0x37: "unsupported_paging_mode", 0x38: "invalid_cr3",
    }
    mismatch_fields = {
        0x40: "abi_version", 0x41: "captured_fields", 0x42: "refusal", 0x43: "msr_reads",
        0x44: "signature", 0x45: "maximum_basic_leaf", 0x46: "maximum_extended_leaf",
        0x47: "leaf1_ecx", 0x48: "leaf1_edx", 0x49: "physical_bits",
        0x4A: "encryption_eax", 0x4B: "encryption_ebx", 0x4C: "multi_key_eax",
        0x4D: "multi_key_ebx", 0x4E: "reserved", 0x4F: "rflags", 0x50: "cr0",
        0x51: "cr3", 0x52: "cr4", 0x53: "efer", 0x54: "syscfg", 0x55: "sev_status",
        0x56: "pat", 0x57: "mtrr_cap", 0x58: "mtrr_default", 0x59: "top_mem",
        0x5A: "smm_address", 0x5B: "smm_mask", 0x5C: "apic_base",
        0x5D: "mmio_config", 0x5E: "iorr", 0x5F: "variable_mtrrs",
    }
    extra = {}
    if code in global_reasons:
        if processor != 0:
            return None
        category, reason, processor = "rendezvous", global_reasons[code], None
    elif code in capture_reasons:
        category, reason = "capture", capture_reasons[code]
    elif code in processor_reasons:
        category, reason = "rendezvous", processor_reasons[code]
    elif code in paging_reasons:
        category, reason = "paging_root", paging_reasons[code]
    elif code in mismatch_fields:
        category, reason = "configuration_mismatch", mismatch_fields[code]
    elif 0x80 <= code <= 0x9F:
        category, reason = "capture", "privilege_or_flags"
        flag_bits = (("TF", 8), ("IF", 9), ("DF", 10), ("NT", 14), ("AC", 18))
        extra["unsupported_flags"] = [name for index, (name, _) in enumerate(flag_bits)
                                      if code & (1 << index)]
        extra["unsupported_rflags_mask"] = sum(1 << bit for index, (_, bit) in enumerate(flag_bits)
                                              if code & (1 << index))
        # A zero mask does not establish CPL or any other unrecorded cause.
        extra["flags_observed"] = True
    elif 0xC0 <= code <= 0xFF and code != 0xC3:
        category, reason = "configuration_mismatch", "cr4"
        bit = code - 0xC0
        names = {0: "VME", 1: "PVI", 2: "TSD", 4: "PSE", 5: "PAE", 6: "MCE",
                 7: "PGE", 8: "PCE", 9: "OSFXSR", 10: "OSXMMEXCPT", 11: "UMIP",
                 12: "LA57", 16: "FSGSBASE", 17: "PCIDE", 18: "OSXSAVE",
                 20: "SMEP", 21: "SMAP", 22: "PKE", 23: "CET"}
        extra.update(rejected_cr4_bit=bit, rejected_cr4_bit_name=names.get(bit),
                     rejected_cr4_mask=f"0x{1 << bit:016x}",
                     ignored_cr4_mask="0x0000000000000008")
        # This is the rejected difference after ignoring AP-only DE, not either
        # CPU's register value or necessarily their complete raw XOR.
    else:
        return None
    return {"stage": stage, "detail_code": code, "processor_index": processor,
            "category": category, "reason": reason, **extra}


def persistent_returning_observation(words: tuple[int, ...]) -> dict:
    """Detail 7: journal DWORDs 4/5/6 survive every lifecycle notification."""
    refusal, counts, metadata = words[10], words[11], words[9]
    overflow = bool(metadata & (1 << 29))
    non_boolean = bool(metadata & (1 << 30))
    header_invalid = bool(metadata & (1 << 31))

    def numeric(value: int) -> int | None:
        # Saturated fields are preserved below as raw words, never as exact
        # refusal codes or entry counts that a caller could misinterpret.
        return None if overflow else value

    def flag(bit: int) -> bool | None:
        return None if non_boolean else bool(metadata & (1 << bit))

    observation = {
        "format": "persistent_v1",
        "encoding_overflow": overflow,
        "non_boolean_flags": non_boolean,
        "inner_header_valid": not header_invalid,
        "numeric_fields_exact": not overflow,
        "boolean_fields_valid": not non_boolean,
        "outcome": numeric(metadata & 0xF),
        "refusal_code": numeric(refusal),
        "attempted_entries": numeric(counts & 0xFFFF),
        "completed_exits": numeric(counts >> 16),
        "delivery_stage": numeric((metadata >> 4) & 7),
        "rust_entered": flag(7),
        "rust_completed": flag(8),
        "cleanup_complete": flag(9),
        "restoration_complete": flag(10),
        "canary_called": flag(11),
        "canary_observed": flag(12),
        "canary_failure_present": bool(metadata & (1 << 13)),
        "lifecycle_counts": {
            "ready_to_boot": numeric((metadata >> 14) & 31),
            "after_ready_to_boot": numeric((metadata >> 19) & 31),
            "exit_boot_services": numeric((metadata >> 24) & 31),
        },
        "raw_words": {
            "refusal_u32": refusal, "entry_exit_u32": counts,
            "metadata_u32": metadata,
        },
    }
    if not (overflow or header_invalid):
        multi_failures = {
            0x4301: "entry_exit_count", 0x4302: "exit_site", 0x4303: "operand",
            0x4304: "guest_registers", 0x4305: "guest_stack_or_flags",
            0x4306: "pending_event", 0x4307: "next_rip",
            0x4308: "unexpected_exit", 0x4309: "entry_limit",
        }
        if refusal in multi_failures:
            observation["multi_exit_failure"] = {"reason": multi_failures[refusal]}
        lookup = attribute_lookup_diagnostic(refusal)
        if lookup is not None:
            observation["attribute_lookup"] = lookup
        fallback = f7_fallback_diagnostic(refusal)
        if fallback is not None:
            observation["f7_get_fallback"] = fallback
        cache = cache_rendezvous_diagnostic(refusal)
        if cache is not None:
            observation["cache_rendezvous"] = cache
    return observation


def startup_route_diagnostic(code, value):
    names = {1: "destination_form", 2: "self_destination", 3: "destination_unassigned",
        4: "recipient_not_ready", 5: "recipient_mode_invalid", 6: "broadcast",
        7: "foreign_match", 8: "duplicate_match", 9: "no_match", 10: "selected_self",
        11: "mailbox_mismatch", 12: "queue_busy", 13: "route_busy", 14: "init_vector"}
    predicate, wide, count = code & 15, bool(code & 32), code >> 6
    if predicate not in names or code > 0x7ff:
        return None
    recipient = predicate in (4, 5, 6, 7, 8, 10, 12)
    mode, cause = (value >> 56) & 7, (value >> 59) & 3
    if wide:
        if count:
            return None
        icr, source, identity, mode_name, cause_name = value, None, None, None, None
    else:
        if value >> 61 or mode > 4 or (not recipient and (value >> 48 or count)):
            return None
        if recipient and predicate not in (4, 5) and mode == 0:
            return None
        icr = (value & 0xffffffff) | (((value >> 32) & 255) << 32)
        source, identity = (value >> 40) & 255, ((value >> 48) & 255) if recipient else None
        mode_name = {0: "unknown", 1: "xapic_8bit", 2: "extended_xapic_4bit",
            3: "extended_xapic_8bit", 4: "x2apic_32bit"}[mode] if recipient else None
        cause_name = ("observed", "guest_control_write", "guest_init", "guest_x2apic_promotion")[cause] if recipient else None
    return {"predicate": names[predicate], "code": predicate,
        "access": "x2apic_msr" if code & 16 else "xapic_mmio", "icr": icr,
        "destination_apic_id": icr >> 32, "source_apic_id": source,
        "recipient_apic_id": identity, "recipient_mode": mode_name,
        "recipient_mode_cause": cause_name,
        "recipient_init_count": count if recipient and not wide else None,
        "recipient_init_count_saturated": count == 31 if recipient and not wide else None,
        "identity_history_omitted": wide}


def startup_target_diagnostic(code, value):
    reason, detail, wide = code & 7, (code >> 3) & 127, bool(code & 1024)
    names = {1: "unsupported_apic_mode", 2: "unsupported_apic_layout",
        3: "unavailable_apic_register", 4: "apic_register_state", 5: "unsupported_cpu_signature",
        6: "target_pending_state", 7: "startup_service_stage"}
    if reason not in names or code > 0x7ff:
        return None
    if (reason in (1, 5) and detail) or (reason == 2 and detail != 3):
        return None
    if (reason in (3, 4) and detail > 0x53) or (reason == 6 and detail > 13):
        return None
    if reason == 7 and not 1 <= detail <= 9:
        return None
    return {"reason": names[reason], "detail": detail,
        "register_offset": detail * 16 if reason in (2, 3, 4) else None,
        "observed_value": value if wide else value & 0xffffffff,
        "target_apic_id": None if wide else value >> 32, "identity_omitted": wide}


def apic_takeover_diagnostic(code):
    if code >> 56 != 0xa1:
        return None
    reason, offset = (code >> 48) & 127, (code >> 32) & 0xffff
    valid = ((reason == 1 and 0x480 <= offset <= 0x4f0 and offset % 16 == 0)
        or (reason == 2 and 0x500 <= offset <= 0x530 and offset % 16 == 0)
        or (reason in (3, 4) and offset == 0x410))
    if not valid:
        return None
    truncated = bool(code & (1 << 55))
    return {"reason": {1: "hidden_ier_not_all_enabled", 2: "hidden_extended_lvt_active",
        3: "routing_control_readback", 4: "routing_control_restore_readback"}[reason],
        "register_offset": offset, "observed_low32": code & 0xffffffff,
        "observed_value": None if truncated else code & 0xffffffff,
        "observed_value_truncated": truncated}


def resident_boot_observation(words: tuple[int, ...], phase: int) -> dict | None:
    """Detail8; use actual journal4/5/6 -> snapshot10/11/9 wire rotation."""
    metadata, detail, last = words[10], words[11], words[9]
    raw = {"journal4": metadata, "journal5": detail, "journal6": last}
    if phase == 0x14:
        stage, reason = metadata & 0xff, (metadata >> 8) & 0xff
        stages = {1: "ebs_hook_admission", 2: "cpu_admission", 3: "processor_inventory",
                  4: "loaded_image", 5: "resident_payload", 6: "resident_allocation",
                  7: "paging_configuration", 8: "memory_types", 9: "initial_memory_map",
                  10: "resident_map_coverage", 11: "resident_slot_mapping", 12: "resident_slot_copy",
                  13: "physical_bootstrap", 14: "runtime_preparation", 15: "retained_memory_map",
                  16: "host_mapping_closure", 17: "nested_page_tables", 18: "bootstrap_mapping",
                  19: "processor_admission", 20: "resident_publication", 21: "low_page_allocation",
                  22: "low_page_memory_map", 23: "low_page_mapping", 24: "low_page_construction",
                  25: "bootstrap_processor_count", 26: "bootstrap_apic_capability",
                  27: "bootstrap_vm_cr", 28: "bootstrap_apic_base"}
        reasons = {0: "operation_status", 1: "allocation_firmware", 2: "allocation_address",
                   3: "allocation_cleanup", 4: "allocation_released", 5: "allocation_layout",
                   6: "allocation_map", 16: "memory_map_firmware", 17: "memory_map_bounds",
                   18: "memory_map_layout", 19: "memory_map_retry_limit",
                   20: "memory_map_cleanup", 21: "memory_map_released",32:"processor_admission_detail"}
        valid = stage in stages and reason in reasons
        return {"format": "resident_preparation_v2", "encoding_valid": valid,
                "stage": stage, "stage_name": stages.get(stage) if valid else None,
                "reason": reason, "reason_name": reasons.get(reason) if valid else None,
                "underlying_status": (detail & 0x7fffffff) | ((detail & 0x80000000) << 32),
                "address": last | ((metadata >> 16) << 32),
                "observed_value": (last | ((metadata >> 16) << 32)) if valid and stage in (25, 26, 27, 28) else None,
                "observed_register": {25: "processor_count", 26: "CPUID.1:ECX", 27: "VM_CR", 28: "APIC_BASE"}.get(stage) if valid else None,
                "rust_entered": True if valid else None, "hook_armed": False if valid else None,
                "all_cpus_activated": False, "windows_boot_proven": False,
                "raw_words": raw}
    if phase == 0x10:
        valid = metadata & ~0x7ff == 0 and metadata & 0xff <= 4
        return {"format": "resident_parent_v1", "encoding_valid": valid,
                "delivery_stage": metadata & 0xff if valid else None,
                "rust_entered": bool(metadata & 0x100) if valid else None,
                "hook_armed": bool(metadata & 0x200) if valid else None,
                "failure_kind": ("efi_status" if metadata & 0x400 else "child_failure") if valid else None,
                "failure": detail | (last << 32), "raw_words": raw,
                "windows_boot_proven": False}
    if phase == 0x13:
        if metadata & 255 == 0x85:
            slot, reason = (metadata >> 8) & 31, (metadata >> 13) & 255
            mode, write = (metadata >> 21) & 3, bool(metadata & (1 << 23))
            value = detail | (last << 32)
            policy = {0x80: "unsupported_processor_profile", 0x81: "current_reserved_bits",
                0x82: "current_encryption_enabled", 0x83: "requested_reserved_bits",
                0x84: "unsupported_control_change", 0x85: "write_readback_mismatch"}
            boundary = (1 <= reason <= 6 or 0x10 <= reason <= 0x15
                or 0x20 <= reason <= 0x2d or 0x30 <= reason <= 0x35
                or 0x40 <= reason <= 0x4d or 0x50 <= reason <= 0x55 or 0x60 <= reason <= 0x65)
            valid = metadata >> 24 == 0x10 and (reason in policy or boundary)
            valid = valid and ((mode == 3 and (boundary or reason == 0x80))
                or (mode != 3 and 0x81 <= reason <= 0x85 and write))
            requested = (detail if mode == 0 else value if mode == 1 else None) if valid else None
            observed = (last if mode == 0 else value if mode == 2 else None) if valid else None
            return {"format": "resident_syscfg_refusal_v1", "encoding_valid": valid,
                "stage": 0x85, "stage_name": "runtime_terminal_stop" if valid else None,
                "processor_slot": slot if valid else None, "exit_code": 0x7c if valid else None,
                "reason": "syscfg_refusal" if valid else None, "msr_index": 0xc0010010 if valid else None,
                "write": write if valid else None, "predicate": policy.get(reason, "instruction_boundary") if valid else None,
                "predicate_code": reason if valid else None, "requested_value": requested,
                "observed_value": observed, "observed_is_readback": valid and reason == 0x85,
                "changed_bits": requested ^ observed if requested is not None and observed is not None else None,
                "boundary_context": value if valid and mode == 3 else None,
                "operand_omitted": mode != 0 if valid else None, "guest_rip": None,
                "all_cpus_activated": True if valid else None, "windows_boot_proven": False,
                "post_activation_terminal_failure_exported": valid,
                "export_limit": "Paired DWORD operands, or one explicitly selected full-width operand/context; RIP is not exported.",
                "raw_words": raw}
        if metadata & 255 == 0x84:
            slot, count = (metadata >> 16) & 31, ((metadata >> 21) & 31) + 1
            value = detail | (last << 32)
            failure = apic_takeover_diagnostic(value)
            valid = metadata >> 26 == 1 and metadata & 0xff00 == 0 and slot < count and failure is not None
            return {"format": "resident_bsp_takeover_failure_v1", "encoding_valid": valid,
                "stage": 0x84, "stage_name": "bsp_takeover_refusal" if valid else None,
                "processor_slot": slot if valid else None, "processor_count": count if valid else None,
                "callback_return_code": value if valid else None,
                "apic_takeover_failure": failure if valid else None,
                "all_cpus_activated": False, "windows_boot_proven": False,
                "post_loader_failure_exported": False, "raw_words": raw}
        if metadata & 0xff == 0x83:
            slot, code = (metadata >> 8) & 31, (metadata >> 13) & 0x7ff
            kind, version = (metadata >> 24) & 15, metadata >> 28
            value = detail | (last << 32)
            kinds = {0: ("unhandled_exit", "guest_rip"), 1: ("instruction_fetch_refusal", "guest_rip"),
                     2: ("msr_refusal", "msr_index"), 3: ("nested_page_fault", "guest_physical_address"),
                     4: ("wide_exit_code", "raw_exit_code"), 5: ("init_ack_refusal", "guest_rip"),
                     6: ("startup_service_refusal", "guest_rip"), 7: ("unexpected_init", "guest_rip"),
                     8: ("apic_mmio_refusal", "guest_physical_address"), 9: ("route_refusal", "msr_index"),
                     10: ("instruction_fetch_refusal", "fetch_failure_and_rip"),
                     11: ("efer_refusal", "efer_failure_operand"),
                     12: ("vmcr_refusal", "vmcr_failure_operand"),
                     13: ("apic_mmio_refusal", "apic_failure_operand"),
                     14: ("startup_route_refusal", "startup_route_evidence"),
                     15: ("startup_target_refusal", "startup_target_evidence")}
            valid = version == 1 and kind in kinds
            if kind == 4:
                valid = valid and code == 0x7ff and value > 0x7ff
            fetch_failure = None
            fetch_rip = None
            efer_failure = None
            vmcr_failure = None
            apic_failure = apic_failure_diagnostic(code, value) if kind == 13 else None
            route_failure = startup_route_diagnostic(code, value) if kind == 14 else None
            target_failure = startup_target_diagnostic(code, value) if kind == 15 else None
            if kind == 14:
                valid = valid and route_failure is not None
            if kind == 15:
                valid = valid and target_failure is not None
            if kind == 13:
                valid = valid and apic_failure is not None
            if kind in (11, 12):
                reason, direction = code & 0x1ff, code >> 9
                simple = {1: ("unsupported_initial_state", "owned_logical_efer"),
                    2: ("backing_mismatch", "actual_vmcb_efer"),
                    3: ("unsupported_mode", "guest_cr0"),
                    4: ("unsupported_debug_state", "guest_rflags"),
                    5: ("unsupported_msr", "rejected_msr_index"),
                    6: ("unsupported_value", "attempted_efer_value")}
                if kind == 12:
                    del simple[1], simple[2]
                    simple[6] = ("unsupported_value", "attempted_vmcr_value")
                instruction_names = ("exit_does_not_permit_candidate", "nrip_not_established",
                    "noncanonical_rip", "noncanonical_nrip", "invalid_instruction_length", "unsupported_instruction_bytes")
                instruction_operands = ("raw_exit_info1", "raw_nrip", "guest_rip", "proposed_next_rip",
                    "fetched_instruction_length", "fetched_opcode_le16")
                pending_names = ("reserved_vector", "invalid_task_priority", "request_not_queued",
                    "request_not_armed", "request_already_consumed", "pending_injection",
                    "nested_delivery_unsupported", "pending_virtual_interrupt", "unsupported_control",
                    "unsupported_nested_control", "control_mismatch", "invalid_entry", "guest_shutdown",
                    "inconsistent_virtual_interrupt_exit")
                category, subreason, operand = None, None, None
                if reason in simple:
                    category, operand = simple[reason]
                    valid = valid and (reason != 5 or value <= 0xffffffff)
                elif 0x10 <= reason <= 0x15 or 0x30 <= reason <= 0x35:
                    sub = reason & 15
                    category = "instruction" if reason < 0x30 else "fault_instruction"
                    subreason, operand = instruction_names[sub], instruction_operands[sub]
                    valid = valid and (sub != 5 or value <= 0xffff) and (sub != 4 or value == 2)
                elif 0x50 <= reason <= 0x54 or 0x60 <= reason <= 0x64:
                    sub = reason & 15
                    category = "hardware_instruction" if reason < 0x60 else "hardware_fault_instruction"
                    subreason = instruction_names[sub]
                    operand = "raw_exit_info1" if sub == 0 else "guest_rip" if sub == 2 else "hardware_next_rip"
                elif 0x20 <= reason <= 0x2d or 0x40 <= reason <= 0x4d:
                    sub = reason & 15
                    category = "pending_state" if reason < 0x40 else "fault_state"
                    subreason = pending_names[sub]
                    operand = "vector" if sub == 0 else "priority" if sub == 1 else "control" if sub in (8, 9) else "none"
                    valid = valid and (value <= 255 if sub in (0, 1) else True if sub in (8, 9) else value == 0)
                else:
                    valid = False
                valid = valid and direction <= 2
                if valid:
                    msr_failure = {"code": reason, "error": category, "subreason": subreason,
                        "access": ("read", "write", "invalid_exit_info1")[direction],
                        "operand_kind": operand, "operand_value": value,
                        "fetched_opcode_bytes": list(value.to_bytes(2, "little")) if operand == "fetched_opcode_le16" else None,
                        "fetched_opcode_width": 2 if operand == "fetched_opcode_le16" else None}
                    if kind == 11:
                        efer_failure = msr_failure
                    else:
                        vmcr_failure = msr_failure
            if kind == 10:
                failure = value >> 48
                base, level = failure & 255, failure >> 8
                names = {1: "unsupported_exit", 2: "unsupported_mode", 3: "address_overflow",
                    4: "unsupported_guest_cache_control", 5: "not_executable",
                    6: "privilege_mismatch", 7: "instruction_pat_not_wb",
                    8: "unreadable_instruction", 9: "segment_limit",
                    0x10: "unsupported_physical_width", 0x11: "five_level_unsupported",
                    0x12: "noncanonical_address", 0x13: "invalid_cr3",
                    0x14: "unreadable_table", 0x15: "table_not_present",
                    0x16: "reserved_table_entry", 0x17: "unsupported_table_entry_bits",
                    0x18: "one_gib_unsupported", 0x19: "incomplete_walk",
                    0x30: "reader_memory_map", 0x31: "reader_address_policy",
                    0x32: "reader_monitor_range", 0x33: "host_pat0_not_wb",
                    0x34: "mtrr_capture_refusal", 0x35: "physical_read_shape",
                    0x36: "physical_ram_admission", 0x37: "fixed_mtrr_control",
                    0x38: "fixed_mtrr_range", 0x39: "physical_memory_not_proven_wb",
                    0x3a: "scratch_alias_occupied"}
                valid = valid and base in names and (1 <= level <= 4 if 0x14 <= base <= 0x17 else level == 0)
                if valid:
                    fetch_failure = {"code": failure, "predicate": names[base], "walk_level": level or None}
                    fetch_rip = value & 0xffffffffffff
                    if fetch_rip & (1 << 47):
                        fetch_rip |= 0xffff000000000000
            context_kind = kinds[kind][1] if valid else None
            return {"format": "resident_terminal_stop_v1", "encoding_valid": valid,
                    "stage": 0x83, "stage_name": "runtime_terminal_stop" if valid else None,
                    "processor_slot": slot if valid else None, "processor_count": None,
                    "exit_code": (None if kind == 15 else (0x7c if code & 16 else 0x400) if kind == 14 else value if kind == 4 else 0x7c if kind in (11, 12) else 0x400 if kind == 13 else code) if valid else None,
                    "reason": kinds[kind][0] if valid else None,
                    "context_kind": context_kind, "context_value": value if valid else None,
                    "guest_rip": value if context_kind == "guest_rip" else fetch_rip,
                    "fetch_failure": fetch_failure,
                    "efer_failure": efer_failure,
                    "vmcr_failure": vmcr_failure,
                    "apic_failure": apic_failure if valid else None,
                    "startup_route_failure": route_failure if valid else None,
                    "startup_target_failure": target_failure if valid else None,
                    "msr_index": value if context_kind == "msr_index" else {11: 0xc0000080, 12: 0xc0010114}.get(kind) if valid else None,
                    "guest_physical_address": value if context_kind == "guest_physical_address" else None,
                    "activation_failure": None, "all_cpus_activated": valid,
                    "post_activation_terminal_failure_exported": valid,
                    "windows_boot_proven": False, "post_loader_failure_exported": False,
                    "export_limit": "one selected context from a terminal stop after activation; "
                                    "does not establish final loader return; full RIP/info1/info2 and CPU count are not all exported",
                    "raw_words": raw}
        if metadata & 0xff == 0x82:
            reason = (metadata >> 8) & 0xff
            slot, count, version = (metadata >> 16) & 31, ((metadata >> 21) & 31) + 1, metadata >> 26
            boundary = 1 <= reason <= 33
            rust = 0x40 <= reason <= 0x7f
            valid = version == 1 and slot < count and (boundary or rust or reason in (0x81, 0x82, 0x83))
            if reason == 0x81:
                valid = valid and detail <= 255 and last <= 255 and detail != last
            elif reason == 0x82:
                valid = valid and last == 1 << slot and detail & last == 0
            elif reason == 0x83:
                valid = valid and (detail | (last << 32)) > 63
            predicates = (None, "trap_flag", "cpl", "max_basic_leaf", "vendor_ebx", "vendor_edx",
                          "vendor_ecx", "hypervisor_present", "baseline_features", "max_extended_leaf",
                          "long_mode", "cr0_em_ts", "cr4_osfxsr", "efer_ffxsr", "xsave_max_leaf",
                          "xsave_features", "supervisor_without_xsaves", "xss_nonzero",
                          "osxsave_cpuid_missing", "xcr0_high", "xcr0_profile", "avx_cpuid_missing",
                          "xcr0_unsupported", "xsave_size_small", "xsave_size_large", "xsave_max_size",
                          "avx_size", "avx_flags", "avx_reserved", "avx_offset_small", "avx_offset_large",
                          "avx_extent", "osxsave_without_xsave", "osxsave_cpuid_unexpected")
            category = ("boundary_refusal" if boundary else "rust_callback_return" if rust or reason == 0x83 else
                        "wait_identity_mismatch" if reason == 0x81 else "missing_guest_ack")
            return {"format": "resident_ap_failure_v1", "encoding_valid": valid,
                    "stage": 0x82, "stage_name": "activation_failed_ap" if valid else None,
                    "activation_failure": 33 if valid else None,
                    "processor_slot": slot if valid else None, "processor_count": count if valid else None,
                    "reason": reason if valid else None, "reason_category": category if valid else None,
                    "boundary_predicate": reason if valid and boundary else None,
                    "boundary_predicate_name": predicates[reason] if valid and boundary else None,
                    "callback_return_code": (reason - 0x40 if rust else detail | (last << 32))
                                            if valid and (rust or reason == 0x83) else None,
                    "apic_takeover_failure": apic_takeover_diagnostic(detail | (last << 32)) if valid and reason == 0x83 else None,
                    "observation_kind": ("raw_cr0_context" if rust else "boundary_predicate_operand")
                                        if valid and (boundary or rust) else None,
                    "observed_value": detail | (last << 32) if valid and (boundary or rust) else None,
                    "observed_apic_id": detail if valid and reason == 0x81 else None,
                    "expected_apic_id": last if valid and reason == 0x81 else None,
                    "guest_ack_mask": detail if valid and reason == 0x82 else None,
                    "required_ack_bit": last if valid and reason == 0x82 else None,
                    "export_limit": "one AP refusal only; observation context does not by itself identify a hardware defect",
                    "all_cpus_activated": False, "windows_boot_proven": False,
                    "post_loader_failure_exported": False, "raw_words": raw}
        if metadata & 0xff == 0x81:
            # A versioned failure-42 record replaces count-in-journal6 with
            # observed/expected DWORDs. The complete BSP sample stays local.
            predicate, count, version = (metadata >> 8) & 0xff, (metadata >> 16) & 0xff, metadata >> 24
            predicates = {1: ("signature", 0x00b40f40), 2: ("version", 0x81050010),
                          3: ("feature", 0x00040007), 4: ("topology", count),
                          5: ("extended_control_reserved_bits", 0)}
            valid = version == 1 and 1 <= count <= 32 and predicate in predicates
            if valid:
                valid = last == predicates[predicate][1]
                if predicate in (1, 2, 3):
                    valid = valid and detail != last
                elif predicate == 5:
                    valid = valid and detail != 0 and detail & 7 == 0
            return {"format": "resident_bsp_routing_failure_v1", "encoding_valid": valid,
                    "stage": 0x81, "stage_name": "activation_failed_bsp_routing" if valid else None,
                    "activation_failure": 42 if valid else None,
                    "predicate": predicates[predicate][0] if valid else None,
                    "processor_count": count if valid else None,
                    "observed_value": detail if valid else None,
                    "expected_value": last if valid else None,
                    "comparison": ("native topology/APIC-base admission rejected; observed is BSP APIC ID, expected is inventory count"
                                   if predicate == 4 else "observed != expected") if valid else None,
                    "export_limit": "one failed field only; full BSP registers and inventory remain local; "
                                    "extended_control exports reserved bits only; topology does not identify the invalid member",
                    "all_cpus_activated": False, "windows_boot_proven": False,
                    "post_loader_failure_exported": False, "raw_words": raw}
        stage, slot = metadata & 0xffff, metadata >> 16
        names = {1: "hook_armed", 2: "firmware_ebs_returned", 3: "ap_release",
                 4: "bsp_capture", 5: "all_activated_before_loader_return", 0x80: "activation_failed"}
        terminal_prepared = bool(detail & 1) if stage == 5 and detail in (0x01000000, 0x01000001) else None
        valid = (stage in names and 1 <= last <= 32
                 and (slot < last if stage in (3, 4) else slot == 0)
                 and (stage == 0x80 or detail == 0 or (stage == 5 and terminal_prepared is not None)))
        return {"format": "resident_boot_v1", "encoding_valid": valid,
                "stage": stage, "stage_name": names.get(stage) if valid else None,
                "processor_slot": slot if valid and stage in (3, 4) else None,
                "processor_count": last if valid else None,
                "activation_failure": detail if valid and stage == 0x80 else None,
                "all_cpus_activated": valid and stage == 5,
                "terminal_reporter_prepared": terminal_prepared if valid else None,
                "windows_boot_proven": False, "post_loader_failure_exported": False,
                "raw_words": raw}
    return None


def decode_frame(value: str) -> dict:
    if not re.fullmatch(r"[0-9a-fA-F]{128}", value):
        raise ValueError("snapshot must contain exactly 512 bits")
    raw = bytes.fromhex(value)[::-1]  # OpenOCD prints a whole DR value MSB first.
    words = struct.unpack("<16I", raw)
    if words[0] != 0x50414E53 or words[1] != 0x02000001:
        raise ValueError("snapshot magic, schema, or frame length mismatch")
    if zlib.crc32(raw[:60]) != words[15]:
        raise ValueError("snapshot CRC mismatch")
    if words[12] & ~0x3FFF:
        raise ValueError("reserved hardware-status bits are set")
    phase, detail = words[8] & 0xFFFF, words[8] >> 16
    # Candidate-only lifecycle bits survive into the final USER2 snapshot.
    # Absence is not failure: older record-only images never attempted a load.
    payload_status = "not_reported"
    if phase in (0x28, 0x35, 0x40) and detail & 0xFF == 4:
        payload_status = {
            0: "not_reported", 0x2000: "loaded_and_freed",
            0x4000: "load_failed", 0x6000: "invalid_status",
        }[detail & 0x6000]
    elif detail == 5 and words[10:12] == (0x44524143, 0x44414F4C):
        if phase == 0x10:
            payload_status = "loaded_and_freed"
        elif phase == 0x1F:
            payload_status = "load_failed"
    returning_status = "not_reported"
    returning_observation = None
    if phase in (0x10, 0x28, 0x35, 0x40) and detail & 0xFF in (6, 7):
        # Both returning formats retain the result bits through lifecycle
        # records. Detail 7 also retains diagnostics in the other three words.
        returning_status = {
            0: "not_reported", 0x2000: "completed", 0x4000: "refused",
            0x8000: "failed",
        }.get(detail & 0xE000, "invalid_status")
        if detail & 0xFF == 7:
            returning_observation = persistent_returning_observation(words)
        elif phase == 0x10:
            # The returning adapter puts outcome/refusal in journal DWORD 4,
            # counts in 5, and delivery stage in 6. Snapshot RTL exports these
            # as frame DWORDs 10, 11, and 9 respectively.
            returning_observation = {
                "outcome_low16": words[10] & 0xFFFF,
                "refusal_low16": words[10] >> 16,
                "attempted_entries_low16": words[11] & 0xFFFF,
                "completed_exits_low16": words[11] >> 16,
                "delivery_stage": words[9],
            }
    return {
        "schema": 1,
        "fpga_build_id": f"{words[3]:08x}{words[2]:08x}",
        "rom_build_id": f"{words[5]:08x}{words[4]:08x}",
        "boot_id": words[6], "sequence": words[7],
        "phase": phase, "detail": detail,
        "card_payload_status": payload_status,
        "native_returning_status": returning_status,
        "native_returning_observation": returning_observation,
        "native_resident_observation": resident_boot_observation(words, phase) if detail == 8 else None,
        # Detail 7 deliberately carries diagnostics in the former CPU field.
        "cpu_id": None if detail == 8 or (returning_observation and detail & 0xFF == 7) else words[9],
        "context": f"{words[11]:08x}{words[10]:08x}",
        "hardware_status": words[12], "rom_reads": words[13], "bar_writes": words[14],
    }


def consistent_snapshot(text: str, manifest: dict | None = None) -> dict:
    previous = None
    for line in text.splitlines():
        # Non-frame log lines are not observations. Malformed frame lines break
        # consecutiveness, including a CRC error between otherwise equal reads.
        value = line.strip()
        if value.startswith("SNAPSHOT:"):
            value = value[len("SNAPSHOT:"):].strip()
        elif not re.fullmatch(r"[0-9a-fA-F]{128}", value):
            continue
        try:
            decoded = decode_frame(value)
            if manifest is not None:
                for key in ("fpga_build_id", "rom_build_id"):
                    if decoded[key] != manifest.get(key):
                        raise ValueError(f"{key} differs from expected candidate")
        except ValueError:
            previous = None
            continue
        value = value.lower()
        if value == previous:
            return decoded
        previous = value
    raise ValueError("no consecutive identical CRC-valid frames matching the expected build")



def decode_percpu_frame(value: str, manifest: dict | None = None) -> dict:
    """CRC-protected bounded USER3 bank. A valid record is last known, not a live CPU read."""
    if not re.fullmatch(r"[0-9a-fA-F]{256}", value):
        raise ValueError("perCPU snapshot must contain exactly1024bits")
    raw=bytes.fromhex(value)[::-1]
    w=struct.unpack("<32I",raw)
    if w[:2]!=(0x55504353,0x04000001) or zlib.crc32(raw[:124])!=w[31]:
        raise ValueError("perCPU magic/schema/CRC mismatch")
    if w[6]&~127 or w[7]&~3 or any(w[27:31]):
        raise ValueError("perCPU reserved frame bits")
    result={"schema":1,"fpga_build_id":f"{w[3]:08x}{w[2]:08x}",
        "rom_build_id":f"{w[5]:08x}{w[4]:08x}","bank":w[6]&63,
        "processor_slot":w[6]&31,"kind":"first_fault" if w[6]&32 else "last_progress",
        "record_valid":bool(w[6]&64),"transport_errors":w[7],
        "transport_error_names":[name for bit,name in enumerate(("invalid_write_or_alignment","invalid_or_incomplete_commit")) if w[7]&(1<<bit)],
        "record":None,"observation":"last_published_checkpoint_not_live_registers"}
    if manifest is not None:
        for key in ("fpga_build_id","rom_build_id"):
            if result[key]!=manifest.get(key): raise ValueError(f"{key} differs from expected candidate")
    r=w[8:27]
    if result["record_valid"]:
        if r[0]==0 or (r[1]>>8)&255!=1 or r[1]&0xfffe0000:
            raise ValueError("invalid perCPU record schema")
        if result["kind"]=="first_fault" and not r[1]&0x10000:
            raise ValueError("first-fault bank lacks fault marker")
        names={1:("exit",("guest_rip","exit_code","exit_info1","exit_info2","guest_cr3","exit_count")),
            2:("resume",("guest_rip","exit_code","exit_info1","exit_info2","guest_cr3","exit_count")),
            3:("stop",("guest_rip","exit_code","reason_info1","reason_info2","exit_count","reserved")),
            4:("host_fault",("host_rip","error_code","cr2","cr3","host_rsp","rflags")),
            5:("pause",("guest_rip","guest_cr3","exit_count","reserved0","reserved1","reserved2")),
            6:("transport_revoking",("config_page","bar_page","mutation_address","value","width","reserved")),
            7:("terminal_barrier",("expected_cpus","acknowledged_cpus","owner","outcome","initial_acknowledged","reserved")),
            8:("syscfg",("guest_rip","current_value","requested_value","observed_value","changed_bits","reserved")),
            9:("initial_ack",("guest_rip","guest_cr3","processor_slot","processor_count","reserved0","reserved1")),
            10:("raw_syscfg_boundary",("raw_exit_rip","raw_nrip","preentry_rip","guest_cr0","physical_mtrr_def_type","host_cr0")),
            11:("raw_boundary_identity_mismatch",("raw_exit_vmcb_pa","expected_vmcb_pa","captured_and_assigned_apic_ids","raw_exit_rip","raw_nrip","preentry_rip")),
            12:("processor_admission_failure",("predicate","register_or_address","observed","expected_or_context","original_status_or_code","processor_and_count")),
            13:("raw_cache_msr_boundary",("raw_exit_rip","raw_guest_rcx","edx_eax_operand","raw_nrip","reason_info1","reason_info2"))}
        event=r[1]&255
        contexts=[r[n]|r[n+1]<<32 for n in range(6,18,2)]
        name,fields=names.get(event,("unknown",()))
        aux_name={1:"msr_index_if_msr_exit",2:"msr_index_if_msr_exit",4:"exception_vector",6:"revocation_reason",8:"syscfg_stage",10:"stop_reason",11:"stop_reason",12:"admission_operation",13:"stop_reason_low32"}.get(event,"aux")
        result["record"]={"sequence":r[0],"event":event,"event_name":name,
            "fields":dict(zip(fields,contexts)),"aux_name":aux_name,"first_fault_requested":bool(r[1]&0x10000),
            "boot_id":r[2],"apic_id":r[3],"tsc":r[4]|r[5]<<32,
            "contexts":[r[n]|r[n+1]<<32 for n in range(6,18,2)],"aux":r[18],"raw_words":list(r)}
        if event == 10:
            result["record"]["boundary_observation"] = "assembly_capture_before_dispatch"
            result["record"]["zero_length_at_hardware_return"] = contexts[0] == contexts[1]
        elif event == 11:
            result["record"]["captured_apic_id"] = contexts[2] >> 32
            result["record"]["assigned_apic_id"] = contexts[2] & 0xffffffff
        elif event == 13:
            result["record"]["boundary_observation"] = "assembly_capture_before_dispatch"
            result["record"]["msr_index"] = contexts[1] & 0xffffffff
            result["record"]["access"] = "not_exported"
            result["record"]["instruction_completed"] = "not_established"
        elif event == 12:
            operation=r[18]; processor=contexts[5]&0xffffffff; count=contexts[5]>>32
            if not r[1]&0x10000 or not 1<=operation<=8 or not 1<=count<=32 or not 1<=contexts[0]<=0xffffffff or (processor>=count and processor!=0xffffffff):
                raise ValueError("invalid admission failure metadata")
            if processor!=0xffffffff and processor!=result["processor_slot"]:
                raise ValueError("admission processor differs from transport bank")
            result["record"]["admission_failure"]={
                "operation":operation,"operation_name":{1:"bootstrap_root",2:"mp_services",3:"processor_callback",4:"cache_capture",5:"returned_inventory",6:"bsp_bank",7:"shared_core_domain",8:"cache_owner"}[operation],
                "predicate":contexts[0],"processor":None if processor==0xffffffff else processor,
                "apic_id":None if r[3]==0xffffffff else r[3],"processor_count":count,
                "register_or_address":f"0x{contexts[1]:016x}","observed":f"0x{contexts[2]:016x}",
                "expected_or_context":f"0x{contexts[3]:016x}","original_status_or_code":f"0x{contexts[4]:016x}",
                "operands_truncated":False,"runtime_activation_claim":False,
                **admission_semantics(operation,contexts[0],contexts[1],contexts[2],contexts[3])}
    elif any(r):
        raise ValueError("unpublished perCPU bank contains data")
    return result

def admission_semantics(operation, predicate, item, observed=None, expected=None):
    """Names describe the exact source predicate; context is not an equality target."""
    result = {"predicate_name": "unmapped_predicate", "predicate_known": False,
              "failure_origin": "software_rejection", "comparison": "predicate_specific",
              "expected_field_role": "context", "status_field_role": "source_failure_code"}
    if operation == 2:
        names = {1: "locate_mp_protocol", 2: "mp_protocol_pointer", 3: "initial_processor_counts",
                 4: "initial_bsp_identity", 5: "processor_inventory_bounds", 6: "initial_processor_info",
                 7: "processor_status_flags", 8: "duplicate_firmware_processor_id", 9: "startup_this_ap",
                 10: "callback_completion", 11: "callback_who_am_i", 12: "callback_wrong_processor",
                 13: "firmware_hardware_identity", 14: "final_processor_counts", 15: "final_bsp_identity",
                 16: "processor_inventory_changed", 17: "final_processor_info", 18: "processor_info_changed",
                 19: "cpuid_identity_profile", 20: "physical_address_width", 21: "topology_leaf_available"}
        result.update(predicate_name=names.get(predicate, "unmapped_mp_predicate"),
                      predicate_known=predicate in names, status_field_role="efi_status")
        if predicate in (1, 3, 4, 6, 9, 11, 14, 15, 17):
            result.update(failure_origin="firmware_service", comparison="status_not_success")
        elif predicate in (7, 10, 12, 13, 16, 18):
            result.update(comparison="equality", expected_field_role="expected_value")
        if predicate == 5:
            bounds = {0: ("processor_count_nonzero", "minimum"), 1: ("processor_count_limit", "maximum"),
                      2: ("all_processors_enabled", "expected_value"), 3: ("bsp_number_in_inventory", "exclusive_upper_bound")}
            name, role = bounds.get(item, ("unmapped_inventory_bound", "context"))
            result.update(predicate_name=name, expected_field_role=role)
        if predicate == 18:
            result["compared_field"] = {0: "processor_id", 1: "status_flag", 2: "package", 3: "core", 4: "thread"}.get(item)
        if predicate == 20:
            result.update(comparison="inclusive_range", expected_field_role="minimum_low32_and_maximum_high32")
    elif operation == 4:
        names = {1: ("topology_maximum_leaf", "at_least", "minimum"),
                 2: ("topology_extension_feature", "required_set_mask", "required_mask"),
                 3: ("processor_signature", "equality", "expected_value"),
                 4: ("physical_address_width", "equality", "expected_value"),
                 5: ("mtrr_capability", "equality", "expected_value"),
                 6: ("syscfg_reserved_or_encryption_bits", "allowed_set_mask", "allowed_mask"),
                 7: ("hwcr_cache_profile", "masked_equality_mask_0x18", "masked_expected_value"),
                 8: ("fixed_attribute_visibility_readback", "equality", "expected_value"),
                 9: ("fixed_attribute_visibility_restore", "equality", "expected_value"),
                 10: ("mtrr_default_unchanged", "equality", "previous_value"),
                 11: ("cache_capture_unique_slot", "unique_sample", "processor_count"),
                 20: ("active_fixed_mtrr_extended_tuple", "each_byte_in_APM2_Table_7_13", "unused"),
                 21: ("active_fixed_mtrr_extensions_enabled", "required_set_mask", "required_mask")}
        if predicate in names:
            name, comparison, role = names[predicate]
            result.update(predicate_name=name, predicate_known=True, comparison=comparison, expected_field_role=role)
        if predicate == 7:
            result["comparison_mask"] = "0x0000000000000018"
    elif operation in (6, 7, 8):
        names = {11: "cache_capture_complete", 12: "physical_bank_agreement", 13: "topology_domain_shape",
                 14: "topology_apic_identity", 15: "duplicate_apic_identity", 16: "package_width_agreement",
                 17: "threads_per_core_agreement", 18: "node_identity_agreement", 19: "shared_core_member_count"}
        result.update(predicate_name=names.get(predicate, "unmapped_cache_predicate"), predicate_known=predicate in names)
        if predicate in (12, 14, 16, 17, 18):
            result.update(comparison="equality", expected_field_role="expected_value")
        if predicate == 12 and item == 0xc0010010:
            result.update(comparison="equality_after_clearing_SYS_CFG19", operands_normalized=True)
        if predicate == 13:
            result.update(observed_field_role="CPUID_80000008_ECX_high32_and_8000001E_EBX_low32",
                          expected_field_role="maximum_threads_per_core")
        if predicate == 19:
            result.update(comparison="population_count", observed_field_role="member_bitmap", expected_field_role="thread_count")
    elif operation == 5 and predicate == 1:
        name = "processor_count" if item == 0 else "bsp_number" if item == 1 else "processor_apic_identity"
        result.update(predicate_name=name, predicate_known=True, comparison="equality", expected_field_role="expected_value")
    elif operation in (1, 3):
        helper_names = {38: "vm_cr_init_redirect", 42: "startup_lapic_profile", 48: "cache_processor_slot",
            49: "cache_observation_seed", 101: "native_cpuid_identity", 102: "native_cpuid_features",
            103: "native_encryption_cpuid_profile", 104: "native_physical_address_width",
            105: "variable_mtrr_count", 106: "native_paging_controls", 107: "mapped_span_admission",
            108: "paging_table_ram_and_wb", 109: "paging_table_cache_bits", 110: "identity_mapping",
            111: "mapping_writable", 112: "mapping_executable", 113: "mapping_physical_wb",
            114: "mapping_pat_wb", 130: "bootstrap_low_page_ram", 131: "bootstrap_mtrrs_enabled",
            132: "bootstrap_fixed_mtrr_capability", 133: "bootstrap_fixed_mtrr_address",
            134: "bootstrap_fixed_mtrr_type", 135: "bootstrap_pat0", 138: "callback_apic_mode",
            139: "lapic_base", 140: "bootstrap_configuration_available", 141: "bootstrap_root_initialize",
            142: "bootstrap_root_span", 143: "bootstrap_root_map_page", 144: "bootstrap_root_map_low_page",
            145: "bootstrap_lapic_uc_pat", 146: "bootstrap_root_map_lapic", 147: "uc_mmio_shape_and_pat0",
            148: "uc_mmio_supervisor", 149: "mmio_effective_uc", 150: "bootstrap_wait_page_permissions",
            151: "vm_cr_svm_disabled", 152: "efer_svm_enabled", 153: "native_syscfg_reserved_or_encryption_bits",
            154: "sev_status_disabled", 155: "tom2_default_profile", 156: "encryption_profile_physical_width",
            157: "callback_x2apic_feature", 158: "callback_x2apic_to_xapic_transition", 238: "callback_paging_agreement",
            601: "context_vmcb_physical", 602: "context_vmcb_virtual", 603: "context_guest_frame",
            604: "host_code_selector", 605: "host_data_selector", 606: "host_task_selector",
            607: "host_cr3_data_range", 608: "host_hsave_data_range", 609: "host_extra_data_range",
            610: "host_owner_data_range", 611: "host_gdtr_data_range", 612: "host_idtr_data_range",
            613: "host_stack_data_range", 614: "dispatch_lower_bound", 615: "dispatch_upper_bound",
            616: "hsave_distinct_from_vmcb", 617: "hsave_distinct_from_auxiliary",
            618: "host_extra_distinct_from_vmcb", 619: "host_extra_distinct_from_auxiliary",
            620: "host_extra_distinct_from_hsave", 621: "host_alias_table_data_range",
            622: "host_alias_physical_address", 623: "host_alias_permissions", 624: "host_alias_pat_wb",
            625: "host_gdt_limit", 626: "host_idt_limit", 627: "host_gdt_data_range",
            628: "host_idt_data_range", 629: "host_tss_type", 630: "host_tss_data_range",
            631: "host_fault_stack_data_range", 632: "host_alias_physical_wb"}
        if predicate in helper_names:
            result.update(predicate_name=helper_names[predicate], predicate_known=True)
        if predicate in (110, 111, 112, 113, 135, 148, 238, 601, 602, 603, 604, 605, 606, 622, 623, 625, 626, 629):
            result.update(comparison="equality", expected_field_role="expected_value")
        if predicate in (616, 617, 618, 619, 620):
            result.update(comparison="not_equal", expected_field_role="address_that_must_be_distinct")
        if predicate in (108, 131, 132):
            result.update(comparison="required_set_mask", expected_field_role="required_mask")
        if predicate == 38:
            result.update(comparison="masked_equality", comparison_mask="0x0000000000000002", expected_field_role="masked_expected_value")
        if predicate == 135:
            result.update(comparison="low8_equality", expected_field_role="pat0_type")
        if predicate == 109:
            result.update(comparison="forbidden_set_mask", expected_field_role="forbidden_mask")
        if predicate in (151, 152):
            result.update(comparison="forbidden_set_mask", expected_field_role="forbidden_mask")
        if predicate == 153:
            result.update(comparison="allowed_set_mask", expected_field_role="allowed_mask")
        if predicate in (150, 154):
            result.update(comparison="equality", expected_field_role="expected_value")
        if predicate == 156:
            result.update(comparison="equality", expected_field_role="expected_value")
        if predicate == 157:
            result.update(comparison="required_set_mask", expected_field_role="required_mask")
        if predicate in (138, 158):
            result.update(comparison="lapic_base_profile" if predicate == 138 else "x2apic_to_xapic_forbidden",
                          expected_field_role="bsp_apic_base")
        if predicate == 103:
            result.update(item_field_role="processor_signature", observed_field_role="CPUID_8000001F_EAX_low32_EBX_high32",
                          expected_field_role="CPUID_8000001F_ECX_low32_EDX_high32")
        if predicate == 104:
            result.update(comparison="inclusive_range" if expected and expected >> 32 else "equality",
                          expected_field_role="minimum_low32_and_maximum_high32" if expected and expected >> 32 else "expected_value")
        if predicate == 155:
            result.update(observed_field_role="TOP_MEM2", expected_field_role="SYS_CFG")
        if predicate in (114, 624, 149):
            result.update(observed_field_role="full_pat_msr", expected_field_role="selected_pat_index")
        if predicate == 114:
            result["comparison"] = "selected_pat_and_pat0_must_be_wb"
        if predicate == 624:
            result["comparison"] = "selected_pat_must_be_wb"
        if predicate == 149:
            result["comparison"] = "combined_mtrr_pat_must_be_uc"
        if predicate == 134:
            result.update(comparison="selected_fixed_byte_equality", expected_field_role="byte_shift_low32_and_required_type_high32")
        if predicate == 105:
            result.update(comparison="low8_count_at_most", expected_field_role="maximum_variable_pairs")
        if predicate in (141, 143, 144, 146):
            result.update(observed_field_role="bootstrap_paging_error_enum", expected_field_role="pat_index" if predicate == 146 else "context")
        if predicate == 101:
            result["compared_field"] = {0: "CPUID_0_EAX", 1: "CPUID_0_EBX", 2: "CPUID_0_EDX", 3: "CPUID_0_ECX", 4: "CPUID_80000000_EAX"}.get(item)
            result.update(comparison="at_least" if item in (0, 4) else "equality", expected_field_role="minimum" if item in (0, 4) else "expected_value")
        if predicate == 102:
            result["compared_field"] = {0: "CPUID_1_ECX", 1: "CPUID_1_EDX", 2: "CPUID_80000001_ECX",
                3: "CPUID_80000001_EDX", 4: "CPUID_8000000A_EDX", 5: "CPUID_8000000A_EBX", 6: "CPUID_8000000A_EAX"}.get(item)
            result["comparison"] = "forbidden_hypervisor_bit" if item == 0 else "at_least" if item == 5 else "equality" if item == 6 else "required_set_mask"
        if predicate == 106:
            result["compared_field"] = {0: "CR0", 3: "CR3", 4: "CR4"}.get(item)
            result["comparison"] = "native_control_admission"
        if predicate in (*range(607, 614), 621, 627, 628, 630, 631):
            result.update(comparison="retained_data_range_and_alignment", expected_field_role="memory_end_or_stack_underflow_bound")
        reason = (predicate - 400) % 16
        level = (predicate - 400) // 16
        walk_names = {1: "unsupported_physical_width", 2: "five_level_paging", 3: "noncanonical_address",
                      4: "invalid_cr3", 5: "unreadable_table", 6: "entry_not_present", 7: "reserved_entry",
                      8: "unsupported_entry_bits", 9: "one_gib_page", 10: "incomplete_walk"}
        if 0 <= level <= 4 and reason in walk_names:
            result.update(predicate_name=walk_names[reason], predicate_known=True, paging_level=level,
                          comparison="paging_walk_rejection", expected_field_role="requested_virtual_address")
            result["observed_field_role"] = {1: "physical_address_width", 2: "five_level_flag",
                3: "noncanonical_virtual_address", 4: "CR3", 5: "unavailable",
                6: "paging_entry", 7: "paging_entry", 8: "paging_entry", 9: "paging_entry",
                10: "last_entry_or_unavailable"}[reason]
            if reason == 5:
                result.update(observed=None, observed_value_available=False,
                              observed_encoded_placeholder=f"0x{observed:016x}" if observed is not None else None)
    return result

def bind_admission_failure(decoded,percpu):
    """Bind preparation/survey evidence to this boot; old sticky faults stay historical."""
    observation=decoded.get("native_resident_observation") or {}
    preparation = (decoded.get("phase") == 20 and observation.get("stage") == 19
                   and observation.get("reason") == 32)
    survey = (decoded.get("phase") == 19 and observation.get("stage") == 0x80
              and observation.get("activation_failure") == 48)
    if not observation.get("encoding_valid") or not (preparation or survey):
        return None
    matches=[]
    for frame in percpu["frames"]:
        record=frame.get("record") or {}
        if record.get("event")==12 and record.get("boot_id")==decoded.get("boot_id") and all(
            frame[key]==decoded.get(key) for key in ("fpga_build_id","rom_build_id")):
            value=record["admission_failure"]
            compact=observation.get("address",0)
            if ((preparation and compact==value["operation"]<<32|value["predicate"])
                    or (survey and value["operation"] in (4,6,7,8))):
                matches.append((frame["kind"], value))
    if not matches:
        return {"status": "full_record_unavailable", "runtime_activation_claim": False}
    # One BSP publisher emits one failure. Disagreeing records from the same
    # candidate and boot cannot be resolved by ordering different CPU clocks.
    values = {json.dumps(value, sort_keys=True) for _, value in matches}
    if len(values) != 1:
        return {"status": "conflicting_full_records", "matching_record_count": len(matches),
                "runtime_activation_claim": False}
    kind, value = next((pair for pair in matches if pair[0] == "last_progress"), matches[0])
    return {**value, "source_bank_kind": kind, "matching_record_count": len(matches),
            "ownership_boundary": "post_ebs_cache_survey" if survey else "firmware_preparation"}


def percpu_snapshots(text: str, manifest: dict | None = None) -> dict:
    """Keep latest individually atomic frames; never require all CPUs to stop."""
    banks={}; rejected=0; identity=None
    requested=set(); matched=set(); current_request=None; frame_count=0
    for line in text.splitlines():
        if line.strip().startswith("CPU_REQUEST:"):
            token=line.strip().split(":",1)[1]
            if not token.isdecimal() or not 0<=int(token)<64:
                current_request=None
            else:
                current_request=int(token);requested.add(current_request)
            continue
        if not line.strip().startswith("CPU_SNAPSHOT:"): continue
        frame_count+=1
        try:
            decoded=decode_percpu_frame(line.strip().split(":",1)[1].strip(),manifest)
        except ValueError:
            rejected+=1; continue
        current_identity=(decoded["fpga_build_id"],decoded["rom_build_id"])
        if identity is not None and current_identity!=identity:
            rejected+=1; continue
        identity=current_identity
        banks[decoded["bank"]]=decoded
        if decoded["bank"]==current_request:matched.add(current_request)
    return {"requested_banks":sorted(requested) if requested else None,
        "unmatched_requested_banks":sorted(requested-matched) if requested else None,
        "frame_count":frame_count,
        "capture_status":"complete" if len(banks)==64 and not requested-matched else "partial" if banks else "not_available",
        "frames":[banks[k] for k in sorted(banks)],"missing_banks":[k for k in range(64) if k not in banks],
        "rejected_frames":rejected,"all_banks_read":len(banks)==64,
        "simultaneous_capture":False,"clock_loss_limit":"A user-clock stop before background CDC delivery can leave an older checkpoint."}

def main():
    parser = argparse.ArgumentParser(description=__doc__)
    modes = parser.add_mutually_exclusive_group(required=True)
    modes.add_argument("--input", type=Path, help="decode retained OpenOCD output offline")
    modes.add_argument("--live", action="store_true", help="read USER2 through the programming USB")
    parser.add_argument("--manifest", type=Path, help="optional expected candidate manifest for explicit build matching")
    args = parser.parse_args()
    manifest = json.loads(args.manifest.read_text(encoding="utf-8-sig")) if args.manifest else None
    if manifest is not None and not all(k in manifest for k in ("fpga_build_id", "rom_build_id")):
        parser.error("an explicit manifest must contain both expected build IDs")
    output_dir = None
    if args.live:
        root = Path(__file__).resolve().parents[2]
        output_dir = root / "target/firmware/squirrel/snapshots" / uuid.uuid4().hex
        output_dir.mkdir(parents=True)
        command = [str(root / "target/firmware/tools/openocd/bin/openocd.exe"),
                   "-f", str(Path(__file__).resolve().parent / "openocd/read_snapshot.cfg")]
        try:
            result = subprocess.run(command, cwd=root, stdout=subprocess.PIPE,
                                    stderr=subprocess.STDOUT, timeout=60, check=False)
        except subprocess.TimeoutExpired as exc:
            (output_dir / "openocd.log").write_bytes(exc.stdout or b"")
            parser.exit(1, f"Reader timed out; partial output retained in {output_dir}\n")
        (output_dir / "openocd.log").write_bytes(result.stdout)
        if result.returncode:
            parser.exit(1, f"OpenOCD failed ({result.returncode}); output retained in {output_dir}\n")
        text = result.stdout.decode("utf-8", errors="replace")
    else:
        text = args.input.read_text(encoding="utf-8-sig")
    try:
        percpu = percpu_snapshots(text, manifest)
        try:
            decoded = consistent_snapshot(text, manifest)
        except ValueError as legacy_error:
            if not percpu["frames"]:
                raise
            decoded = {"legacy_snapshot_error":str(legacy_error),
                "fpga_build_id":percpu["frames"][0]["fpga_build_id"],
                "rom_build_id":percpu["frames"][0]["rom_build_id"]}
        decoded["percpu_diagnostics"] = percpu
        decoded["processor_admission_failure"] = bind_admission_failure(decoded,percpu)
    except ValueError as exc:
        parser.exit(1, f"{exc}" + (f"; raw output: {output_dir}" if output_dir else "") + "\n")
    serialized = json.dumps(decoded, indent=2) + "\n"
    if output_dir:
        (output_dir / "snapshot.json").write_text(serialized, encoding="utf-8")
        if manifest is not None:
            (output_dir / "expected-manifest.json").write_text(json.dumps(manifest, indent=2), encoding="utf-8")
    print(serialized, end="")


if __name__ == "__main__":
    main()
