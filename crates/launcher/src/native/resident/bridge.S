/* Native ReadyToBoot callback capture. This object belongs to the original
 * guest-visible DXE image and is required only until the callback returns.
 * Persistent world-switch code is separately linked from runtime.S.
 * Reuse the qualified EFI x64 capture and restoration instructions verbatim.
 * Callback RCX=event, RDX=context; inner receives R8=NativeBoundary.
 * The callback is void: unlike image entry, every incoming GPR is restored.
 */
#define SVMVISOR_RESIDENT_CALLBACK 1
#define SVMVISOR_AP_DIAGNOSTICS 1
#define efi_main svmvisor_resident_callback
#define svmvisor_native_efi_main_inner svmvisor_resident_callback_inner
#include "../admission/boundary.S"
