#!/bin/sh
# Builds vortex for NovaOS (i686 ring-3 ELF32 executable)
# and installs it into NovaOS fixtures as VORTEX.ELF, TORRENT.ELF, and VORTEX.PKG.
set -e
cd "$(dirname "$0")"

NOVAOS_ROOT="${NOVAOS_DIR:-/home/firoze/projects/others/novaos}"
if [ ! -d "$NOVAOS_ROOT" ]; then
    echo "Error: NovaOS directory not found at $NOVAOS_ROOT" >&2
    exit 1
fi

RUST_SYSROOT="$NOVAOS_ROOT/tools/rust-sysroot"
LIBRUST="$RUST_SYSROOT/sysroot/lib/rustlib/i686-novaos/lib"
if [ ! -f "$LIBRUST/libcore.rlib" ]; then
    echo "Building NovaOS Rust sysroot..."
    "$RUST_SYSROOT/build-sysroot.sh"
fi

LIBC="$NOVAOS_ROOT/userland/libc"
CC="gcc -m32 -ffreestanding -fno-stack-protector -fno-pie -Wall -Wextra -I$LIBC/include"

nasm -f elf32 "$LIBC/crt0.asm" -o crt0.o
$CC -c "$LIBC/syscall.c" -o syscall.o
$CC -c "$LIBC/stdlib.c" -o stdlib.o
$CC -c "$LIBC/string.c" -o string.o

echo "Compiling Vortex BitTorrent & Stream Studio for NovaOS (pure Rust i686)..."
RUSTC_CMD="${RUSTC:-rustc}"
BOOTSTRAP_ENV="RUSTC_BOOTSTRAP=1"

env $BOOTSTRAP_ENV $RUSTC_CMD --edition 2021 -Z unstable-options \
    --target "$RUST_SYSROOT/i686-novaos.json" \
    --crate-type bin -C panic=abort -C opt-level=2 \
    --emit obj=vortex_main.o \
    -L "$LIBRUST" \
    --extern core="$LIBRUST/libcore.rlib" \
    --extern compiler_builtins="$LIBRUST/libcompiler_builtins.rlib" \
    -C link-dead-code=no \
    vortex.rs

ld -m elf_i386 -Ttext=0x08048000 --entry=_start -static \
    -o vortex.elf crt0.o vortex_main.o syscall.o string.o stdlib.o \
    "$LIBRUST/libcore.rlib" "$LIBRUST/libcompiler_builtins.rlib"

mkdir -p "$NOVAOS_ROOT/tools/fixtures"
cp vortex.elf "$NOVAOS_ROOT/tools/fixtures/VORTEX.ELF"
cp vortex.elf "$NOVAOS_ROOT/tools/fixtures/TORRENT.ELF"

# Generate VORTEX.PKG package metadata fixture
cat << 'EOF' > "$NOVAOS_ROOT/tools/fixtures/VORTEX.PKG"
[package]
id = "org.novaos.vortex"
name = "Vortex Studio"
version = "1.0.0"
category = "Internet"
app_filename = "VORTEX.ELF"
summary = "BitTorrent client & IDM stream download accelerator"
EOF

echo "✅ Built and installed VORTEX.ELF, TORRENT.ELF, and VORTEX.PKG into $NOVAOS_ROOT/tools/fixtures/"
