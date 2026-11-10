use crate::book::data_dir;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

/// English Phonon voices (from `python -m ptts --list-voices`).
pub const PHONON_VOICES: &[&str] = &[
    "Everly", "Hadley", "Juniper", "Harper", "Imogen", "Delia", "Zoey", "Maeve", "Freya", "Pippa",
    "Elodie-Rose", "Lottie", "Hugh", "Spencer", "Miles", "Edwin", "Preston", "Trevor", "Easton",
    "Micah", "Russell", "Alfie", "Toby", "Teddy",
];
pub const WOOF_VOICES: &[&str] = &["juno", "mira"];
/// English Kokoro voices: a = American, b = British; f = female, m = male.
pub const KOKORO_VOICES: &[&str] = &[
    "af_heart", "af_bella", "af_nicole", "af_sarah", "af_sky", "af_nova", "af_river", "af_jessica", "af_kore",
    "af_alloy", "af_aoede", "bf_emma", "bf_isabella", "bf_alice", "bf_lily", "am_michael", "am_fenrir",
    "am_puck", "am_echo", "am_eric", "am_liam", "am_onyx", "am_adam", "am_santa", "bm_george", "bm_fable",
    "bm_lewis", "bm_daniel",
];

/// One tunable generation setting of an engine, passed to its worker as `--flag value`.
pub struct Param {
    pub flag: &'static str,
    pub label: &'static str,
    pub help: &'static str,
    pub kind: Kind,
}
pub enum Kind {
    Float { min: f32, max: f32, step: f32, default: f32 },
    Int { min: i32, max: i32, default: i32 },
    Choice { options: &'static [(&'static str, &'static str)], default: usize }, // (value, label)
}

const PIECE: Param = Param {
    flag: "max-chars",
    label: "Text per generation step",
    help: "Longest stretch of text (characters) synthesized in one go. Longer = more natural flow across sentences, but more risk of skipped or repeated words.",
    kind: Kind::Int { min: 150, max: 1200, default: 700 },
};

pub const PHONON_PARAMS: &[Param] = &[
    Param {
        flag: "temperature",
        label: "Temperature",
        help: "Randomness of the delivery. Higher = more varied and lively, but more mistakes. Phonon default 0.3.",
        kind: Kind::Float { min: 0.0, max: 1.0, step: 0.05, default: 0.3 },
    },
    Param {
        flag: "quant",
        label: "Weights",
        help: "Full precision sounds best; 8-bit is faster on slow CPUs.",
        kind: Kind::Choice { options: &[("f32", "Full precision"), ("q8_0", "8-bit (faster)")], default: 0 },
    },
    PIECE,
];

pub const WOOF_PARAMS: &[Param] = &[
    Param {
        flag: "temp",
        label: "Temperature",
        help: "Randomness of the delivery. Higher = livelier, but more mistakes. Woof default 0.5.",
        kind: Kind::Float { min: 0.1, max: 1.0, step: 0.05, default: 0.5 },
    },
    Param {
        flag: "clause-max",
        label: "Sentence split length",
        help: "Woof speaks each sentence separately and cuts longer ones at commas. Raise to keep long sentences whole. Woof default 120.",
        kind: Kind::Int { min: 60, max: 300, default: 120 },
    },
    Param {
        flag: "eos",
        label: "End-of-speech threshold",
        help: "How sure the model must be that a sentence is finished before stopping. Higher lets sentences run on. Woof default 0.9.",
        kind: Kind::Float { min: 0.5, max: 0.99, step: 0.01, default: 0.9 },
    },
    Param {
        flag: "retries",
        label: "Retakes",
        help: "How often a piece is re-generated when it stalls or runs too long. More = cleaner but slower. Woof default 3.",
        kind: Kind::Int { min: 0, max: 6, default: 3 },
    },
    PIECE,
];

pub const KOKORO_PARAMS: &[Param] = &[PIECE];

#[derive(Serialize, Deserialize, Clone)]
#[serde(default)]
pub struct Config {
    pub engine: String,
    /// engine name -> worker command (argv); built-in, derived from the data folder
    #[serde(skip)]
    pub engines: BTreeMap<String, Vec<String>>,
    /// engine name -> selected voice
    pub voices: BTreeMap<String, String>,
    pub sentence_pause: f32,
    pub para_pause: f32,
    pub max_pause: f32,
    pub font_size: f32,
    pub line_spacing: f32,
    pub serif: bool,
    pub page_width: f32,
    pub page_mode: bool,
    pub continuous: bool,
    pub follow: bool,
    pub prefetch: bool,
    pub volume: f32,
    /// playback speed (pitch preserved)
    pub speed: f32,
    /// seconds of streamed audio to buffer before playback starts
    pub stream_buffer: f32,
    /// engine -> flag -> value: advanced generation settings changed from their defaults
    pub advanced: BTreeMap<String, BTreeMap<String, String>>,
}

impl Default for Config {
    fn default() -> Self {
        let d = data_dir();
        let py = crate::installer::venv_python();
        let cmd = |script: &str| vec![py.to_string_lossy().into(), d.join("workers").join(script).to_string_lossy().into()];
        let mut engines = BTreeMap::new();
        engines.insert("phonon".to_string(), cmd("phonon_worker.py"));
        engines.insert("woof".to_string(), cmd("woof_worker.py"));
        engines.insert("kokoro".to_string(), cmd("kokoro_worker.py"));
        let mut voices = BTreeMap::new();
        voices.insert("phonon".to_string(), "Everly".to_string());
        voices.insert("woof".to_string(), "juno".to_string());
        voices.insert("kokoro".to_string(), "af_heart".to_string());
        Config {
            engine: "kokoro".into(),
            engines,
            voices,
            sentence_pause: 0.30,
            para_pause: 0.60,
            max_pause: 0.45,
            font_size: 20.0,
            line_spacing: 1.6,
            serif: true,
            page_width: 720.0,
            page_mode: false,
            continuous: true,
            follow: true,
            prefetch: true,
            volume: 1.0,
            speed: 1.0,
            stream_buffer: 10.0,
            advanced: BTreeMap::new(),
        }
    }
}

impl Config {
    fn path() -> PathBuf {
        crate::book::config_dir().join("config.json")
    }

    pub fn load() -> Self {
        let mut c: Config = std::fs::read(Self::path())
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        // engine commands are always derived from the current data folder
        let defaults = Config::default();
        c.engines = defaults.engines;
        // voices added in newer versions appear automatically in older configs
        for (name, v) in defaults.voices {
            c.voices.entry(name).or_insert(v);
        }
        if !c.engines.contains_key(&c.engine) {
            c.engine = c.engines.keys().next().cloned().unwrap_or_default();
        }
        c
    }

    pub fn save(&self) {
        let p = Self::path();
        let _ = std::fs::create_dir_all(p.parent().unwrap());
        let _ = std::fs::write(p, serde_json::to_vec_pretty(self).unwrap());
    }

    pub fn voice_list(engine: &str) -> &'static [&'static str] {
        match engine {
            "phonon" => PHONON_VOICES,
            "woof" => WOOF_VOICES,
            "kokoro" => KOKORO_VOICES,
            _ => &[],
        }
    }

    pub fn params(engine: &str) -> &'static [Param] {
        match engine {
            "phonon" => PHONON_PARAMS,
            "woof" => WOOF_PARAMS,
            "kokoro" => KOKORO_PARAMS,
            _ => &[],
        }
    }

    /// Current value of an advanced setting (as text), or None when it is at its default.
    pub fn adv(&self, flag: &str) -> Option<&str> {
        self.advanced.get(&self.engine).and_then(|m| m.get(flag)).map(String::as_str)
    }

    pub fn set_adv(&mut self, flag: &str, value: Option<String>) {
        let m = self.advanced.entry(self.engine.clone()).or_default();
        match value {
            Some(v) => {
                m.insert(flag.to_string(), v);
            }
            None => {
                m.remove(flag);
            }
        }
    }

    pub fn voice(&self) -> Option<&str> {
        self.voices.get(&self.engine).map(String::as_str)
    }

    /// Audio is stored per engine+voice, so changing voice never reuses old audio.
    pub fn profile(&self) -> String {
        match self.voice() {
            Some(v) => format!("{}-{}", self.engine, v).to_lowercase(),
            None => self.engine.clone(),
        }
    }

    /// Full worker argv: engine command + voice + pacing.
    pub fn worker_command(&self) -> Vec<String> {
        let mut cmd = self.engines.get(&self.engine).cloned().unwrap_or_default();
        if let Some(v) = self.voice() {
            cmd.extend(["--voice".into(), v.into()]);
        }
        cmd.extend([
            "--sentence-pause".into(), format!("{:.2}", self.sentence_pause),
            "--para-pause".into(), format!("{:.2}", self.para_pause),
            "--max-pause".into(), format!("{:.2}", self.max_pause),
        ]);
        if let Some(m) = self.advanced.get(&self.engine) {
            for p in Config::params(&self.engine) {
                if let Some(v) = m.get(p.flag) {
                    cmd.extend([format!("--{}", p.flag), v.clone()]);
                }
            }
        }
        cmd
    }
}

/// Friendly voice name: "af_heart" -> "Heart (US ♀)", other engines' names are capitalised.
pub fn voice_label(voice: &str) -> String {
    let b = voice.as_bytes();
    if b.len() > 3 && b[2] == b'_' && matches!(b[0], b'a' | b'b') && matches!(b[1], b'f' | b'm') {
        let mut name = voice[3..].to_string();
        name[..1].make_ascii_uppercase();
        let accent = if b[0] == b'a' { "US" } else { "UK" };
        let sex = if b[1] == b'f' { "♀" } else { "♂" };
        return format!("{name} ({accent} {sex})");
    }
    let mut c = voice.chars();
    c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
}

/// "phonon-everly" -> "Phonon · Everly"; legacy "phonon" -> "Phonon (original voice)"
pub fn profile_label(profile: &str) -> String {
    let cap = |s: &str| {
        let mut c = s.chars();
        c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
    };
    match profile.split_once('-') {
        Some((e, v)) => format!("{} · {}", cap(e), voice_label(v)),
        None => format!("{} (original voice)", cap(profile)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn advanced_settings_reach_worker() {
        let mut c = Config::default();
        c.engine = "woof".into();
        assert!(!c.worker_command().iter().any(|a| a == "--temp"), "defaults are not passed");
        c.set_adv("temp", Some("0.650".into()));
        c.set_adv("retries", Some("5".into()));
        let cmd = c.worker_command();
        let pos = cmd.iter().position(|a| a == "--temp").unwrap();
        assert_eq!(cmd[pos + 1], "0.650");
        assert!(cmd.windows(2).any(|w| w[0] == "--retries" && w[1] == "5"));
        // other engines are unaffected
        c.engine = "phonon".into();
        assert!(!c.worker_command().iter().any(|a| a == "--temp" || a == "--retries"));
        c.engine = "woof".into();
        c.set_adv("temp", None);
        assert!(!c.worker_command().iter().any(|a| a == "--temp"));
    }
}
