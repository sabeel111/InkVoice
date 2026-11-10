//! Installs voice engines from inside the app, so users never touch Python:
//! 1. downloads `uv` (a single-file Python manager) into the data folder,
//! 2. lets uv create a private Python environment there,
//! 3. installs the engine's packages and downloads its model files.
//! Everything lives under the InkVoice data folder; removing an engine deletes its files.
use crate::book::data_dir;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

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
    /// rough download sizes in MB, used to weight the progress bar
    pip_mb: f32,
    warmup_mb: f32,
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
        pip_mb: 50.0,
        warmup_mb: 0.0,
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
        pip_mb: 25.0,
        warmup_mb: 120.0,
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
        pip_mb: 16.0,
        warmup_mb: 0.0,
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

#[derive(Clone)]
pub struct StepInfo {
    pub label: String,
    /// share of the whole install (rough MB of work)
    pub weight: f32,
}

pub enum Ev {
    /// the steps this install will go through
    Plan(Vec<StepInfo>),
    /// step `i` is `frac` done; `detail` e.g. "38.2 of 92.4 MB · 4.1 MB/s"
    Step { i: usize, frac: f32, detail: String },
    Done,
    Cancelled,
    Failed(String),
}

pub struct Install {
    pub engine: String,
    pub rx: Receiver<Ev>,
    cancel: Arc<AtomicBool>,
    child: Arc<Mutex<Option<Child>>>,
}

impl Install {
    /// Stop the install: kills the running download/setup and cleans up partial files.
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::SeqCst);
        if let Some(c) = self.child.lock().unwrap().as_mut() {
            let _ = c.kill();
        }
    }
}

/// An install handle with nothing behind it (screenshots/tests).
#[cfg(test)]
pub fn dummy(engine: &str) -> Install {
    let (_tx, rx) = channel();
    Install { engine: engine.to_string(), rx, cancel: Arc::default(), child: Arc::default() }
}

/// Everything a running install needs to report progress and be cancelled.
struct Ctl {
    tx: Sender<Ev>,
    ctx: eframe::egui::Context,
    cancel: Arc<AtomicBool>,
    child: Arc<Mutex<Option<Child>>>,
    step: usize,
}

impl Ctl {
    fn emit(&self, frac: f32, detail: impl Into<String>) {
        let _ = self.tx.send(Ev::Step { i: self.step, frac: frac.clamp(0.0, 1.0), detail: detail.into() });
        self.ctx.request_repaint();
    }
    fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::SeqCst)
    }
    fn check(&self) -> Result<(), String> {
        if self.cancelled() { Err(CANCELLED.into()) } else { Ok(()) }
    }
}

const CANCELLED: &str = "cancelled";

pub fn start(engine: &str, ctx: eframe::egui::Context) -> Install {
    let (tx, rx) = channel();
    let cancel = Arc::new(AtomicBool::new(false));
    let child: Arc<Mutex<Option<Child>>> = Arc::default();
    let mut ctl = Ctl { tx: tx.clone(), ctx: ctx.clone(), cancel: cancel.clone(), child: child.clone(), step: 0 };
    let name = engine.to_string();
    std::thread::spawn(move || {
        let r = run(&name, &mut ctl);
        let ev = match r {
            Ok(()) => Ev::Done,
            Err(_) if ctl.cancelled() => {
                cleanup_partial();
                Ev::Cancelled
            }
            Err(e) => {
                cleanup_partial();
                Ev::Failed(e)
            }
        };
        let _ = tx.send(ev);
        ctx.request_repaint();
    });
    Install { engine: engine.to_string(), rx, cancel, child }
}

/// Remove half-downloaded files and a half-created Python environment.
fn cleanup_partial() {
    fn walk(d: &Path) {
        for e in std::fs::read_dir(d).into_iter().flatten().flatten() {
            let p = e.path();
            if p.is_dir() {
                walk(&p);
            } else if p.extension().is_some_and(|x| x == "part") {
                let _ = std::fs::remove_file(p);
            }
        }
    }
    let d = data_dir();
    walk(&d.join("models"));
    walk(&d.join("tools"));
    let venv = d.join("venv");
    if venv.is_dir() && !venv_python().exists() {
        let _ = std::fs::remove_dir_all(venv);
    }
}

enum Task {
    Uv,
    Python,
    Packages,
    Files(Vec<(String, PathBuf, u64)>),
    Warmup,
}

fn run(name: &str, ctl: &mut Ctl) -> Result<(), String> {
    let s = spec(name).ok_or("unknown engine")?;
    let d = data_dir();
    std::fs::create_dir_all(&d).map_err(|e| e.to_string())?;

    // ---- plan: only the steps that are still needed, weighted by size ----
    let mut plan: Vec<(StepInfo, Task)> = Vec::new();
    if !uv_path().exists() {
        plan.push((StepInfo { label: "Download the installer tool (uv)".into(), weight: 18.0 }, Task::Uv));
    }
    if !venv_python().exists() {
        plan.push((StepInfo { label: "Set up a private Python".into(), weight: 33.0 }, Task::Python));
    }
    plan.push((StepInfo { label: format!("Install {} software", s.title), weight: s.pip_mb }, Task::Packages));
    let missing: Vec<(String, PathBuf)> = s
        .files
        .iter()
        .map(|(u, rel)| (u.to_string(), d.join(rel)))
        .filter(|(_, p)| !p.is_file())
        .collect();
    if !missing.is_empty() {
        let sized: Vec<(String, PathBuf, u64)> =
            missing.into_iter().map(|(u, p)| { let n = content_length(&u).unwrap_or(0); (u, p, n) }).collect();
        let mb = sized.iter().map(|f| f.2).sum::<u64>() as f32 / 1e6;
        plan.push((StepInfo { label: format!("Download the {} voice model", s.title), weight: mb.max(1.0) }, Task::Files(sized)));
    }
    if s.warmup.is_some() {
        plan.push((StepInfo { label: format!("Download the {} voice model", s.title), weight: s.warmup_mb }, Task::Warmup));
    }
    let _ = ctl.tx.send(Ev::Plan(plan.iter().map(|p| p.0.clone()).collect()));

    for (i, (_, task)) in plan.into_iter().enumerate() {
        ctl.check()?;
        ctl.step = i;
        ctl.emit(0.0, "");
        match task {
            Task::Uv => get_uv(ctl)?,
            Task::Python => {
                let venv = d.join("venv");
                run_uv(uv_cmd(&["venv", "--python", PYTHON, venv.to_str().unwrap()]), ctl)?;
            }
            Task::Packages => {
                let py = venv_python();
                let mut args = vec!["pip", "install", "--python", py.to_str().unwrap()];
                args.extend(s.pip);
                run_uv(uv_cmd(&args), ctl)?;
            }
            Task::Files(files) => download_all(&files, ctl)?,
            Task::Warmup => {
                let mut c = Command::new(venv_python());
                c.args(["-c", s.warmup.unwrap()]).env("HF_HOME", d.join("hf")).env("INKVOICE_DATA", &d);
                run_watching_dir(c, &d.join("hf"), s.warmup_mb * 1e6, ctl)?;
            }
        }
        ctl.emit(1.0, "");
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
        .env("UV_NO_PROGRESS", "1")
        .env("NO_COLOR", "1");
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

/// Start a child that `Install::cancel` can kill.
fn spawn(mut c: Command, ctl: &Ctl) -> Result<Child, String> {
    no_window(&mut c);
    ctl.check()?;
    let prog = c.get_program().to_string_lossy().into_owned();
    let child = c.spawn().map_err(|e| format!("could not run {prog}: {e}"))?;
    Ok(child)
}

/// Wait for a child registered in `ctl.child`, calling `tick` every 250 ms.
fn wait(mut child: Child, ctl: &Ctl, mut tick: impl FnMut()) -> Result<std::process::ExitStatus, String> {
    let stderr = child.stderr.take();
    *ctl.child.lock().unwrap() = Some(child);
    let tail = stderr.map(|e| std::thread::spawn(move || {
        let mut last = Vec::new();
        for l in BufReader::new(e).lines().map_while(Result::ok) {
            last.push(l);
            if last.len() > 6 {
                last.remove(0);
            }
        }
        last.join("\n")
    }));
    let status = loop {
        let st = ctl.child.lock().unwrap().as_mut().unwrap().try_wait().map_err(|e| e.to_string())?;
        if let Some(st) = st {
            break st;
        }
        tick();
        std::thread::sleep(Duration::from_millis(250));
    };
    *ctl.child.lock().unwrap() = None;
    ctl.check()?;
    if status.success() {
        Ok(status)
    } else {
        let log = tail.and_then(|t| t.join().ok()).unwrap_or_default();
        Err(if log.is_empty() { format!("failed ({status})") } else { log })
    }
}

/// Run a uv command, turning its "Downloading X (16.0MiB)" / "Downloaded X" lines into progress.
fn run_uv(mut c: Command, ctl: &Ctl) -> Result<(), String> {
    c.stdout(Stdio::null()).stderr(Stdio::piped());
    let mut child = spawn(c, ctl)?;
    let err = child.stderr.take().unwrap();
    *ctl.child.lock().unwrap() = Some(child);

    #[derive(Default)]
    struct St {
        total: f32,
        done: f32,
        sizes: std::collections::HashMap<String, f32>,
        current: String,
        last_line: Vec<String>,
        events: u32,
    }
    let st = Arc::new(Mutex::new(St::default()));
    let st2 = st.clone();
    let reader = std::thread::spawn(move || {
        for l in BufReader::new(err).lines().map_while(Result::ok) {
            let l = l.trim().to_string();
            let mut s = st2.lock().unwrap();
            s.events += 1;
            if let Some(rest) = l.strip_prefix("Downloading ") {
                // "numpy (16.0MiB)" or "cpython-3.12.16-linux-x86_64-gnu (download) (32.9MiB)"
                if let Some(open) = rest.rfind('(') {
                    let name = rest[..open].trim().trim_end_matches("(download)").trim().to_string();
                    let mb = parse_size(&rest[open + 1..rest.len().saturating_sub(1)]);
                    s.total += mb;
                    s.sizes.insert(name.clone(), mb);
                    s.current = format!("Downloading {name} · {mb:.1} MB");
                }
            } else if let Some(name) = l.strip_prefix("Downloaded ") {
                let name = name.trim().trim_end_matches("(download)").trim();
                let mb = s.sizes.get(name).copied().unwrap_or(0.0);
                s.done += mb;
            } else if l.starts_with("Resolved") || l.starts_with("Prepared") || l.starts_with("Installed") || l.starts_with("Using") {
                s.current = l.clone();
            }
            s.last_line.push(l);
            if s.last_line.len() > 6 {
                s.last_line.remove(0);
            }
        }
    });

    let t0 = Instant::now();
    let status = loop {
        let st_now = ctl.child.lock().unwrap().as_mut().unwrap().try_wait().map_err(|e| e.to_string())?;
        if let Some(x) = st_now {
            break x;
        }
        {
            let s = st.lock().unwrap();
            // known downloads drive the bar; between events it creeps forward so it never looks stuck
            let known = if s.total > 0.0 { s.done / s.total } else { 0.0 };
            let creep = 1.0 - (-t0.elapsed().as_secs_f32() / 25.0).exp();
            let frac = (0.05 + 0.85 * known.max(creep * 0.6)).min(0.95);
            // uv only reports a download when it finishes: show what is being fetched meanwhile
            let detail = if s.total > 0.0 && s.done > 0.0 && s.done < s.total {
                format!("{:.1} of {:.1} MB", s.done, s.total)
            } else {
                s.current.clone()
            };
            ctl.emit(frac, detail);
        }
        std::thread::sleep(Duration::from_millis(250));
    };
    *ctl.child.lock().unwrap() = None;
    let _ = reader.join();
    ctl.check()?;
    if status.success() {
        Ok(())
    } else {
        Err(st.lock().unwrap().last_line.join("\n"))
    }
}

fn parse_size(s: &str) -> f32 {
    let s = s.trim();
    let (num, mult) = if let Some(n) = s.strip_suffix("GiB") {
        (n, 1073.7)
    } else if let Some(n) = s.strip_suffix("MiB") {
        (n, 1.048_576)
    } else if let Some(n) = s.strip_suffix("KiB") {
        (n, 0.001_024)
    } else {
        (s, 1.0)
    };
    num.trim().parse::<f32>().unwrap_or(0.0) * mult
}

/// Run a command while measuring how much it writes into `dir` (e.g. model weights).
fn run_watching_dir(mut c: Command, dir: &Path, expect_bytes: f32, ctl: &Ctl) -> Result<(), String> {
    c.stdout(Stdio::null()).stderr(Stdio::piped());
    let child = spawn(c, ctl)?;
    let start = dir_size(dir) as f32;
    let mut meter = Meter::new();
    wait(child, ctl, || {
        let got = (dir_size(dir) as f32 - start).max(0.0);
        meter.add_total(got as u64);
        let frac = (got / expect_bytes.max(1.0)).min(0.97);
        ctl.emit(frac, format!("{:.1} MB{}", got / 1e6, meter.speed_text()));
    })?;
    Ok(())
}

fn dir_size(p: &Path) -> u64 {
    let Ok(rd) = std::fs::read_dir(p) else { return 0 };
    rd.flatten()
        .map(|e| match e.file_type() {
            Ok(t) if t.is_dir() => dir_size(&e.path()),
            Ok(_) => e.metadata().map(|m| m.len()).unwrap_or(0),
            Err(_) => 0,
        })
        .sum()
}

/// Smoothed transfer speed.
struct Meter {
    samples: std::collections::VecDeque<(Instant, u64)>,
}
impl Meter {
    fn new() -> Self {
        Self { samples: Default::default() }
    }
    fn add_total(&mut self, total: u64) {
        let now = Instant::now();
        self.samples.push_back((now, total));
        while self.samples.len() > 2 && now.duration_since(self.samples[0].0) > Duration::from_secs(3) {
            self.samples.pop_front();
        }
    }
    fn speed(&self) -> Option<f32> {
        let (a, b) = (self.samples.front()?, self.samples.back()?);
        let dt = b.0.duration_since(a.0).as_secs_f32();
        (dt > 0.5).then(|| (b.1.saturating_sub(a.1)) as f32 / dt)
    }
    fn speed_text(&self) -> String {
        match self.speed() {
            Some(s) if s > 1e3 => format!(" · {:.1} MB/s", s / 1e6),
            _ => String::new(),
        }
    }
}

fn uv_asset() -> &'static str {
    match (cfg!(windows), cfg!(target_arch = "aarch64")) {
        (true, false) => "uv-x86_64-pc-windows-msvc.zip",
        (true, true) => "uv-aarch64-pc-windows-msvc.zip",
        (false, false) => "uv-x86_64-unknown-linux-gnu.tar.gz",
        (false, true) => "uv-aarch64-unknown-linux-gnu.tar.gz",
    }
}

fn get_uv(ctl: &Ctl) -> Result<(), String> {
    let tools = data_dir().join("tools");
    let unpack = tools.join("uv-unpack");
    let _ = std::fs::remove_dir_all(&unpack);
    std::fs::create_dir_all(&unpack).map_err(|e| e.to_string())?;
    let asset = uv_asset();
    let archive = tools.join(asset);
    let url = format!("https://github.com/astral-sh/uv/releases/download/{UV_VERSION}/{asset}");
    let size = content_length(&url).unwrap_or(0);
    download_all(&[(url, archive.clone(), size)], ctl)?;
    // `tar` ships with Linux and with Windows 10+ (it also unpacks .zip there)
    let mut c = Command::new("tar");
    c.arg("-xf").arg(&archive).arg("-C").arg(&unpack).stdout(Stdio::null()).stderr(Stdio::piped());
    let child = spawn(c, ctl)?;
    wait(child, ctl, || {})?;
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

/// Download files with `curl` (bundled with Linux distros and Windows 10+), reporting
/// combined progress, size and speed. Each file goes to `<dest>.part` and is renamed when complete.
fn download_all(files: &[(String, PathBuf, u64)], ctl: &Ctl) -> Result<(), String> {
    let total: u64 = files.iter().map(|f| f.2).sum();
    let mut done: u64 = 0;
    let mut meter = Meter::new();
    for (url, dest, size) in files {
        if let Some(p) = dest.parent() {
            std::fs::create_dir_all(p).map_err(|e| e.to_string())?;
        }
        let mut part = dest.clone().into_os_string();
        part.push(".part");
        let part = PathBuf::from(part);
        let mut c = Command::new("curl");
        c.args(["-fsSL", "--retry", "3", "-o"]).arg(&part).arg(url).stdout(Stdio::null()).stderr(Stdio::piped());
        let child = spawn(c, ctl)?;
        let r = wait(child, ctl, || {
            let have = std::fs::metadata(&part).map(|m| m.len()).unwrap_or(0);
            let now = done + have;
            meter.add_total(now);
            if total > 0 {
                ctl.emit(now as f32 / total as f32, format!("{:.1} of {:.1} MB{}", now as f32 / 1e6, total as f32 / 1e6, meter.speed_text()));
            } else {
                ctl.emit(0.5, format!("{:.1} MB{}", now as f32 / 1e6, meter.speed_text()));
            }
        });
        if let Err(e) = r {
            let _ = std::fs::remove_file(&part);
            return Err(if e == CANCELLED { e } else { format!("download failed: {url}\n{e}") });
        }
        std::fs::rename(&part, dest).map_err(|e| e.to_string())?;
        done += size.max(&std::fs::metadata(dest).map(|m| m.len()).unwrap_or(0)).to_owned();
    }
    Ok(())
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
    #[test]
    fn parses_uv_sizes() {
        assert!((super::parse_size("16.0MiB") - 16.777).abs() < 0.01);
        assert!((super::parse_size("512KiB") - 0.524).abs() < 0.01);
    }

    /// Full install into an empty data folder, printing the progress the UI would show. Run with:
    ///   XDG_DATA_HOME=/tmp/x/data XDG_CONFIG_HOME=/tmp/x/config cargo test fresh_install -- --ignored --nocapture
    #[test]
    #[ignore]
    fn fresh_install() {
        let engine = std::env::var("ENGINE").unwrap_or_else(|_| "kokoro".into());
        let t0 = std::time::Instant::now();
        let inst = super::start(&engine, eframe::egui::Context::default());
        let mut plan = Vec::new();
        let mut last_print = (usize::MAX, -1.0f32);
        let mut overall_prev = 0.0f32;
        for ev in inst.rx.iter() {
            match ev {
                super::Ev::Plan(p) => {
                    eprintln!("PLAN: {}", p.iter().map(|s| format!("{} ({:.0})", s.label, s.weight)).collect::<Vec<_>>().join(" → "));
                    plan = p;
                }
                super::Ev::Step { i, frac, detail } => {
                    let tw: f32 = plan.iter().map(|s| s.weight).sum();
                    let overall = (plan[..i].iter().map(|s| s.weight).sum::<f32>() + plan[i].weight * frac) / tw;
                    assert!(overall + 1e-4 >= overall_prev || frac == 0.0, "overall progress went backwards");
                    overall_prev = overall_prev.max(overall);
                    if i != last_print.0 || frac - last_print.1 >= 0.2 || frac >= 1.0 && last_print.1 < 1.0 {
                        eprintln!("[{:5.1}s] {:3.0}% overall | step {} {:3.0}% {}", t0.elapsed().as_secs_f32(), overall * 100.0, i + 1, frac * 100.0, detail);
                        last_print = (i, frac);
                    }
                }
                super::Ev::Done => {
                    eprintln!("[{:5.1}s] DONE", t0.elapsed().as_secs_f32());
                    break;
                }
                super::Ev::Cancelled => panic!("cancelled"),
                super::Ev::Failed(e) => panic!("install failed: {e}"),
            }
        }
        assert!(super::is_installed(&engine), "probe says not installed");
    }

    /// Cancel mid-download and check nothing half-finished is left behind.
    #[test]
    #[ignore]
    fn cancel_cleans_up() {
        let inst = super::start("kokoro", eframe::egui::Context::default());
        let t0 = std::time::Instant::now();
        let mut cancelled = false;
        for ev in inst.rx.iter() {
            if let super::Ev::Step { frac, .. } = &ev {
                if !cancelled && *frac > 0.05 && t0.elapsed().as_secs() > 2 {
                    inst.cancel();
                    cancelled = true;
                }
            }
            match ev {
                super::Ev::Cancelled => break,
                super::Ev::Done => panic!("finished before cancel"),
                super::Ev::Failed(e) => panic!("failed instead of cancelled: {e}"),
                _ => {}
            }
        }
        fn parts(d: &std::path::Path) -> usize {
            std::fs::read_dir(d).into_iter().flatten().flatten().map(|e| {
                let p = e.path();
                if p.is_dir() { parts(&p) } else { p.extension().is_some_and(|x| x == "part") as usize }
            }).sum()
        }
        assert_eq!(parts(&crate::book::data_dir()), 0, ".part files left behind");
        assert!(!super::is_installed("kokoro"));
    }
}
