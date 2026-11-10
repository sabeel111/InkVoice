"""Chatterbox (Resemble AI, 0.5B, MIT) via ONNX Runtime — no PyTorch.
Model: huggingface.co/onnx-community/chatterbox-ONNX (4-bit language model), downloaded to
~/.local/share/bookreader/models/chatterbox. Inference follows the model card's reference script.

--voice default        the bundled voice clip
--voice /path/x.wav    clone any ~10 s clip (mono or stereo WAV, any sample rate)
--exaggeration 0.5     emotional intensity (0.5 neutral, 0.7+ dramatic; higher also speaks faster)
"""
import os, sys
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import common

import numpy as np

ap = common.base_args("default")
ap.add_argument("--exaggeration", type=float, default=0.5)
ap.add_argument("--threads", type=int, default=0, help="CPU threads (0 = physical cores)")
ap.set_defaults(max_chars=300)  # Chatterbox is happiest with a sentence or two at a time
args = ap.parse_args()

SR = 24000
START_SPEECH, STOP_SPEECH = 6561, 6562
LAYERS, KV_HEADS, HEAD_DIM = 30, 16, 64
DATA = os.path.join(os.environ.get("XDG_DATA_HOME", os.path.expanduser("~/.local/share")), "bookreader")
MODEL = os.path.join(DATA, "models", "chatterbox")


def read_wav(path):
    """Mono float32 at 24 kHz from a PCM16 / PCM32 / float32 WAV (no librosa needed)."""
    b = open(path, "rb").read()
    assert b[:4] == b"RIFF" and b[8:12] == b"WAVE", "not a WAV file"
    i, fmt, data = 12, None, None
    while i + 8 <= len(b):
        cid, n = b[i:i + 4], int.from_bytes(b[i + 4:i + 8], "little")
        if cid == b"fmt ":
            fmt = b[i + 8:i + 8 + n]
        elif cid == b"data":
            data = b[i + 8:i + 8 + n]
        i += 8 + n + (n & 1)
    tag = int.from_bytes(fmt[0:2], "little")
    ch = int.from_bytes(fmt[2:4], "little")
    sr = int.from_bytes(fmt[4:8], "little")
    bits = int.from_bytes(fmt[14:16], "little")
    if tag == 0xFFFE:  # WAVE_FORMAT_EXTENSIBLE: real tag is in the sub-format GUID
        tag = int.from_bytes(fmt[24:26], "little")
    if tag == 3:
        a = np.frombuffer(data, "<f4" if bits == 32 else "<f8").astype(np.float32)
    elif bits == 16:
        a = np.frombuffer(data, "<i2").astype(np.float32) / 32768
    elif bits == 32:
        a = np.frombuffer(data, "<i4").astype(np.float32) / 2**31
    else:
        raise ValueError(f"unsupported WAV ({bits}-bit, format {tag})")
    a = a.reshape(-1, ch).mean(1)
    if sr != SR:  # linear resample is fine for a voice-conditioning clip
        n = int(len(a) * SR / sr)
        a = np.interp(np.linspace(0, len(a) - 1, n), np.arange(len(a)), a).astype(np.float32)
    return a


def load():
    import onnxruntime as ort
    from tokenizers import Tokenizer

    so = ort.SessionOptions()
    so.intra_op_num_threads = args.threads or max(1, (os.cpu_count() or 2) // 2)
    sess = lambda name: ort.InferenceSession(os.path.join(MODEL, "onnx", name), so, providers=["CPUExecutionProvider"])
    enc = sess("speech_encoder.onnx")
    embed = sess("embed_tokens.onnx")
    lm = sess("language_model_q4.onnx")
    dec = sess("conditional_decoder.onnx")
    tok = Tokenizer.from_file(os.path.join(MODEL, "tokenizer.json"))

    voice = os.path.join(MODEL, "default_voice.wav") if args.voice in ("", "default") else os.path.expanduser(args.voice)
    # the reference voice is encoded once and reused for every sentence
    cond_emb, prompt_token, spk_emb, spk_feat = enc.run(None, {"audio_values": read_wav(voice)[None, :]})
    exag = np.array([args.exaggeration], dtype=np.float32)

    def synth(text):
        ids = np.array([tok.encode(text).ids], dtype=np.int64)
        pos = np.where(ids >= START_SPEECH, 0, np.arange(ids.shape[1])[None, :] - 1)
        emb_in = {"input_ids": ids, "position_ids": pos, "exaggeration": exag}
        # ~15 chars/s of speech at 25 speech tokens/s, with headroom
        max_new = int(min(2000, 60 + len(text) / 15 * 25 * 1.6))
        gen = np.array([[START_SPEECH]], dtype=np.int64)
        past = None
        for i in range(max_new):
            x = embed.run(None, emb_in)[0]
            if i == 0:
                x = np.concatenate((cond_emb, x), axis=1)
                past = {f"past_key_values.{l}.{kv}": np.zeros([1, KV_HEADS, 0, HEAD_DIM], dtype=np.float32)
                        for l in range(LAYERS) for kv in ("key", "value")}
                mask = np.ones((1, x.shape[1]), dtype=np.int64)
            logits, *present = lm.run(None, dict(inputs_embeds=x, attention_mask=mask, **past))
            logits = logits[:, -1, :]
            # repetition penalty 1.2 (as in the reference script), then greedy pick
            prev = np.take_along_axis(logits, gen, axis=1)
            np.put_along_axis(logits, gen, np.where(prev < 0, prev * 1.2, prev / 1.2), axis=1)
            nxt = np.argmax(logits, axis=-1, keepdims=True).astype(np.int64)
            gen = np.concatenate((gen, nxt), axis=-1)
            if nxt[0, 0] == STOP_SPEECH:
                break
            emb_in["input_ids"] = nxt
            emb_in["position_ids"] = np.full((1, 1), i + 1, dtype=np.int64)
            mask = np.concatenate([mask, np.ones((1, 1), dtype=np.int64)], axis=1)
            for j, k in enumerate(past):
                past[k] = present[j]
        speech = gen[:, 1:-1] if gen[0, -1] == STOP_SPEECH else gen[:, 1:]
        speech = np.concatenate([prompt_token, speech], axis=1)
        wav = dec.run(None, {"speech_tokens": speech, "speaker_embeddings": spk_emb, "speaker_features": spk_feat})[0]
        return np.squeeze(wav, axis=0)

    return synth, SR


common.serve(args, load)
