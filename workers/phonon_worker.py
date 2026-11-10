"""Phonon (Gradium) TTS worker. Protocol and pacing: see common.py.
List voices: <InkVoice data>/venv/bin/python -m ptts --list-voices
"""
import os, sys
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import common

ap = common.base_args("Everly")
ap.add_argument("--temperature", type=float, default=0.3, help="sampling temperature (Phonon default 0.3)")
ap.add_argument("--quant", default="f32", help="weight format: f32 (best) or q8_0 (faster)")
args = ap.parse_args()


def load():
    import ptts
    tts = ptts.TTS(lang="en", voice=args.voice, temperature=args.temperature, quant=args.quant)
    return (lambda text: tts.synth(text)), tts.sample_rate


common.serve(args, load)
