#!/usr/bin/env bash
set -euo pipefail
export PATH="/usr/bin:/c/Windows/System32"
validation=$(cd "$(dirname "$0")" && pwd)
source_dir="@SOURCE@"
toolchain="@TOOLCHAIN@"
export PATH="$toolchain/mingw64/bin:/usr/bin:/c/Program Files/Git/usr/bin:/c/Windows/System32"
export MSYSTEM=MINGW64
export MSYS2_PATH_TYPE=strict
export PYTHONDONTWRITEBYTECODE=1
export PKG_CONFIG_PATH="$toolchain/mingw64/lib/pkgconfig:$toolchain/mingw64/share/pkgconfig"
export PKG_CONFIG_PREFIX_VARIABLE=prefix
mkdir -p "$validation/build"
cd "$validation/build"
gcc --version
python --version
ninja --version
pkgconf --modversion glib-2.0 pixman-1 zlib
configure_status=0
configure_started=$(date +%s)
"$source_dir/configure" \
 --target-list=x86_64-softmmu \
 --without-default-features \
 --enable-system --enable-tcg --enable-pixman \
 --disable-docs --disable-guest-agent --disable-tools \
 --disable-werror --disable-download --disable-install-blobs \
 --disable-fdt \
 --with-pkgversion=svmvisor-nrip-v1 \
 --prefix="$validation/runtime" || configure_status=$?
if ((configure_status != 0)); then
 test -f build.ninja
 test "$(stat -c %Y build.ninja)" -ge "$configure_started"
 grep -q "ERROR: Postconf script .*symlink-install-tree.py.* failed with exit code 1" meson-logs/meson-log.txt
 export MESONINTROSPECT="$validation/build/pyvenv/bin/meson introspect"
 ./pyvenv/bin/python.exe "$source_dir/scripts/symlink-install-tree.py" > "$validation/postconf-failure.log" 2>&1 || true
 grep -q 'WinError 1314' "$validation/postconf-failure.log"
 grep -q 'os.symlink(source, bundle_dest)' "$validation/postconf-failure.log"
 printf 'Continuing generated Ninja graph after verified optional symlink privilege failure; runtime packaging is explicit.\n'
 python "$validation/standalone_ninja.py"
 ninja -f build-standalone.ninja -j8 qemu-system-x86_64.exe
else
 ninja -j8 qemu-system-x86_64.exe
fi
