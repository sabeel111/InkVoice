//! Installs voice engines from inside the app, so users never touch Python:
//! 1. downloads `uv` (a single-file Python manager) into the data folder,
//! 2. lets uv create a private Python environment there,
//! 3. installs the engine's packages and downloads its model files.
//! Everything lives under the InkVoice data folder; removing an engine deletes its files.
use crate::book::data_dir;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc::{channel, Receiver, Sender};

const UV_VERSION: &str = "0.13.7";
const PYTHON: &str = "3.12";

pub struct Spec {
    pub name: &'static str,
    pub title: &'static str,
    pub blurb: &'static str,
    pub size: &'static str,
    pub license: &'static str,
    /// Python packages
    pip: &'static [&'static str],
    /// package folder that proves the packages are installed
    probe_pkg: &'static str,
    /// (url, path inside the data folder)
    files: &'static [(&'static str, &'static str)],
    /// Python run once after installing (e.g. to fetch model weights)
    warmup: Option<&'static str>,
}

macro_rules! woof {
    ($f:literal) => {
        (concat!("https://huggingface.co/ConwayResearch/Underdog-Woof/resolve/main/", $f), concat!("models/woof/", $f))
    };
}

pub const SPECS: &[Spec] = &[
    Spec {
        name: "kokoro",
        title: "Kokoro",
        blurb: "Clear, natural narration. 28 English voices (US & UK). Recommended.",
        size: "115 MB",
        license: "Apache-2.0",
        pip: &["kokoro-onnx", "numpy"],
        probe_pkg: "kokoro_onnx",
        files: &[
            (concat!("https://github.com/thewh1teagle/kokoro-onnx/releases/download/model-files-v1.0/", "kokoro-v1.0.int8.onnx"), "models/kokoro/kokoro-v1.0.int8.onnx"),
            (concat!("https://github.com/thewh1teagle/kokoro-onnx/releases/download/model-files-v1.0/", "voices-v1.0.bin"), "models/kokoro/voices-v1.0.bin"),
        ],
        warmup: None,
    },
    Spec {
        name: "phonon",
        title: "Phonon",
        blurb: "Fast and light (31M). 24 English voices.",
        size: "125 MB",
        license: "Apache-2.0",
        pip: &["ptts", "numpy"],
        probe_pkg: "ptts",
        files: &[],
        warmup: Some("import ptts; ptts.TTS(lang='en', voice='Everly'); print('ready')"),
    },
    Spec {
        name: "woof",
        title: "Underdog Woof",
        blurb: "Tiny (32 MB) and lively. 2 voices. Free for non-commercial use only.",
        size: "41 MB",
        license: "Non-commercial",
        pip: &["numpy"],
        probe_pkg: "numpy",
        files: &[
            woof!("woof_int8.npz"),
            woof!("underdog_woof/__init__.py"),
            woof!("underdog_woof/__main__.py"),
            woof!("underdog_woof/normalize.py"),
            woof!("underdog_woof/respell.py"),
            woof!("underdog_woof/respell.tsv"),
            woof!("underdog_woof/runtime.py"),
            woof!("underdog_woof/textsplit.py"),
            woof!("LICENSE"),
            woof!("NOTICE"),
        ],
        warmup: None,
    },
];
pub fn spec(name: &str) -> Option<&'static Spec> {
    SPECS.iter().find(|s| s.name == name)
}

pub fn venv_python() -> PathBuf {
    let v = data_dir().join("venv");
    if cfg!(windows) { v.join("Scripts").join("python.exe") } else { v.join("bin").join("python") }
}

fn site_packages() -> Vec<PathBuf> {
    let v = data_dir().join("venv");
    let mut out = vec![v.join("Lib").join("site-packages")];
    if let Ok(rd) = std::fs::read_dir(v.join("lib")) {
        out.extend(rd.flatten().map(|e| e.path().join("site-packages")));
    }
    out
}

pub fn is_installed(name: &str) -> bool {
    let Some(s) = spec(name) else { return false };
    // engines that fetch weights at warm-up keep them in <data>/hf
    if s.warmup.is_some() && !data_dir().join("hf").is_dir() {
        return false;
    }
    venv_python().exists()
        && site_packages().iter().any(|sp| sp.join(s.probe_pkg).is_dir())
        && s.files.iter().all(|(_, rel)| data_dir().join(rel).is_file())
}

/// Remove an engine's model files (the shared Python environment stays; it is small).
pub fn remove(name: &str) {
    let d = data_dir();
    match name {
        "phonon" => {
            let _ = std::fs::remove_dir_all(d.join("hf"));
            if let Some(py) = venv_python().to_str() {
                let _ = uv_cmd(&["pip", "uninstall", "--python", py, "ptts"]).output();
            }
        }
        "kokoro" => {
            let _ = std::fs::remove_dir_all(d.join("models").join("kokoro"));
        }
        "woof" => {
            let _ = std::fs::remove_dir_all(d.join("models").join("woof"));
        }
        _ => {}
    }
}

pub enum Ev {
    Step(String),
    /// Some(fraction) for downloads, None when the length is unknown
    Progress(Option<f32>),
    Done,
    Failed(String),
}

pub struct Install {
    pub engine: String,
    pub rx: Receiver<Ev>,
}

pub fn start(engine: &str, ctx: eframe::egui::Context) -> Install {
    let (tx, rx) = channel();
    let name = engine.to_string();
    std::thread::spawn(move || {
        let send = |e: Ev| {
            let _ = tx.send(e);
            ctx.request_repaint();
        };
        match run(&name, &tx, &ctx) {
            Ok(()) => send(Ev::Done),
            Err(e) => send(Ev::Failed(e)),
        }
    });
    Install { engine: engine.to_string(), rx }
}

fn run(name: &str, tx: &Sender<Ev>, ctx: &eframe::egui::Context) -> Result<(), String> {
    let s = spec(name).ok_or("unknown engine")?;
    let step = |t: &str| {
        let _ = tx.send(Ev::Step(t.to_string()));
        let _ = tx.send(Ev::Progress(None));
        ctx.request_repaint();
    };
    let d = data_dir();
    std::fs::create_dir_all(&d).map_err(|e| e.to_string())?;

    if !uv_path().exists() {
        step("Downloading the Python manager (uv)…");
        get_uv(tx, ctx)?;
    }
    if !venv_python().exists() {
        step("Setting up a private Python (first time only)…");
        let venv = d.join("venv");
        run_logged(uv_cmd(&["venv", "--python", PYTHON, venv.to_str().unwrap()]))?;
    }
    step(&format!("Installing {} packages…", s.title));
    let py = venv_python();
    let mut args = vec!["pip", "install", "--python", py.to_str().unwrap()];
    args.extend(s.pip);
    run_logged(uv_cmd(&args))?;

    for (i, (url, rel)) in s.files.iter().enumerate() {
        let dest = d.join(rel);
        if dest.is_file() {
            continue;
        }
        let file = Path::new(rel).file_name().unwrap().to_string_lossy().into_owned();
        step(&format!("Downloading {} model ({}/{}): {file}", s.title, i + 1, s.files.len()));
        download(url, &dest, tx, ctx)?;
    }
    if let Some(code) = s.warmup {
        step(&format!("Downloading {} voice model…", s.title));
        let mut c = Command::new(&py);
        c.args(["-c", code]).env("HF_HOME", d.join("hf")).env("INKVOICE_DATA", &d);
        run_logged(c)?;
    }
    Ok(())
}

// ------------------------------------------------------------------------------------------

fn uv_path() -> PathBuf {
    data_dir().join("tools").join(if cfg!(windows) { "uv.exe" } else { "uv" })
}

fn uv_cmd(args: &[&str]) -> Command {
    let d = data_dir();
    let mut c = Command::new(uv_path());
    c.args(args)
        .env("UV_PYTHON_INSTALL_DIR", d.join("python"))
        .env("UV_CACHE_DIR", d.join("cache").join("uv"))
        .env("UV_PYTHON_PREFERENCE", "only-managed")
        .env("UV_NO_PROGRESS", "1");
    c
}

fn no_window(c: &mut Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        c.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    let _ = c;
}

/// Run a command, keeping its last output lines for the error message.
fn run_logged(mut c: Command) -> Result<(), String> {
    no_window(&mut c);
    let mut child = c
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("could not run {:?}: {e}", c.get_program()))?;
    let err = child.stderr.take().unwrap();
    let tail = std::thread::spawn(move || {
        let mut last: Vec<String> = Vec::new();
        for l in BufReader::new(err).lines().map_while(Result::ok) {
            last.push(l);
            if last.len() > 6 {
                last.remove(0);
            }
        }
        last.join("\n")
    });
    let _ = std::io::copy(&mut child.stdout.take().unwrap(), &mut std::io::sink());
    let status = child.wait().map_err(|e| e.to_string())?;
    let log = tail.join().unwrap_or_default();
    if status.success() { Ok(()) } else { Err(if log.is_empty() { format!("failed ({status})") } else { log }) }
}

fn uv_asset() -> &'static str {
    match (cfg!(windows), cfg!(target_arch = "aarch64")) {
        (true, false) => "uv-x86_64-pc-windows-msvc.zip",
        (true, true) => "uv-aarch64-pc-windows-msvc.zip",
        (false, false) => "uv-x86_64-unknown-linux-gnu.tar.gz",
        (false, true) => "uv-aarch64-unknown-linux-gnu.tar.gz",
    }
}

fn get_uv(tx: &Sender<Ev>, ctx: &eframe::egui::Context) -> Result<(), String> {
    let tools = data_dir().join("tools");
    let unpack = tools.join("uv-unpack");
    let _ = std::fs::remove_dir_all(&unpack);
    std::fs::create_dir_all(&unpack).map_err(|e| e.to_string())?;
    let asset = uv_asset();
    let archive = tools.join(asset);
    let url = format!("https://github.com/astral-sh/uv/releases/download/{UV_VERSION}/{asset}");
    download(&url, &archive, tx, ctx)?;
    // `tar` ships with Linux and with Windows 10+ (it also unpacks .zip there)
    let mut c = Command::new("tar");
    c.arg("-xf").arg(&archive).arg("-C").arg(&unpack);
    run_logged(c)?;
    let exe = if cfg!(windows) { "uv.exe" } else { "uv" };
    let found = find_file(&unpack, exe).ok_or("uv was not found in its download")?;
    std::fs::rename(&found, uv_path()).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(uv_path(), std::fs::Permissions::from_mode(0o755));
    }
    let _ = std::fs::remove_dir_all(&unpack);
    let _ = std::fs::remove_file(&archive);
    Ok(())
}

fn find_file(dir: &Path, name: &str) -> Option<PathBuf> {
    for e in std::fs::read_dir(dir).ok()?.flatten() {
        let p = e.path();
        if p.is_dir() {
            if let Some(f) = find_file(&p, name) {
                return Some(f);
            }
        } else if p.file_name().is_some_and(|n| n == name) {
            return Some(p);
        }
    }
    None
}

/// Download with `curl` (bundled with Linux distros and Windows 10+), reporting progress
/// by watching the file grow. Writes to `<dest>.part` and renames when complete.
fn download(url: &str, dest: &Path, tx: &Sender<Ev>, ctx: &eframe::egui::Context) -> Result<(), String> {
    if let Some(p) = dest.parent() {
        std::fs::create_dir_all(p).map_err(|e| e.to_string())?;
    }
    let part = dest.with_extension(format!("{}part", dest.extension().map(|e| format!("{}.", e.to_string_lossy())).unwrap_or_default()));
    let total = content_length(url);
    let mut c = Command::new("curl");
    c.args(["-fsSL", "--retry", "3", "-o"]).arg(&part).arg(url).stderr(Stdio::piped());
    no_window(&mut c);
    let mut child = c.spawn().map_err(|e| format!("could not run curl: {e}"))?;
    loop {
        if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
            if !status.success() {
                let mut msg = String::new();
                if let Some(mut e) = child.stderr.take() {
                    let _ = std::io::Read::read_to_string(&mut e, &mut msg);
                }
                let _ = std::fs::remove_file(&part);
                return Err(format!("download failed: {url}\n{}", msg.trim()));
            }
            break;
        }
        if let Some(t) = total {
            let have = std::fs::metadata(&part).map(|m| m.len()).unwrap_or(0);
            let _ = tx.send(Ev::Progress(Some((have as f32 / t as f32).min(1.0))));
            ctx.request_repaint();
        }
        std::thread::sleep(std::time::Duration::from_millis(250));
    }
    std::fs::rename(&part, dest).map_err(|e| e.to_string())
}

/// Final Content-Length after redirects, if the server reports one.
fn content_length(url: &str) -> Option<u64> {
    let mut c = Command::new("curl");
    c.args(["-sIL", url]);
    no_window(&mut c);
    let out = c.output().ok()?;
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| {
            let (k, v) = l.split_once(':')?;
            k.trim().eq_ignore_ascii_case("content-length").then(|| v.trim().parse::<u64>().ok()).flatten()
        })
        .filter(|&n| n > 0)
        .last()
}

#[cfg(test)]
mod tests {
    /// Full install into an empty data folder. Run with:
    ///   XDG_DATA_HOME=/tmp/x/data XDG_CONFIG_HOME=/tmp/x/config cargo test fresh_install -- --ignored --nocapture
    #[test]
    #[ignore]
    fn fresh_install() {
        let engine = std::env::var("ENGINE").unwrap_or_else(|_| "kokoro".into());
        let t0 = std::time::Instant::now();
        let inst = super::start(&engine, eframe::egui::Context::default());
        let mut last = String::new();
        for ev in inst.rx.iter() {
            match ev {
                super::Ev::Step(s) => {
                    eprintln!("[{:5.1}s] {s}", t0.elapsed().as_secs_f32());
                    last = s;
                }
                super::Ev::Progress(_) => {}
                super::Ev::Done => {
                    eprintln!("[{:5.1}s] DONE", t0.elapsed().as_secs_f32());
                    break;
                }
                super::Ev::Failed(e) => panic!("install failed during '{last}': {e}"),
            }
        }
        assert!(super::is_installed(&engine), "probe says not installed");
    }
}
