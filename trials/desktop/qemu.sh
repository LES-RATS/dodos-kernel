#!/bin/sh
# Boots the measurement image under QEMU (x86-64 emulation) with a FAT
# directory as its stick, until the image powers the machine off.
# Usage: trials/desktop/qemu.sh OUT_DIR [CONFIG_FILE] [extra QEMU args...]
# Timings under emulation are meaningless; this checks that every path runs.
set -eu
here=$(cd "$(dirname "$0")" && pwd)
out=$1
shift
config=${1:-$here/qemu-config.txt}
[ $# -gt 0 ] && shift
qemu=${QEMU:-/opt/homebrew/bin/qemu-system-x86_64}
share=$(dirname "$qemu")/../share/qemu
(cd "$here/image" && cargo build --release)
rm -rf "$out"
mkdir -p "$out"
# A 64 MB FAT32 disk image without partitions, written with mtools.
mt="nix shell nixpkgs#mtools -c"
dd if=/dev/zero of="$out/stick.img" bs=1048576 count=64 2>/dev/null
$mt mformat -i "$out/stick.img" -F -v DODOS ::
$mt mmd -i "$out/stick.img" ::/EFI ::/EFI/BOOT ::/DODOS
$mt mcopy -i "$out/stick.img" "$here/image/target/x86_64-unknown-uefi/release/dodos-trial-desktop.efi" ::/EFI/BOOT/BOOTX64.EFI
$mt mcopy -i "$out/stick.img" "$config" ::/DODOS/CONFIG.TXT
cp "$share/edk2-i386-vars.fd" "$out/vars.fd"
"$qemu" -machine q35 -cpu max -smp "${SMP:-12,sockets=1,cores=6,threads=2}" -m 2G \
	-drive if=pflash,format=raw,readonly=on,file="$share/edk2-x86_64-code.fd" \
	-drive if=pflash,format=raw,file="$out/vars.fd" \
	-drive file="$out/stick.img",format=raw,media=disk \
	-serial file:"$out/serial.log" -display none -monitor none \
	"$@"
# Copy the results out of the disk image.
$mt mcopy -s -i "$out/stick.img" ::/DODOS "$out/"
