#!/bin/sh
# Builds dist/inkvoice-linux-x86_64.tar.gz from target/release/inkvoice.
set -e
cd "$(dirname "$0")/../.."
NAME="inkvoice-linux-$(uname -m)"
rm -rf "dist/$NAME" && mkdir -p "dist/$NAME"
cp target/release/inkvoice packaging/linux/inkvoice.desktop packaging/linux/install.sh "dist/$NAME/"
cp assets/inkvoice.png "dist/$NAME/inkvoice.png"
strip "dist/$NAME/inkvoice" 2>/dev/null || true
chmod +x "dist/$NAME/install.sh"
tar -C dist -czf "dist/$NAME.tar.gz" "$NAME"
rm -rf "dist/$NAME"
echo "dist/$NAME.tar.gz"
