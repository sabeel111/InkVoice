#!/bin/sh
# One-line install of the latest InkVoice release:
#   curl -fsSL https://raw.githubusercontent.com/sabeel111/InkVoice/main/packaging/linux/get.sh | sh
set -e
if [ -t 1 ]; then A="$(printf '\033[38;2;222;166;88m')"; D="$(printf '\033[2m')"; B="$(printf '\033[1m')"; R="$(printf '\033[0m')"; else A= D= B= R=; fi

printf '\n  %s📖 InkVoice%s  %sbooks that read themselves%s\n\n' "$A$B" "$R" "$D" "$R"
case "$(uname -m)" in
  x86_64|amd64) ARCH=x86_64 ;;
  *) echo "  Sorry — prebuilt InkVoice is only available for x86_64 Linux for now." >&2
     echo "  Build it from source: https://github.com/sabeel111/InkVoice" >&2; exit 1 ;;
esac
command -v curl >/dev/null || { echo "  curl is needed to install InkVoice." >&2; exit 1; }

URL="https://github.com/sabeel111/InkVoice/releases/latest/download/inkvoice-linux-$ARCH.tar.gz"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT
printf '  %s→%s Downloading the app\n' "$A" "$R"
curl -fL --progress-bar "$URL" -o "$TMP/inkvoice.tar.gz"
tar -xzf "$TMP/inkvoice.tar.gz" -C "$TMP"
printf '  %s→%s Installing for %s\n' "$A" "$R" "$(id -un)"
sh "$TMP/inkvoice-linux-$ARCH/install.sh" | sed 's/^/    /'
printf '\n  %s✔ Done!%s Open %sInkVoice%s from your app launcher, then pick a voice in Settings.\n\n' "$A$B" "$R" "$B" "$R"
