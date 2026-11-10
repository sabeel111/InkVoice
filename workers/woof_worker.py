"""Underdog Woof TTS worker. Protocol and pacing: see common.py.
Model: huggingface.co/ConwayResearch/Underdog-Woof (numpy runtime, 24 kHz), downloaded by setup.sh
to ~/.local/share/bookreader/models/woof. Voices: juno, mira. License: non-commercial.

Expressiveness knobs:
  --temp         sampling temperature (Woof default 0.5; higher = livelier, slightly riskier)
  --clause-max   Woof speaks each sentence separately and cuts sentences longer than this many
                 characters at commas. Its default (120) suits short assistant replies; novels
                 read better with whole sentences kept together (one piece may last up to 30 s).
"""
import os, sys
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import common

ap = common.base_args("juno")
ap.add_argument("--temp", type=float, default=0.5)
ap.add_argument("--clause-max", type=int, default=120)
ap.add_argument("--eos", type=float, default=0.9, help="end-of-speech threshold (higher = lets sentences run longer)")
ap.add_argument("--retries", type=int, default=3, help="re-takes when a piece stalls or runs long")
args = ap.parse_args()


def load():
    import numpy as np
    data = os.path.join(os.environ.get("XDG_DATA_HOME", os.path.expanduser("~/.local/share")), "bookreader")
    sys.path.insert(0, os.path.join(data, "models", "woof"))
    from underdog_woof import Woof, SR
    from underdog_woof.textsplit import split_text

    tts = Woof(voice=args.voice.lower(), temp=args.temp, eos=args.eos, retries=args.retries)

    def synth(text):
        # same as Woof.__call__, but with our own sentence/clause splitting limit
        if tts.norm is not None:
            text = tts.norm(text)
        pieces = split_text(text, clause_max=args.clause_max) or [text]
        return np.concatenate([tts._piece(p) for p in pieces]).astype(np.float32)

    return synth, SR


common.serve(args, load)
