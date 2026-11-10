#!/bin/sh
# One-line install of the latest InkVoice release:
#   curl -fsSL https://raw.githubusercontent.com/sabeel111/InkVoice/main/packaging/linux/get.sh | sh
set -e
case "$(uname -m)" in
  x86_64|amd64) ARCH=x86_64 ;;
  *) echo "Sorry, prebuilt InkVoice is only available for x86_64 Linux. Build from source: https://github.com/sabeel111/InkVoice" >&2; exit 1 ;;
esac
URL="https://github.com/sabeel111/InkVoice/releases/latest/download/inkvoice-linux-$ARCH.tar.gz"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT
echo "Downloading InkVoice…"
curl -fsSL "$URL" | tar -xz -C "$TMP"
sh "$TMP/inkvoice-linux-$ARCH/install.sh"
