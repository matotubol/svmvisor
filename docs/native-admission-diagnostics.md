# Exact processor admission refusal evidence

The physical1b05 candidate returned EFI_UNSUPPORTED at preparation stage19;
Windows continued without activation. That aggregate status did not identify
the failing origin. This change retains the original checked samples and service
statuses without changing admission policy, retrying MSR reads, or activating SVM.

## Ownership and wire record

The existing blocking MP owner publishes each callback result with release/acquire.
The AP records only memory. Firmware dispatch status takes precedence over callback
contents when StartupThisAP fails; incomplete results are never consumed. The BSP
copies the resulting record after the MP call ends. Only its serialized card owner
writes the diagnostic endpoint, after revalidating the UC mappings, PCI identity,
BAR/configuration, build IDs and boot ID. Optional transport failure does not alter
the refusal or erase the compact preparation status.

Preparationv2 stage19/reason32 stores operation in address-high and predicate in
address-low. USER3 event12 carries predicate, item/register/address, observed64,
expected/context64, original EFI status/code64, and processor32/count32. Its header
retains actual APIC ID; unavailable processor/APIC values remain UINT32_MAX. An
unknown identity uses the known BSP transport bank without fabricating identity.
The same payload/commit owner serves resident diagnostics. Only the final sequence
write publishes a record, after all nineteen payload DWORDs and a store fence.

Operations:1 owned bootstrap root;2 MP services/inventory;3 CPU-local mapping and
host closure;4 physical cache capture;5 returned inventory;6 completed/BSP bank;
7 topology/core domain;8 shared owner initialization. The offline decoder names
each predicate and its operand roles. Mask predicates, ranges, forbidden equality,
normalized SYS_CFG19, contextual operands and unavailable PTE reads are explicit.
The MP firmware/software origin distinction preserves original Status even when
the external child status remains EFI_UNSUPPORTED.

The decoder binds extended records to current build/boot and compact stage19 class.
It accepts current progress when sticky first-fault belongs to an older boot;
conflicting current records are ambiguous rather than arbitrarily selected. A
missing extended record leaves the compact failure visible without invented values.

## Validation and limits

Tests exercise actual MP callback publication, AP/BSP wide operands, WhoAmI failure,
no/duplicate callback, inventory changes, and timeout precedence over an already
written AP rejection. Actual commit helpers are tested against failure at every
payload/commit write. Systematic old/new capture and comparator checks preserve
admission decisions and exact RDMSR/WRMSR traces. The exact production card fixture
can wrap the OVMF MP service to inject stage19 failure; this is an external fixture,
never a production feature. It validates the unmodified child/parent compact path;
full USER3 transport uses unit/device-model and offline decoder evidence, not a
physical transport claim.

No timing baseline, cache correctness on silicon, Windows activation, SMM stability,
Hyper-V/VBS compatibility or containment claim follows from these diagnostic tests.
The next physical refusal must be attributed only from its exact new evidence.
