# First-party RTL

`svmvisor_option_rom.sv` is the read-only 4 KiB data source for Expansion ROM
BAR slot 6. `svmvisor_tlps128_bar_rdengine.sv` is the matching read completer;
it preserves First/Last DWORD byte enables so firmware byte probes receive the
correct Completion Byte Count and Lower Address. The build overlays both into
the pinned upstream board controller.

The ROM has the same two-clock response latency as the upstream BAR blocks.
`tools/rompack` writes its `$readmemh` image as little-endian numeric DWORDs;
the completer converts those DWORDs to PCIe wire byte order. Unused addresses
are padded with `0xff`.

Do not copy the complete upstream PCILeech source tree into this directory.
Board support is pinned by `../config.psd1` and fetched into `target/`;
this directory is reserved for the small amount of code owned by svmvisor.
