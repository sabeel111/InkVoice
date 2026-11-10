"""Kokoro 82M (Apache-2.0) via ONNX Runtime — no PyTorch. Protocol and pacing: see common.py.
Model files (github.com/thewh1teagle/kokoro-onnx, release model-files-v1.0), downloaded by setup.sh
to the InkVoice data folder/models/kokoro.
Voices: af_* / am_* American female/male, bf_* / bm_* British female/male (e.g. af_heart, bf_emma).
"""
import os, sys
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import common

ap = common.base_args("af_heart")
ap.add_argument("--quality", choices=["int8", "full"], default="int8",
                help="int8: 92 MB model; full: 326 MB float32 model")
args = ap.parse_args()

DATA = common.data_dir()
MODEL = os.path.join(DATA, "models", "kokoro")


def load():
    from kokoro_onnx import Kokoro
    name = "kokoro-v1.0.int8.onnx" if args.quality == "int8" else "kokoro-v1.0.onnx"
    k = Kokoro(os.path.join(MODEL, name), os.path.join(MODEL, "voices-v1.0.bin"))
    voice = args.voice.lower()
    if voice not in k.get_voices():
        raise ValueError(f"unknown Kokoro voice '{args.voice}'")
    lang = "en-gb" if voice.startswith("b") else "en-us"
    sr_holder = {}

    def synth(text):
        audio, sr = k.create(text, voice=voice, speed=1.0, lang=lang)
        sr_holder["sr"] = sr
        return audio

    return synth, 24000


common.serve(args, load)
