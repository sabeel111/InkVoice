"""Shared plumbing for BookReader TTS workers.

Protocol (JSON lines):
  stdin : {"id": 1, "text": "...", "out": "/path/c0001.wav"}
  stdout: {"id": 1, "progress": 0.42}            (zero or more)
          {"id": 1, "ok": true, "stream_offset": s} | {"id": 1, "error": "..."}
  stream mode: request adds "stream": true, "start_char": n; see synthesize()

Output: <out>.wav plus <out>.json = {"duration": s, "words": [[char_start, char_end, t0, t1], ...]}
where char offsets are Unicode code points into the request text. Files are written to
temporaries and renamed, so a .wav on disk is always complete.

Pacing: whole paragraphs go to the model in one call (natural intonation), the model's edge
silence is trimmed, pauses are inserted deliberately, and long internal silences are shortened.

Word timing is estimated, not model-reported: inside each synthesized piece, punctuation is
matched to the pauses actually detected in the audio, and words are interpolated between
those anchors by character length.
"""
import argparse, json, os, re, sys, wave
import numpy as np

# Keep stdout clean for the protocol: anything libraries print goes to stderr.
_PROTO = sys.stdout
sys.stdout = sys.stderr

WORD = re.compile(r"\S+")
PARA_BREAK = re.compile(r"\n\s*\n")
SENT_END_WORD = re.compile(r"[.!?…][\"”’)\]]*$")
BREAK_WORD = re.compile(r"[.!?…,;:—–][\"”’)\]]*$")


def data_dir():
    """InkVoice's data folder (passed by the app; falls back to the Linux default)."""
    return os.environ.get("INKVOICE_DATA") or os.path.join(
        os.environ.get("XDG_DATA_HOME", os.path.expanduser("~/.local/share")), "inkvoice")


def base_args(default_voice):
    ap = argparse.ArgumentParser()
    ap.add_argument("--voice", default=default_voice)
    ap.add_argument("--sentence-pause", type=float, default=0.30, help="gap between pieces of one paragraph (s)")
    ap.add_argument("--para-pause", type=float, default=0.60, help="gap between paragraphs (s)")
    ap.add_argument("--max-pause", type=float, default=0.45, help="shorten internal silences longer than this (s)")
    ap.add_argument("--max-chars", type=int, default=700, help="split paragraphs longer than this at sentence ends")
    return ap


def emit(obj):
    _PROTO.write(json.dumps(obj) + "\n")
    _PROTO.flush()


def split_pieces(text, max_chars):
    """-> list of (word_indices, ends_paragraph), plus the word list [(start, end, word)]."""
    words = [(m.start(), m.end(), m.group()) for m in WORD.finditer(text)]
    paras, cur = [], []
    for i, (s, _, _) in enumerate(words):
        if cur and PARA_BREAK.search(text, words[i - 1][1], s):
            paras.append(cur)
            cur = []
        cur.append(i)
    if cur:
        paras.append(cur)

    pieces = []
    for para in paras:
        chunks, chunk, size = [], [], 0
        for i in para:
            chunk.append(i)
            size += len(words[i][2]) + 1
            if size > max_chars and SENT_END_WORD.search(words[i][2]):
                chunks.append(chunk)
                chunk, size = [], 0
        if chunk:
            chunks.append(chunk)
        for k, c in enumerate(chunks):
            pieces.append((c, k == len(chunks) - 1))
    return pieces, words


def _frames_quiet(a, sr, thr=0.01):
    hop = sr // 100
    n = len(a) // hop
    if n == 0:
        return np.zeros(0, dtype=bool), hop
    return np.sqrt((a[: n * hop].reshape(n, hop) ** 2).mean(1)) < thr, hop


def trim(a, sr, keep=0.03):
    q, hop = _frames_quiet(a, sr)
    voiced = np.nonzero(~q)[0]
    if len(voiced) == 0:
        return a[:0]
    k = int(keep * sr)
    return a[max(0, voiced[0] * hop - k): min(len(a), (voiced[-1] + 1) * hop + k)]


def cap_pauses(a, sr, max_s):
    q, hop = _frames_quiet(a, sr)
    max_f = int(max_s * 100)
    keep, i, n, last = [], 0, len(q), 0
    while i < n:
        if not q[i]:
            i += 1
            continue
        j = i
        while j < n and q[j]:
            j += 1
        if j - i > max_f:
            keep.append(a[last:(i + max_f // 2) * hop])
            last = (j - (max_f - max_f // 2)) * hop
        i = j
    keep.append(a[last:])
    return np.concatenate(keep)


def pauses(a, sr, min_s=0.12):
    """Interior silent runs as (start_s, end_s)."""
    q, hop = _frames_quiet(a, sr)
    out, i, n = [], 0, len(q)
    while i < n:
        if not q[i]:
            i += 1
            continue
        j = i
        while j < n and q[j]:
            j += 1
        if i > 0 and j < n and (j - i) / 100 >= min_s:
            out.append((i * hop / sr, j * hop / sr))
        i = j
    return out


def align(piece_words, a, sr, t0):
    """Estimate [start, end, t_start, t_end] for each word of one synthesized piece."""
    dur = len(a) / sr
    pos, c = [], 0
    for _, _, w in piece_words:
        pos.append((c, c + len(w)))
        c += len(w) + 1
    total = max(1, c - 1)
    found = pauses(a, sr)
    xs, ys, last_t = [0.0], [0.0], 0.0
    for i, (_, _, w) in enumerate(piece_words[:-1]):
        if not BREAK_WORD.search(w):
            continue
        cpos = pos[i][1]
        ac, at = xs[-1], ys[-1]
        expected = at + (cpos - ac) / max(1, total - ac) * (dur - at)
        tol = max(0.6, 0.12 * dur)
        cands = [p for p in found if p[0] >= last_t and abs((p[0] + p[1]) / 2 - expected) < tol]
        if not cands:
            continue
        p = min(cands, key=lambda p: abs((p[0] + p[1]) / 2 - expected))
        xs += [cpos, pos[i + 1][0]]
        ys += [p[0], p[1]]
        last_t = p[1]
    xs.append(total)
    ys.append(dur)
    out = []
    for (cs, ce, _), (p0, p1) in zip(piece_words, pos):
        out.append([cs, ce, round(t0 + float(np.interp(p0, xs, ys)), 3), round(t0 + float(np.interp(p1, xs, ys)), 3)])
    return out


def _atomic_write(path, write):
    tmp = path + ".part"
    write(tmp)
    os.replace(tmp, path)


def synthesize(req_id, text, out, synth_piece, sr, args, stream=False, start_char=0):
    """Synthesize `text` to `out` (.wav + .json).

    stream=True: generation starts at the piece containing `start_char`, and every finished piece
    is appended as raw 16-bit PCM to `<out>.stream.pcm` and announced with a progress message
    carrying {"chunk": {"samples": n, "words": [...stream-relative timings...]}}, so the app can
    start playing long before the whole chapter is done. Pieces before `start_char` are made
    afterwards; the final file is always assembled in reading order.
    """
    pieces, words = split_pieces(text, args.max_chars)
    if not pieces:
        raise ValueError("empty text")
    k = 0
    if stream and start_char > 0:
        for i, (idx, _) in enumerate(pieces):
            if words[idx[0]][0] <= start_char:
                k = i
    order = list(range(k, len(pieces))) + list(range(0, k))
    texts = [" ".join(words[i][2] for i in idx) for idx, _ in pieces]
    total = sum(len(t) for t in texts)

    pcm_path = out + ".stream.pcm"
    pcm_file = open(pcm_path, "wb") if stream else None
    if stream:
        emit({"id": req_id, "stream": pcm_path, "sr": sr})

    made = {}  # piece index -> (audio, gap, piece-relative word timings)
    stream_t, done = 0.0, 0
    for n, i in enumerate(order):
        idx, end_para = pieces[i]
        pw = [words[j] for j in idx]
        a = np.asarray(synth_piece(texts[i]), dtype=np.float32).reshape(-1)
        a = cap_pauses(trim(a, sr), sr, args.max_pause)
        rel = align(pw, a, sr, 0.0)
        gap = np.zeros(int(sr * (args.para_pause if end_para else args.sentence_pause)), dtype=np.float32)
        made[i] = (a, gap, rel)
        done += len(texts[i])
        msg = {"id": req_id, "progress": round(done / total, 3)}
        if pcm_file and i >= k:  # first pass: stream in playback order
            chunk = (np.clip(np.concatenate([a, gap]), -1, 1) * 32767).astype("<i2")
            pcm_file.write(chunk.tobytes())
            pcm_file.flush()
            msg["chunk"] = {
                "samples": len(chunk),
                "words": [[cs, ce, round(t0 + stream_t, 3), round(t1 + stream_t, 3)] for cs, ce, t0, t1 in rel],
            }
            stream_t += len(chunk) / sr
        emit(msg)
    if pcm_file:
        pcm_file.close()

    parts, timings, t, stream_offset = [], [], 0.0, 0.0
    for i in range(len(pieces)):
        a, gap, rel = made[i]
        if i == k:
            stream_offset = t
        timings += [[cs, ce, round(t0 + t, 3), round(t1 + t, 3)] for cs, ce, t0, t1 in rel]
        parts += [a, gap]
        t += (len(a) + len(gap)) / sr
    pcm = (np.clip(np.concatenate(parts), -1, 1) * 32767).astype("<i2")

    def write_wav(p):
        with wave.open(p, "wb") as w:
            w.setnchannels(1); w.setsampwidth(2); w.setframerate(sr)
            w.writeframes(pcm.tobytes())

    def write_json(p):
        with open(p, "w") as f:
            json.dump({"duration": round(t, 3), "words": timings}, f)

    _atomic_write(os.path.splitext(out)[0] + ".json", write_json)
    _atomic_write(out, write_wav)
    if stream:
        try:
            os.remove(pcm_path)
        except OSError:
            pass
    return round(stream_offset, 3)


def serve(args, load):
    """load() -> (synth_piece(text) -> float32 array, sample_rate). Errors are reported per request."""
    try:
        synth_piece, sr = load()
        load_err = None
    except Exception as e:  # report on every request instead of dying silently
        synth_piece, sr, load_err = None, 0, f"model failed to load: {e}"
    for line in sys.stdin:
        req = json.loads(line)
        try:
            if load_err:
                raise RuntimeError(load_err)
            offset = synthesize(req["id"], req["text"], req["out"], synth_piece, sr, args,
                                stream=bool(req.get("stream")), start_char=int(req.get("start_char") or 0))
            emit({"id": req["id"], "ok": True, "stream_offset": offset})
        except Exception as e:
            emit({"id": req["id"], "error": str(e)})
