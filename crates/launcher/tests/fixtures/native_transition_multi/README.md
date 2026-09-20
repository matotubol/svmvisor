# Multi-exit guest fixture

`guest.S` independently assembles the frozen 1,792-byte `guest.bin`.
`tests/native_guest_resources.rs` compares all constructor bytes of
`native::resources::guest` with that binary.

Regenerate `guest.bin` only when the guest program deliberately changes:

```powershell
clang --target=x86_64-none-elf -c guest.S -o guest.o
llvm-objcopy -O binary --only-section=.text guest.o guest.bin
```
