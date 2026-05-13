#!/bin/bash

if [[ ! -e config.ini ]]; then
   echo 'No config.ini found.'
   echo 'You must copy config-example.ini to config.ini and edit before run.'
   exit 1
fi

function check_exists {
    if [[ -z $(which $1 2>/dev/null) ]]; then
      echo "No "$1" found"
      exit 1
    fi
}

check_exists cargo
check_exists sgdisk
check_exists mkfs.fat
check_exists realpath
check_exists mcopy
check_exists mmd

GREETER_EFI_FILE=EFI/BOOT/BOOTX64.efi
OFFSET=1048576
SECTOR_SIZE=512

OUTPUT_IMG=$(test -n "$1" && echo "$1" || echo pba.gptdisk)

pushd "$(dirname "$(realpath "$0")")" || exit 1

cargo fetch || exit 1
RUSTC_MINOR=$(rustc --version | grep -oP '1\.\K[0-9]+')
if [ "$RUSTC_MINOR" -ge 59 ]; then
    find "${CARGO_HOME:-$HOME/.cargo}/git/checkouts/uefi-rs-"* -name '*.rs' \
        -exec sed -i 's/\basm!(/core::arch::asm!(/g' {} +
fi
cargo b --release || exit 1

# 1mb for gpt stuff & align,
# 1 remaining is more than enough for our image + config.ini + remaining gpt stuff
dd if=/dev/zero of="$OUTPUT_IMG" bs=1M count=2

function error {
  rm "$OUTPUT_IMG"
  echo -e "\nFailed to build the PBA image\n"
  exit 1
}

sgdisk -n 1:0:0 -t 1:ef00 "$OUTPUT_IMG" || error

mkfs.fat --offset $(("$OFFSET" / "$SECTOR_SIZE")) "${OUTPUT_IMG}" || error

export MTOOLS_SKIP_CHECK=1
mmd -i "${OUTPUT_IMG}@@${OFFSET}" ::/EFI ::/EFI/BOOT || error
mcopy -i "${OUTPUT_IMG}@@${OFFSET}" \
    target/x86_64-unknown-uefi/release/opal-uefi-greeter.efi \
    "::/EFI/BOOT/BOOTX64.efi" || error
mcopy -i "${OUTPUT_IMG}@@${OFFSET}" config.ini ::/config.ini || error

echo
echo "Built the PBA image successfully"
echo "Load it using: 'sedutil-cli --loadPBAimage <password> ${OUTPUT_IMG} <drive>'"
echo
