#!/usr/bin/env bash
# Creates the Python venv used by the TTS workers and downloads the voice engines:
#   Phonon (ptts), Underdog Woof (numpy), Kokoro 82M (ONNX Runtime). No PyTorch.
set -euo pipefail
D="${XDG_DATA_HOME:-$HOME/.local/share}/bookreader"
mkdir -p "$D/models/kokoro" "$D/workers"
python3 -m venv "$D/venv"
"$D/venv/bin/pip" install -q --upgrade pip ptts numpy huggingface_hub kokoro-onnx
"$D/venv/bin/python" -c "from huggingface_hub import snapshot_download as s; s('ConwayResearch/Underdog-Woof', local_dir='$D/models/woof')"
K=https://github.com/thewh1teagle/kokoro-onnx/releases/download/model-files-v1.0
for f in kokoro-v1.0.int8.onnx voices-v1.0.bin; do
  [ -f "$D/models/kokoro/$f" ] || curl -fSL -o "$D/models/kokoro/$f" "$K/$f"
done
cp "$(dirname "$0")"/workers/*.py "$D/workers/"
echo "Done. Phonon + Woof + Kokoro ready in $D"
