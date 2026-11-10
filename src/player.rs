//! Audio playback with:
//! - a growing sample buffer, so streamed audio can play while the rest is still generated
//! - speed change without pitch change (WSOLA time-stretching, tuned for speech)
//! - positions reported in *book time* (unaffected by speed), so word timings stay valid
use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// What a piece of audio reads: a chapter, or one page of it.
#[derive(Clone, PartialEq, Debug)]
pub struct Unit {
    pub book: String,
    pub ch: usize,
    pub pg: Option<usize>,
}

/// [char_start, char_end, t_start, t_end] — chars index the text that was synthesized.
#[derive(Deserialize, Clone, Copy, Debug)]
#[allow(dead_code)] // t_end kept for future use (e.g. karaoke-style fill)
pub struct Word(pub usize, pub usize, pub f32, pub f32);

#[derive(Deserialize, Default)]
struct Timing {
    #[serde(default)]
    words: Vec<Word>,
}

pub fn timing_path(wav: &Path) -> PathBuf {
    wav.with_extension("json")
}

/// Duration from the WAV header (16-bit PCM written by the workers).
pub fn wav_duration(path: &Path) -> Option<f32> {
    use std::io::Read;
    let mut h = [0u8; 44];
    let mut f = std::fs::File::open(path).ok()?;
    f.read_exact(&mut h).ok()?;
    let byte_rate = u32::from_le_bytes(h[28..32].try_into().ok()?) as f32;
    let len = f.metadata().ok()?.len().saturating_sub(44) as f32;
    (byte_rate > 0.0).then(|| len / byte_rate)
}

/// Read a mono 16-bit PCM WAV (as written by the workers) into f32 samples.
fn read_wav(path: &Path) -> Result<(Vec<f32>, u32), String> {
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    if bytes.len() < 44 || &bytes[0..4] != b"RIFF" {
        return Err("not a WAV file".into());
    }
    let channels = u16::from_le_bytes([bytes[22], bytes[23]]) as usize;
    let sr = u32::from_le_bytes(bytes[24..28].try_into().unwrap());
    // find the "data" chunk
    let mut i = 12;
    while i + 8 <= bytes.len() {
        let id = &bytes[i..i + 4];
        let len = u32::from_le_bytes(bytes[i + 4..i + 8].try_into().unwrap()) as usize;
        if id == b"data" {
            let data = &bytes[i + 8..(i + 8 + len).min(bytes.len())];
            let samples = data
                .chunks_exact(2 * channels.max(1))
                .map(|c| i16::from_le_bytes([c[0], c[1]]) as f32 / 32768.0)
                .collect();
            return Ok((samples, sr));
        }
        i += 8 + len + (len & 1);
    }
    Err("WAV has no data".into())
}

pub fn pcm_to_f32(bytes: &[u8]) -> impl Iterator<Item = f32> + '_ {
    bytes.chunks_exact(2).map(|c| i16::from_le_bytes([c[0], c[1]]) as f32 / 32768.0)
}

// ------------------------------------------------------------------------------------------

/// State shared between the UI thread and the audio thread.
struct Shared {
    samples: Vec<f32>,
    sr: u32,
    /// all audio is in `samples` (no more streaming)
    complete: bool,
    /// read position in `samples` (book time), fractional
    pos: f64,
    speed: f32,
    /// waiting for more streamed audio
    buffering: bool,
    /// seconds that must be buffered ahead before (re)starting
    min_ahead: f32,
    seek_to: Option<f64>,
}

impl Shared {
    fn ahead_secs(&self) -> f32 {
        (self.samples.len() as f64 - self.pos).max(0.0) as f32 / self.sr as f32
    }
}

/// rodio source: WSOLA time-stretch over the shared buffer.
struct Stretch {
    shared: Arc<Mutex<Shared>>,
    sr: u32,
    n: usize,
    hop: usize,
    tol: usize,
    window: Vec<f32>,
    tail: Vec<f32>,
    /// natural continuation of the previous frame (for waveform-similarity search)
    prev_next: Option<usize>,
    out: std::collections::VecDeque<f32>,
}

impl Stretch {
    fn new(shared: Arc<Mutex<Shared>>, sr: u32) -> Self {
        let n = ((sr as f32 * 0.030) as usize) & !1; // 30 ms frames
        let hop = n / 2;
        let tol = (sr as f32 * 0.008) as usize; // ±8 ms search
        let window = (0..n).map(|i| 0.5 - 0.5 * (2.0 * std::f32::consts::PI * i as f32 / n as f32).cos()).collect();
        Self { shared, sr, n, hop, tol, window, tail: vec![0.0; hop], prev_next: None, out: Default::default() }
    }

    /// Produce one hop of output. Returns false when playback is over.
    fn fill(&mut self) -> bool {
        let mut s = self.shared.lock().unwrap();
        if let Some(t) = s.seek_to.take() {
            s.pos = t.clamp(0.0, s.samples.len() as f64);
            self.prev_next = None;
            self.tail.iter_mut().for_each(|x| *x = 0.0);
        }
        let len = s.samples.len();
        let need = self.n + 2 * self.tol;
        let ahead = len as f64 - s.pos;
        if !s.complete {
            // not enough streamed audio: play silence until the buffer refills
            if ahead < need as f64 || (s.buffering && s.ahead_secs() < s.min_ahead) {
                s.buffering = true;
                self.out.extend(std::iter::repeat(0.0).take(self.hop));
                return true;
            }
            s.buffering = false;
        } else if ahead <= 0.0 {
            return false;
        }

        let x = &s.samples;
        let at = |i: usize| x.get(i).copied().unwrap_or(0.0);
        let speed = s.speed;
        let ideal = s.pos as usize;

        if (speed - 1.0).abs() < 0.01 {
            // normal speed: plain copy; right after stretched audio, crossfade out its tail once
            let after_stretch = self.prev_next.is_some();
            for i in 0..self.hop {
                let v = if after_stretch { self.tail[i] + at(ideal + i) * self.window[i] } else { at(ideal + i) };
                self.out.push_back(v);
            }
            self.tail.iter_mut().for_each(|t| *t = 0.0);
            self.prev_next = None;
            s.pos += self.hop as f64;
            return true;
        }

        // WSOLA: pick the frame near `ideal` that best continues the previous one
        let mut best = ideal;
        if let Some(nat) = self.prev_next {
            let lo = ideal.saturating_sub(self.tol);
            let hi = (ideal + self.tol).min(len.saturating_sub(self.n));
            let mut best_c = f32::MIN;
            let mut c = lo;
            while c <= hi {
                let mut acc = 0.0;
                let mut i = 0;
                while i < self.hop {
                    acc += at(c + i) * at(nat + i);
                    i += 2;
                }
                if acc > best_c {
                    best_c = acc;
                    best = c;
                }
                c += 2;
            }
        }
        for i in 0..self.hop {
            self.out.push_back(self.tail[i] + at(best + i) * self.window[i]);
        }
        for i in 0..self.hop {
            self.tail[i] = at(best + self.hop + i) * self.window[self.hop + i];
        }
        self.prev_next = Some(best + self.hop);
        s.pos += self.hop as f64 * speed as f64;
        true
    }
}

impl Iterator for Stretch {
    type Item = f32;
    fn next(&mut self) -> Option<f32> {
        if self.out.is_empty() && !self.fill() {
            return None;
        }
        self.out.pop_front()
    }
}

impl rodio::Source for Stretch {
    fn current_frame_len(&self) -> Option<usize> {
        None
    }
    fn channels(&self) -> u16 {
        1
    }
    fn sample_rate(&self) -> u32 {
        self.sr
    }
    fn total_duration(&self) -> Option<Duration> {
        None
    }
}

// ------------------------------------------------------------------------------------------

pub struct Now {
    pub path: PathBuf,
    pub label: String,
    pub unit: Unit,
    pub words: Vec<Word>,
    /// still being generated while playing
    pub streaming: bool,
    /// estimated total length while streaming (s)
    pub est_total: Option<f32>,
}

pub struct Player {
    _stream: rodio::OutputStream,
    handle: rodio::OutputStreamHandle,
    sink: Option<rodio::Sink>,
    shared: Option<Arc<Mutex<Shared>>>,
    pub now: Option<Now>,
    volume: f32,
    speed: f32,
}

impl Player {
    pub fn new(volume: f32, speed: f32) -> Option<Self> {
        let (_stream, handle) = rodio::OutputStream::try_default().ok()?;
        Some(Self { _stream, handle, sink: None, shared: None, now: None, volume, speed })
    }

    fn start(&mut self, samples: Vec<f32>, sr: u32, complete: bool, min_ahead: f32, now: Now) -> Result<(), String> {
        self.stop();
        let shared = Arc::new(Mutex::new(Shared {
            samples,
            sr,
            complete,
            pos: 0.0,
            speed: self.speed,
            buffering: !complete,
            min_ahead,
            seek_to: None,
        }));
        let sink = rodio::Sink::try_new(&self.handle).map_err(|e| e.to_string())?;
        sink.set_volume(self.volume);
        sink.append(Stretch::new(shared.clone(), sr));
        self.sink = Some(sink);
        self.shared = Some(shared);
        self.now = Some(now);
        Ok(())
    }

    /// Play a finished WAV (+ its word timings).
    pub fn play(&mut self, path: &Path, label: String, unit: Unit) -> Result<(), String> {
        let (samples, sr) = read_wav(path)?;
        let words = load_words(path);
        self.start(samples, sr, true, 0.0, Now { path: path.to_path_buf(), label, unit, words, streaming: false, est_total: None })
    }

    /// Start playing audio that is still being generated; feed it with `stream_append`.
    /// Playback begins once `min_ahead` seconds are buffered.
    pub fn play_stream(&mut self, path: &Path, sr: u32, min_ahead: f32, label: String, unit: Unit) -> Result<(), String> {
        let now = Now { path: path.to_path_buf(), label, unit, words: Vec::new(), streaming: true, est_total: None };
        self.start(Vec::new(), sr, false, min_ahead, now)
    }

    pub fn stream_append(&mut self, samples: impl Iterator<Item = f32>, words: Vec<Word>) {
        if let (Some(sh), Some(now)) = (&self.shared, &mut self.now) {
            sh.lock().unwrap().samples.extend(samples);
            now.words.extend(words);
        }
    }

    /// The whole file is ready: switch to it, keeping the current spot.
    /// `offset` = where the streamed audio starts inside the final file (s).
    pub fn stream_finish(&mut self, offset: f32) {
        let (Some(sh), Some(now)) = (&self.shared, &mut self.now) else { return };
        let Ok((samples, sr)) = read_wav(&now.path) else { return };
        let mut s = sh.lock().unwrap();
        let shift = offset as f64 * sr as f64;
        s.pos += shift;
        if let Some(t) = s.seek_to.as_mut() {
            *t += shift;
        }
        s.samples = samples;
        s.complete = true;
        s.buffering = false;
        drop(s);
        now.words = load_words(&now.path);
        now.streaming = false;
        now.est_total = None;
    }

    fn with<T>(&self, f: impl FnOnce(&Shared) -> T) -> Option<T> {
        self.shared.as_ref().map(|s| f(&s.lock().unwrap()))
    }

    pub fn is_playing(&self) -> bool {
        self.sink.as_ref().is_some_and(|s| !s.empty() && !s.is_paused())
    }
    pub fn is_buffering(&self) -> bool {
        self.with(|s| s.buffering).unwrap_or(false)
    }
    /// Loaded audio has played to the end (not stopped by the user).
    pub fn finished(&self) -> bool {
        self.now.is_some() && self.sink.as_ref().map_or(true, |s| s.empty())
    }
    pub fn toggle(&mut self) {
        if let Some(s) = &self.sink {
            if s.is_paused() { s.play() } else { s.pause() }
        }
    }
    pub fn stop(&mut self) {
        if let Some(s) = self.sink.take() {
            s.stop();
        }
        self.shared = None;
        self.now = None;
    }
    /// Current position in book time (s).
    pub fn pos(&self) -> f32 {
        self.with(|s| (s.seek_to.unwrap_or(s.pos) / s.sr as f64) as f32).unwrap_or(0.0)
    }
    /// Seconds of audio available (whole file, or streamed so far).
    pub fn buffered(&self) -> f32 {
        self.with(|s| s.samples.len() as f32 / s.sr as f32).unwrap_or(0.0)
    }
    /// Total length: exact when complete, estimated while streaming.
    pub fn duration(&self) -> f32 {
        let b = self.buffered();
        match &self.now {
            Some(n) if n.streaming => n.est_total.unwrap_or(b).max(b),
            _ => b,
        }
    }
    pub fn seek(&mut self, t: f32) {
        if let Some(sh) = &self.shared {
            let mut s = sh.lock().unwrap();
            let max = s.samples.len() as f64;
            s.seek_to = Some((t.max(0.0) as f64 * s.sr as f64).min(max));
        }
    }
    pub fn set_volume(&mut self, v: f32) {
        self.volume = v;
        if let Some(s) = &self.sink {
            s.set_volume(v);
        }
    }
    pub fn set_speed(&mut self, v: f32) {
        self.speed = v;
        if let Some(sh) = &self.shared {
            sh.lock().unwrap().speed = v;
        }
    }
    /// Index of the word being spoken right now.
    pub fn current_word(&self) -> Option<usize> {
        let now = self.now.as_ref()?;
        if now.words.is_empty() {
            return None;
        }
        let t = self.pos();
        let i = now.words.partition_point(|w| w.2 <= t);
        Some(i.saturating_sub(1))
    }
    /// Time at which the first word at or after `char_pos` starts.
    pub fn time_of_char(&self, char_pos: usize) -> Option<f32> {
        let now = self.now.as_ref()?;
        now.words.iter().find(|w| w.1 > char_pos).map(|w| w.2)
    }
}

fn load_words(wav: &Path) -> Vec<Word> {
    std::fs::read(timing_path(wav))
        .ok()
        .and_then(|b| serde_json::from_slice::<Timing>(&b).ok())
        .unwrap_or_default()
        .words
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(samples: Vec<f32>, speed: f32) -> (Vec<f32>, f64) {
        let shared = Arc::new(Mutex::new(Shared {
            samples, sr: 24000, complete: true, pos: 0.0, speed, buffering: false, min_ahead: 0.0, seek_to: None,
        }));
        let out: Vec<f32> = Stretch::new(shared.clone(), 24000).collect();
        let pos = shared.lock().unwrap().pos;
        (out, pos)
    }

    #[test]
    fn stretch_changes_length_not_pitch() {
        // 2 s of a 220 Hz tone
        let tone: Vec<f32> = (0..48000).map(|i| (i as f32 * 220.0 * std::f32::consts::TAU / 24000.0).sin() * 0.5).collect();
        for speed in [0.75f32, 1.0, 1.5, 2.0] {
            let (out, _) = run(tone.clone(), speed);
            let expect = 48000.0 / speed;
            assert!((out.len() as f32 - expect).abs() < 1500.0, "speed {speed}: len {} vs {expect}", out.len());
            // pitch: count zero crossings in the steady middle part
            let mid = &out[out.len() / 4..out.len() * 3 / 4];
            let zc = mid.windows(2).filter(|w| w[0] < 0.0 && w[1] >= 0.0).count() as f32;
            let hz = zc / (mid.len() as f32 / 24000.0);
            assert!((hz - 220.0).abs() < 8.0, "speed {speed}: pitch {hz} Hz");
        }
    }

    #[test]
    fn streaming_waits_for_buffer() {
        let shared = Arc::new(Mutex::new(Shared {
            samples: vec![0.1; 1000], sr: 24000, complete: false, pos: 0.0, speed: 1.0, buffering: true, min_ahead: 1.0, seek_to: None,
        }));
        let mut s = Stretch::new(shared.clone(), 24000);
        for _ in 0..2000 {
            assert_eq!(s.next(), Some(0.0)); // silence while buffering
        }
        assert_eq!(shared.lock().unwrap().pos, 0.0);
        shared.lock().unwrap().samples.extend(vec![0.1; 48000]);
        let got: Vec<f32> = (0..2000).filter_map(|_| s.next()).collect();
        assert!(got.iter().any(|&x| x > 0.05), "plays once buffered");
        assert!(shared.lock().unwrap().pos > 0.0);
    }
}
