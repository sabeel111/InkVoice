#![cfg_attr(windows, windows_subsystem = "windows")] // no console window on Windows
mod book;
mod config;
mod installer;
mod player;
mod theme;
mod tts;
mod ui;

use book::{data_dir, Book};
use config::Config;
use eframe::egui;
use player::{Player, Unit};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::time::Instant;

const WORKERS: &[(&str, &str)] = &[
    ("common.py", include_str!("../workers/common.py")),
    ("phonon_worker.py", include_str!("../workers/phonon_worker.py")),
    ("woof_worker.py", include_str!("../workers/woof_worker.py")),
    ("kokoro_worker.py", include_str!("../workers/kokoro_worker.py")),
];

#[derive(PartialEq, Clone, Copy)]
enum View {
    Library,
    Reader,
}

/// A generated audio file on disk, for the library's audio list.
struct AudioItem {
    profile: String,
    path: PathBuf,
    unit: Unit,
    label: String,
    duration: f32,
}

struct Job {
    path: PathBuf,
    progress: f32,
    started: Instant,
}

/// Audio being streamed into the player while it is generated.
struct Feed {
    out: PathBuf,
    pcm: PathBuf,
    read: u64,
}

pub const SPEEDS: &[f32] = &[0.75, 0.9, 1.0, 1.1, 1.25, 1.5, 1.75, 2.0];

struct Status {
    text: String,
    error: bool,
    at: Instant,
}

pub struct App {
    cfg: Config,
    lib: Vec<Book>,
    /// book open in the reader
    open: Option<String>,
    /// book selected in the library (details panel)
    sel: Option<String>,
    ch: usize,
    pg: usize,
    view: View,
    tts: tts::Tts,
    /// queued + running jobs: wav path -> label
    pending: HashMap<PathBuf, String>,
    job: Option<Job>,
    autoplay: Option<PathBuf>,
    import_rx: Receiver<Result<Book, String>>,
    import_tx: Sender<Result<Book, String>>,
    importing: bool,
    player: Option<Player>,
    status: Option<Status>,
    ctx: egui::Context,
    scroll_top: bool,
    /// ready wav files of the open book for the current voice profile
    have: HashSet<PathBuf>,
    lib_audio: HashMap<String, Vec<AudioItem>>,
    confirm_remove: Option<String>,
    covers: HashMap<String, Option<egui::TextureHandle>>,
    search: String,
    show_settings: bool,
    worker_dirty: bool,
    follow_target: Option<(usize, usize)>,
    seek_drag: Option<f32>,
    sidebar: bool,
    /// last highlighted word we scrolled to
    last_word: Option<usize>,
    /// voice preview being generated
    preview: Option<PathBuf>,
    feed: Option<Feed>,
    /// voice engine being installed + its current step / progress
    install: Option<installer::Install>,
    install_step: String,
    install_progress: Option<f32>,
    installed: HashSet<String>,
    confirm_uninstall: Option<String>,
}

impl App {
    fn new(cc: &eframe::CreationContext, open_file: Option<PathBuf>) -> Self {
        theme::install(&cc.egui_ctx);
        install_workers();
        let cfg = Config::load();
        cfg.save();
        let (import_tx, import_rx) = channel();
        let tts = tts::Tts::start(cfg.worker_command(), cc.egui_ctx.clone());
        let mut app = Self {
            lib: book::load_library(),
            open: None,
            sel: None,
            ch: 0,
            pg: 0,
            view: View::Library,
            tts,
            pending: HashMap::new(),
            job: None,
            autoplay: None,
            import_rx,
            import_tx,
            importing: false,
            player: Player::new(cfg.volume, cfg.speed),
            status: None,
            ctx: cc.egui_ctx.clone(),
            scroll_top: false,
            have: HashSet::new(),
            lib_audio: HashMap::new(),
            confirm_remove: None,
            covers: HashMap::new(),
            search: String::new(),
            show_settings: false,
            worker_dirty: false,
            follow_target: None,
            seek_drag: None,
            sidebar: true,
            last_word: None,
            preview: None,
            feed: None,
            install: None,
            install_step: String::new(),
            install_progress: None,
            installed: installer::SPECS.iter().filter(|s| installer::is_installed(s.name)).map(|s| s.name.to_string()).collect(),
            confirm_uninstall: None,
            cfg,
        };
        app.sel = app.lib.first().map(|b| b.id.clone());
        // use an installed engine if the selected one isn't installed
        if !app.installed.contains(&app.cfg.engine) {
            if let Some(e) = installer::SPECS.iter().find(|s| app.installed.contains(s.name)) {
                app.cfg.engine = e.name.to_string();
                app.cfg.save();
                app.tts = tts::Tts::start(app.cfg.worker_command(), app.ctx.clone());
            }
        }
        app.refresh_lib_audio();
        if app.player.is_none() {
            app.notify_err("No audio output device found");
        }
        if let Some(p) = open_file {
            app.import_path(p);
        }
        app
    }

    // ---------- lookups ----------

    fn book(&self, id: &str) -> Option<&Book> {
        self.lib.iter().find(|b| b.id == id)
    }
    fn open_book(&self) -> Option<&Book> {
        self.open.as_deref().and_then(|id| self.book(id))
    }
    fn open_book_mut(&mut self) -> Option<&mut Book> {
        let id = self.open.clone()?;
        self.lib.iter_mut().find(|b| b.id == id)
    }

    fn audio_path(&self, b: &Book, ch: usize, pg: Option<usize>) -> PathBuf {
        let name = match pg {
            None => format!("c{ch:04}.wav"),
            Some(p) => format!("c{ch:04}_p{p:04}.wav"),
        };
        b.dir().join("audio").join(self.cfg.profile()).join(name)
    }
    fn unit_path(&self, u: &Unit) -> Option<PathBuf> {
        Some(self.audio_path(self.book(&u.book)?, u.ch, u.pg))
    }
    fn unit_label(&self, u: &Unit) -> String {
        let Some(b) = self.book(&u.book) else { return String::new() };
        let Some(c) = b.chapters.get(u.ch) else { return String::new() };
        match u.pg {
            None => c.title.clone(),
            Some(p) => format!("{} · page {}", c.title, p + 1),
        }
    }
    fn unit_text(&self, u: &Unit) -> Option<String> {
        let b = self.book(&u.book)?;
        Some(match u.pg {
            None => b.chapter_text(u.ch),
            Some(p) => b.chapters.get(u.ch)?.pages.get(p)?.clone(),
        })
    }
    /// The unit the reader is on, in the current voice mode.
    fn reader_unit(&self) -> Option<Unit> {
        Some(Unit { book: self.open.clone()?, ch: self.ch, pg: self.cfg.page_mode.then_some(self.pg) })
    }
    fn next_unit(&self, u: &Unit) -> Option<Unit> {
        let b = self.book(&u.book)?;
        let mut n = u.clone();
        match u.pg {
            None => n.ch += 1,
            Some(p) if p + 1 < b.chapters[u.ch].pages.len() => n.pg = Some(p + 1),
            Some(_) => {
                n.ch += 1;
                n.pg = Some(0);
            }
        }
        (n.ch < b.chapters.len()).then_some(n)
    }
    fn prev_unit(&self, u: &Unit) -> Option<Unit> {
        let b = self.book(&u.book)?;
        let mut n = u.clone();
        match u.pg {
            None => n.ch = u.ch.checked_sub(1)?,
            Some(p) if p > 0 => n.pg = Some(p - 1),
            Some(_) => {
                n.ch = u.ch.checked_sub(1)?;
                n.pg = Some(b.chapters[n.ch].pages.len() - 1);
            }
        }
        Some(n)
    }
    fn is_ready(&self, path: &Path) -> bool {
        self.have.contains(path) || path.exists()
    }

    // ---------- status ----------

    fn notify(&mut self, text: impl Into<String>) {
        self.status = Some(Status { text: text.into(), error: false, at: Instant::now() });
    }
    fn notify_err(&mut self, text: impl Into<String>) {
        self.status = Some(Status { text: text.into(), error: true, at: Instant::now() });
    }

    // ---------- library ----------

    fn refresh_have(&mut self) {
        self.have.clear();
        let Some(b) = self.open_book() else { return };
        let dir = b.dir().join("audio").join(self.cfg.profile());
        if let Ok(rd) = std::fs::read_dir(dir) {
            self.have = rd.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|e| e == "wav")).collect();
        }
    }

    fn refresh_lib_audio(&mut self) {
        let map = self.lib.iter().map(|b| (b.id.clone(), scan_audio(b))).collect();
        self.lib_audio = map;
    }

    fn open_reader(&mut self, id: &str) {
        let Some(b) = self.lib.iter_mut().find(|b| b.id == id) else { return };
        b.opened = book::now_secs();
        b.save_progress();
        let (ch, pg) = b.pos;
        self.open = Some(id.to_string());
        self.sel = Some(id.to_string());
        self.ch = ch;
        self.pg = pg;
        self.scroll_top = true;
        self.follow_target = None;
        self.refresh_have();
        self.view = View::Reader;
    }

    fn go(&mut self, ch: usize, pg: usize) {
        self.ch = ch;
        self.pg = pg;
        self.scroll_top = true;
        if let Some(b) = self.open_book_mut() {
            b.pos = (ch, pg);
            b.save_progress();
        }
    }
    fn next_page(&mut self) {
        let Some(b) = self.open_book() else { return };
        if self.pg + 1 < b.chapters[self.ch].pages.len() {
            self.go(self.ch, self.pg + 1);
        } else if self.ch + 1 < b.chapters.len() {
            self.go(self.ch + 1, 0);
        }
    }
    fn prev_page(&mut self) {
        if self.pg > 0 {
            self.go(self.ch, self.pg - 1);
        } else if self.ch > 0 {
            let last = self.open_book().unwrap().chapters[self.ch - 1].pages.len() - 1;
            self.go(self.ch - 1, last);
        }
    }

    fn remove_book(&mut self, id: &str) {
        let Some(i) = self.lib.iter().position(|b| b.id == id) else { return };
        let b = self.lib.remove(i);
        if self.player.as_ref().and_then(|p| p.now.as_ref()).is_some_and(|n| n.unit.book == b.id) {
            if let Some(p) = &mut self.player { p.stop(); }
        }
        if self.pending.keys().any(|p| p.starts_with(b.dir())) {
            self.cancel_generation();
        }
        let _ = std::fs::remove_dir_all(b.dir());
        self.lib_audio.remove(&b.id);
        self.covers.remove(&b.id);
        if self.open.as_deref() == Some(id) {
            self.open = None;
        }
        self.sel = self.lib.first().map(|b| b.id.clone());
        self.confirm_remove = None;
        self.notify(format!("Removed “{}”", b.title));
    }

    fn import_dialog(&mut self) {
        if self.importing {
            return;
        }
        self.importing = true;
        let (tx, ctx) = (self.import_tx.clone(), self.ctx.clone());
        std::thread::spawn(move || {
            let r = match rfd::FileDialog::new().add_filter("Books", &["epub", "pdf"]).pick_file() {
                Some(p) => book::import(&p).map_err(|e| e.to_string()),
                None => Err(String::new()),
            };
            let _ = tx.send(r);
            ctx.request_repaint();
        });
    }
    fn import_path(&mut self, p: PathBuf) {
        self.importing = true;
        let (tx, ctx) = (self.import_tx.clone(), self.ctx.clone());
        std::thread::spawn(move || {
            let _ = tx.send(book::import(&p).map_err(|e| e.to_string()));
            ctx.request_repaint();
        });
    }

    // ---------- generation ----------

    /// Queue audio for a unit unless it exists or is queued. Returns its path.
    fn enqueue(&mut self, u: &Unit) -> Option<PathBuf> {
        if !self.installed.contains(&self.cfg.engine) {
            return None;
        }
        let path = self.unit_path(u)?;
        if self.is_ready(&path) || self.pending.contains_key(&path) {
            return Some(path);
        }
        let text = self.unit_text(u)?;
        let _ = std::fs::create_dir_all(path.parent()?);
        let label = self.unit_label(u);
        self.pending.insert(path.clone(), label);
        self.tts.submit(tts::Job { text, out: path.clone(), stream: None }, false);
        Some(path)
    }

    /// Generate a unit with streaming, ahead of the queue, starting at `start_char`.
    fn enqueue_stream(&mut self, u: &Unit, start_char: usize) -> Option<PathBuf> {
        let path = self.unit_path(u)?;
        if self.job.as_ref().is_some_and(|j| j.path == path) {
            return Some(path); // already being made: play when done
        }
        let text = self.unit_text(u)?;
        let _ = std::fs::create_dir_all(path.parent()?);
        let label = self.unit_label(u);
        self.pending.insert(path.clone(), label);
        self.tts.submit(tts::Job { text, out: path.clone(), stream: Some(start_char) }, true);
        Some(path)
    }

    fn generate_all(&mut self) {
        if !self.engine_ready() {
            return;
        }
        let Some(b) = self.open_book() else { return };
        let id = b.id.clone();
        let shape: Vec<usize> = b.chapters.iter().map(|c| c.pages.len()).collect();
        let mut n = 0;
        for (ch, pages) in shape.into_iter().enumerate() {
            let units: Vec<Option<usize>> = if self.cfg.page_mode { (0..pages).map(Some).collect() } else { vec![None] };
            for pg in units {
                let u = Unit { book: id.clone(), ch, pg };
                if let Some(p) = self.unit_path(&u) {
                    if !self.is_ready(&p) && !self.pending.contains_key(&p) {
                        self.enqueue(&u);
                        n += 1;
                    }
                }
            }
        }
        if n == 0 {
            self.notify("Everything is already generated for this voice");
        } else {
            self.notify(format!("Queued {n} {}", if self.cfg.page_mode { "pages" } else { "chapters" }));
        }
    }

    fn set_speed(&mut self, v: f32) {
        self.cfg.speed = v;
        if let Some(p) = &mut self.player {
            p.set_speed(v);
        }
        self.cfg.save();
    }

    /// Throw away a chapter's audio (current voice) and generate it again with the current settings.
    fn regenerate_chapter(&mut self, ch: usize) {
        if !self.engine_ready() {
            return;
        }
        let Some(b) = self.open_book() else { return };
        let id = b.id.clone();
        let pages = b.chapters[ch].pages.len();
        let units: Vec<Unit> = if self.cfg.page_mode {
            (0..pages).map(|p| Unit { book: id.clone(), ch, pg: Some(p) }).collect()
        } else {
            vec![Unit { book: id.clone(), ch, pg: None }]
        };
        for u in &units {
            let Some(path) = self.unit_path(u) else { continue };
            if self.pending.contains_key(&path) {
                continue; // already being made
            }
            if self.player.as_ref().and_then(|p| p.now.as_ref()).is_some_and(|n| n.path == path) {
                if let Some(p) = &mut self.player {
                    p.stop();
                }
            }
            let _ = std::fs::remove_file(&path);
            let _ = std::fs::remove_file(player::timing_path(&path));
            self.have.remove(&path);
            self.enqueue(u);
        }
        let title = self.unit_label(&Unit { book: id.clone(), ch, pg: None });
        if let Some(b) = self.book(&id) {
            let items = scan_audio(b);
            self.lib_audio.insert(id, items);
        }
        self.notify(format!("Regenerating {title} with the current settings"));
    }

    fn cancel_generation(&mut self) {
        if self.feed.is_some() {
            if let Some(p) = &mut self.player {
                p.stop();
            }
            self.feed = None;
        }
        self.tts.cancel();
        self.pending.clear();
        self.job = None;
        self.autoplay = None;
    }

    fn engine_ready(&mut self) -> bool {
        if self.installed.contains(&self.cfg.engine) {
            return true;
        }
        self.notify_err("Install a voice engine first (Settings → Voice engines)");
        self.show_settings = true;
        false
    }

    fn install_engine(&mut self, name: &str) {
        if self.install.is_none() {
            self.install_step = "Starting…".into();
            self.install_progress = None;
            self.install = Some(installer::start(name, self.ctx.clone()));
        }
    }

    fn uninstall_engine(&mut self, name: &str) {
        if self.cfg.engine == name {
            self.cancel_generation();
        }
        installer::remove(name);
        self.installed.remove(name);
        self.confirm_uninstall = None;
        if self.cfg.engine == name {
            if let Some(e) = installer::SPECS.iter().find(|s| self.installed.contains(s.name)) {
                self.cfg.engine = e.name.to_string();
                self.worker_dirty = true;
                self.apply_worker_settings();
            }
        }
        self.notify(format!("Removed {}", installer::spec(name).map_or(name, |s| s.title)));
    }

    fn poll_install(&mut self) {
        let Some(inst) = &self.install else { return };
        let mut finished = None;
        while let Ok(ev) = inst.rx.try_recv() {
            match ev {
                installer::Ev::Step(s) => self.install_step = s,
                installer::Ev::Progress(p) => self.install_progress = p,
                installer::Ev::Done => finished = Some(Ok(())),
                installer::Ev::Failed(e) => finished = Some(Err(e)),
            }
        }
        if let Some(r) = finished {
            let name = inst.engine.clone();
            let title = installer::spec(&name).map_or(name.clone(), |s| s.title.to_string());
            self.install = None;
            match r {
                Ok(()) if installer::is_installed(&name) => {
                    self.installed.insert(name.clone());
                    // first engine, or the selected one wasn't installed: switch to it
                    if !self.installed.contains(&self.cfg.engine) || self.installed.len() == 1 {
                        self.cfg.engine = name;
                        self.worker_dirty = true;
                        self.apply_worker_settings();
                    }
                    self.notify(format!("{title} is ready"));
                }
                Ok(()) => self.notify_err(format!("{title} install finished but files are missing — try again")),
                Err(e) => self.notify_err(format!("Installing {title} failed: {e}")),
            }
        }
    }

    /// Restart the worker after the engine, voice or pacing changed.
    fn apply_worker_settings(&mut self) {
        if !self.worker_dirty {
            return;
        }
        self.worker_dirty = false;
        let had_jobs = !self.pending.is_empty();
        self.cancel_generation();
        self.tts = tts::Tts::start(self.cfg.worker_command(), self.ctx.clone());
        self.cfg.save();
        self.refresh_have();
        if had_jobs {
            self.notify("Voice settings changed — the generation queue was cleared");
        }
    }

    // ---------- playback ----------

    /// Play a unit, generating it first if needed. `from_page` starts at that page of a chapter.
    fn play_unit(&mut self, u: Unit, from_page: Option<usize>) {
        let start = match (u.pg, from_page) {
            (None, Some(p)) if p > 0 => self.book(&u.book).and_then(|b| b.page_char_starts(u.ch).get(p).copied()),
            _ => None,
        };
        self.play_from_char(u, start);
    }

    /// Play a unit from the word at `start_char` (chars into the unit's text).
    fn play_from_char(&mut self, u: Unit, start_char: Option<usize>) {
        let Some(path) = self.unit_path(&u) else { return };
        if self.is_ready(&path) {
            self.play_path(&path, u, start_char);
        } else if self.engine_ready() {
            if let Some(p) = &mut self.player {
                p.stop();
            }
            self.feed = None;
            self.autoplay = self.enqueue_stream(&u, start_char.unwrap_or(0));
        }
    }

    /// Play an existing audio file (any voice profile).
    fn play_path(&mut self, path: &Path, u: Unit, start_char: Option<usize>) {
        let label = format!("{} — {}", self.book(&u.book).map(|b| b.title.as_str()).unwrap_or(""), self.unit_label(&u));
        let Some(player) = &mut self.player else {
            self.notify_err("No audio output device found");
            return;
        };
        match player.play(path, label, u.clone()) {
            Ok(()) => {
                if let Some(t) = start_char.and_then(|c| player.time_of_char(c)) {
                    player.seek(t);
                }
                self.follow_target = None;
                self.last_word = None;
                // prepare the next chapter/page while this one plays (current voice only)
                if self.cfg.prefetch && self.unit_path(&u).as_deref() == Some(path) {
                    if let Some(n) = self.next_unit(&u) {
                        self.enqueue(&n);
                    }
                }
            }
            Err(e) => self.notify_err(format!("Can't play audio: {e}")),
        }
    }

    /// Generate (once) and play a short sample of the selected voice.
    fn preview_voice(&mut self) {
        if !self.engine_ready() {
            return;
        }
        self.apply_worker_settings();
        let path = data_dir().join("previews").join(format!("{}.wav", self.cfg.profile()));
        if path.exists() {
            if let Some(p) = &mut self.player {
                let _ = p.play(&path, "Voice preview".into(), Unit { book: String::new(), ch: 0, pg: None });
            }
            return;
        }
        let _ = std::fs::create_dir_all(path.parent().unwrap());
        let text = "The lamps along the river had already been lit when she reached the bridge. \
                    “You came back,” he said quietly. She smiled, and for a moment neither of them spoke.";
        self.pending.insert(path.clone(), "voice preview".into());
        self.tts.submit(tts::Job { text: text.into(), out: path.clone(), stream: None }, true);
        self.preview = Some(path);
    }

    fn toggle_play(&mut self) {
        if let Some(p) = &mut self.player {
            if p.now.is_some() {
                p.toggle();
                return;
            }
        }
        if let Some(u) = self.reader_unit() {
            let page = self.pg;
            self.play_unit(u, Some(page));
        }
    }

    fn skip_unit(&mut self, forward: bool) {
        let Some(u) = self.player.as_ref().and_then(|p| p.now.as_ref()).map(|n| n.unit.clone()) else { return };
        if !forward && self.player.as_ref().is_some_and(|p| p.pos() > 3.0) {
            if let Some(p) = &mut self.player { p.seek(0.0); }
            return;
        }
        let n = if forward { self.next_unit(&u) } else { self.prev_unit(&u) };
        if let Some(n) = n {
            self.show_unit(&n);
            self.play_unit(n, None);
        }
    }

    /// Move the reader to a unit if it is in the open book.
    fn show_unit(&mut self, u: &Unit) {
        if self.open.as_deref() == Some(u.book.as_str()) && self.cfg.follow {
            self.go(u.ch, u.pg.unwrap_or(0));
        }
    }

    fn seek_by(&mut self, d: f32) {
        if let Some(p) = &mut self.player {
            let t = p.pos() + d;
            p.seek(t);
        }
    }

    // ---------- per-frame ----------

    fn poll(&mut self) {
        self.poll_install();
        let mut changed_books = HashSet::new();
        while let Ok(ev) = self.tts.rx.try_recv() {
            match ev {
                tts::Event::Started(p) => {
                    if self.pending.contains_key(&p) {
                        self.job = Some(Job { path: p, progress: 0.0, started: Instant::now() });
                    }
                }
                tts::Event::StreamStart { out, pcm, sr } => {
                    if self.autoplay.as_ref() == Some(&out) {
                        self.autoplay = None;
                        if let (Some(u), Some(player)) = (unit_of(&out), self.player.as_mut()) {
                            let label = format!(
                                "{} — {}",
                                self.lib.iter().find(|b| b.id == u.book).map(|b| b.title.as_str()).unwrap_or(""),
                                self.pending.get(&out).cloned().unwrap_or_default()
                            );
                            if player.play_stream(&out, sr, self.cfg.stream_buffer, label, u).is_ok() {
                                self.feed = Some(Feed { out, pcm, read: 0 });
                                self.follow_target = None;
                                self.last_word = None;
                            }
                        }
                    }
                }
                tts::Event::Chunk { out, samples, words } => {
                    self.feed_chunk(&out, samples, words);
                }
                tts::Event::Progress(p, f) => {
                    if let Some(j) = self.job.as_mut().filter(|j| j.path == p) {
                        j.progress = f;
                    }
                    if self.feed.as_ref().is_some_and(|fd| fd.out == p) {
                        if let Some(pl) = &mut self.player {
                            let b = pl.buffered();
                            if let Some(n) = pl.now.as_mut() {
                                n.est_total = (f > 0.02).then(|| b / f);
                            }
                        }
                    }
                }
                tts::Event::Done { out: p, stream_offset } => {
                    if self.feed.as_ref().is_some_and(|fd| fd.out == p) {
                        self.feed = None;
                        if let Some(pl) = &mut self.player {
                            if pl.now.as_ref().is_some_and(|n| n.path == p) {
                                pl.stream_finish(stream_offset);
                            }
                        }
                    }
                    self.pending.remove(&p);
                    if self.job.as_ref().is_some_and(|j| j.path == p) {
                        self.job = None;
                    }
                    self.have.insert(p.clone());
                    if let Some(id) = book_id_of(&p) {
                        changed_books.insert(id);
                    }
                    if self.autoplay.as_ref() == Some(&p) {
                        self.autoplay = None;
                        if let Some(u) = unit_of(&p) {
                            self.play_path(&p, u, None);
                        }
                    }
                    if self.preview.as_ref() == Some(&p) {
                        self.preview = None;
                        if let Some(pl) = &mut self.player {
                            let _ = pl.play(&p, "Voice preview".into(), Unit { book: String::new(), ch: 0, pg: None });
                        }
                    }
                }
                tts::Event::Failed(p, e) => {
                    if self.feed.as_ref().is_some_and(|fd| fd.out == p) {
                        self.feed = None;
                        if let Some(pl) = &mut self.player {
                            pl.stop();
                        }
                    }
                    let was_wanted = self.pending.remove(&p).is_some();
                    if self.job.as_ref().is_some_and(|j| j.path == p) {
                        self.job = None;
                    }
                    if self.autoplay.as_ref() == Some(&p) {
                        self.autoplay = None;
                    }
                    if was_wanted {
                        self.notify_err(format!("Voice generation failed: {e}"));
                    }
                }
            }
        }
        for id in changed_books {
            if let Some(b) = self.book(&id) {
                let items = scan_audio(b);
                self.lib_audio.insert(id, items);
            }
        }

        while let Ok(r) = self.import_rx.try_recv() {
            self.importing = false;
            match r {
                Ok(b) => {
                    let id = b.id.clone();
                    self.notify(format!("Imported “{}”", b.title));
                    self.lib.retain(|x| x.id != id);
                    self.covers.remove(&id);
                    self.lib.push(b);
                    self.lib_audio.insert(id.clone(), scan_audio(self.book(&id).unwrap()));
                    self.open_reader(&id);
                    book::sort_library(&mut self.lib);
                }
                Err(e) if e.is_empty() => {}
                Err(e) => self.notify_err(format!("Import failed: {e}")),
            }
        }

        // audio ran out: continue with the next chapter/page
        if self.player.as_ref().is_some_and(|p| p.finished()) {
            let u = self.player.as_ref().unwrap().now.as_ref().unwrap().unit.clone();
            self.player.as_mut().unwrap().stop();
            if self.cfg.continuous {
                if let Some(n) = self.next_unit(&u) {
                    self.show_unit(&n);
                    self.play_unit(n, None);
                }
            }
        }

        // follow along: turn the page when the spoken word moves onto another page
        if let Some(target) = self.spoken_page() {
            if self.follow_target != Some(target) {
                self.follow_target = Some(target);
                if self.cfg.follow && self.view == View::Reader && (self.ch, self.pg) != target {
                    self.go(target.0, target.1);
                }
            }
        }
    }

    /// Append newly streamed audio to the player.
    fn feed_chunk(&mut self, out: &Path, samples: usize, words: Vec<player::Word>) {
        let Some(feed) = self.feed.as_mut().filter(|f| f.out == out) else { return };
        let Some(pl) = self.player.as_mut().filter(|p| p.now.as_ref().is_some_and(|n| n.path == out)) else {
            self.feed = None; // user switched to something else
            return;
        };
        use std::io::{Read, Seek, SeekFrom};
        let mut buf = vec![0u8; samples * 2];
        let ok = std::fs::File::open(&feed.pcm)
            .and_then(|mut f| f.seek(SeekFrom::Start(feed.read)).and_then(|_| f.read_exact(&mut buf)))
            .is_ok();
        if ok {
            feed.read += buf.len() as u64;
            pl.stream_append(player::pcm_to_f32(&buf), words);
        }
    }

    /// (chapter, page) of the word currently being spoken, if it is in the open book.
    fn spoken_page(&self) -> Option<(usize, usize)> {
        let p = self.player.as_ref()?;
        let now = p.now.as_ref()?;
        if self.open.as_deref() != Some(now.unit.book.as_str()) || !p.is_playing() {
            return None;
        }
        match now.unit.pg {
            Some(pg) => Some((now.unit.ch, pg)),
            None => {
                let w = now.words.get(p.current_word()?)?;
                let starts = self.open_book()?.page_char_starts(now.unit.ch);
                let pg = starts.partition_point(|&s| s <= w.0).saturating_sub(1);
                Some((now.unit.ch, pg))
            }
        }
    }

    fn cover(&mut self, b_id: &str) -> Option<egui::TextureHandle> {
        if let Some(t) = self.covers.get(b_id) {
            return t.clone();
        }
        let tex = self.book(b_id).and_then(|b| {
            let img = image::open(b.cover_path()).ok()?.to_rgba8();
            let size = [img.width() as usize, img.height() as usize];
            let ci = egui::ColorImage::from_rgba_unmultiplied(size, img.as_raw());
            Some(self.ctx.load_texture(format!("cover-{b_id}"), ci, egui::TextureOptions::LINEAR))
        });
        self.covers.insert(b_id.to_string(), tex.clone());
        tex
    }
}

/// Write the bundled worker scripts next to the data (kept in sync with this build).
fn install_workers() {
    let wd = data_dir().join("workers");
    let _ = std::fs::create_dir_all(&wd);
    for (name, src) in WORKERS {
        let p = wd.join(name);
        if std::fs::read_to_string(&p).ok().as_deref() != Some(*src) {
            let _ = std::fs::write(p, src);
        }
    }
}

/// .../books/<id>/audio/<profile>/c0001.wav -> id
fn book_id_of(p: &Path) -> Option<String> {
    Some(p.parent()?.parent()?.parent()?.file_name()?.to_string_lossy().into_owned())
}

fn parse_unit_name(stem: &str) -> Option<(usize, Option<usize>)> {
    let rest = stem.strip_prefix('c')?;
    Some(match rest.split_once("_p") {
        Some((c, p)) => (c.parse().ok()?, Some(p.parse().ok()?)),
        None => (rest.parse().ok()?, None),
    })
}

fn unit_of(p: &Path) -> Option<Unit> {
    let (ch, pg) = parse_unit_name(p.file_stem()?.to_str()?)?;
    Some(Unit { book: book_id_of(p)?, ch, pg })
}

fn scan_audio(b: &Book) -> Vec<AudioItem> {
    let mut out = Vec::new();
    let Ok(profiles) = std::fs::read_dir(b.dir().join("audio")) else { return out };
    let mut profiles: Vec<PathBuf> = profiles.flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect();
    profiles.sort();
    for dir in profiles {
        let profile = dir.file_name().unwrap().to_string_lossy().into_owned();
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        let mut paths: Vec<PathBuf> = rd.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|e| e == "wav")).collect();
        paths.sort();
        for path in paths {
            let Some((ch, pg)) = path.file_stem().and_then(|s| s.to_str()).and_then(parse_unit_name) else { continue };
            let Some(c) = b.chapters.get(ch) else { continue };
            let label = match pg {
                None => c.title.clone(),
                Some(p) => format!("{} · page {}", c.title, p + 1),
            };
            out.push(AudioItem {
                profile: profile.clone(),
                duration: player::wav_duration(&path).unwrap_or(0.0),
                unit: Unit { book: b.id.clone(), ch, pg },
                path,
                label,
            });
        }
    }
    out
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _: &mut eframe::Frame) {
        self.poll();
        if self.job.is_some() || self.install.is_some() || self.player.as_ref().is_some_and(|p| p.now.is_some()) || self.status.is_some() {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
        self.shortcuts(ctx);
        self.draw(ctx);
    }
}

fn main() -> eframe::Result {
    book::migrate_legacy();
    let args: Vec<String> = std::env::args().skip(1).collect();
    // headless import: `inkvoice --import book.epub`
    if args.first().map(String::as_str) == Some("--import") {
        for p in &args[1..] {
            match book::import(Path::new(p)) {
                Ok(b) => println!("imported: {} ({} chapters)", b.title, b.chapters.len()),
                Err(e) => eprintln!("{p}: {e}"),
            }
        }
        return Ok(());
    }
    let open_file = args.into_iter().find(|a| !a.starts_with("--")).map(PathBuf::from);

    let icon = image::load_from_memory(include_bytes!("../assets/inkvoice.png")).ok().map(|i| {
        let i = i.to_rgba8();
        egui::IconData { width: i.width(), height: i.height(), rgba: i.into_raw() }
    });
    let mut viewport = egui::ViewportBuilder::default();
    if let Some(icon) = icon {
        viewport = viewport.with_icon(std::sync::Arc::new(icon));
    }
    let opts = eframe::NativeOptions {
        viewport: viewport
            .with_inner_size([1180.0, 780.0])
            .with_min_inner_size([760.0, 480.0])
            .with_app_id("inkvoice"),
        // With vsync on, eglSwapBuffers blocks forever on Wayland while the window is hidden
        // (other workspace / covered), freezing the event loop -> Hyprland "not responding".
        vsync: false,
        ..Default::default()
    };
    eframe::run_native("InkVoice", opts, Box::new(move |cc| Ok(Box::new(App::new(cc, open_file)))))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parses_audio_names() {
        assert_eq!(parse_unit_name("c0003"), Some((3, None)));
        assert_eq!(parse_unit_name("c0001_p0012"), Some((1, Some(12))));
        assert_eq!(parse_unit_name("junk"), None);
        let u = unit_of(Path::new("/x/books/my_book/audio/phonon-everly/c0002_p0001.wav")).unwrap();
        assert_eq!(u, Unit { book: "my_book".into(), ch: 2, pg: Some(1) });
    }
    #[test]
    fn profile_labels() {
        assert_eq!(config::profile_label("phonon-everly"), "Phonon · Everly");
        assert_eq!(config::profile_label("phonon-elodie-rose"), "Phonon · Elodie-rose");
        assert_eq!(config::profile_label("phonon"), "Phonon (original voice)");
        assert_eq!(config::profile_label("kokoro-af_heart"), "Kokoro · Heart (US ♀)");
        assert_eq!(config::profile_label("kokoro-bm_george"), "Kokoro · George (UK ♂)");
    }
}

/// Renders the real UI to PNGs (no window). Run with:
///   XDG_DATA_HOME=... XDG_CONFIG_HOME=... SHOTS=dir cargo test --release screenshots -- --ignored
#[cfg(test)]
mod screenshots {
    use super::*;
    use egui_kittest::Harness;

    #[test]
    #[ignore]
    fn screenshots() {
        let Ok(dir) = std::env::var("SHOTS") else { return };
        let save = |h: &mut Harness<App>, name: &str| {
            h.run_steps(4);
            h.render().unwrap().save(format!("{dir}/{name}.png")).unwrap();
        };
        let mut h = Harness::builder().with_size([1280.0, 800.0]).build_eframe(|cc| App::new(cc, None));
        let id = h.state().lib[0].id.clone();
        h.state_mut().sel = Some(id.clone());
        save(&mut h, "1_library");

        let s = h.state_mut();
        s.open_reader(&id);
        s.cfg.page_mode = true;
        s.go(4, 0);
        s.play_unit(Unit { book: id.clone(), ch: 4, pg: Some(0) }, None);
        if let Some(p) = &mut s.player {
            p.seek(14.0);
        }
        save(&mut h, "2_reader_playing");

        let s = h.state_mut();
        let fake = std::path::PathBuf::from("/tmp/fake.wav");
        s.pending.insert(fake.clone(), "CHAPTER TWO".into());
        s.pending.insert("/tmp/fake2.wav".into(), "CHAPTER THREE".into());
        s.job = Some(Job { path: fake, progress: 0.42, started: Instant::now() - std::time::Duration::from_secs(40) });
        save(&mut h, "3_generating");

        h.state_mut().show_settings = true;
        h.state_mut().cfg.engine = "woof".into();
        h.state_mut().cfg.set_adv("temp", Some("0.650".into()));
        save(&mut h, "4_settings");
        h.state_mut().cfg.engine = "phonon".into();

        // streaming: play a page that has no audio yet; it must start before generation ends
        let s = h.state_mut();
        s.show_settings = false;
        s.pending.clear();
        s.job = None;
        s.cfg.speed = 1.25;
        s.go(4, 3);
        let t0 = Instant::now();
        s.play_unit(Unit { book: id.clone(), ch: 4, pg: Some(3) }, None);
        let mut started = None;
        while t0.elapsed().as_secs() < 90 {
            h.run_steps(1);
            std::thread::sleep(std::time::Duration::from_millis(100));
            let s = h.state();
            let p = s.player.as_ref().unwrap();
            if started.is_none() && p.now.as_ref().is_some_and(|n| n.streaming) && !p.is_buffering() && p.pos() > 0.5 {
                started = Some(t0.elapsed().as_secs_f32());
                eprintln!("STREAM: playing after {:.1}s, buffered {:.1}s, still generating: {}", started.unwrap(), p.buffered(), s.job.is_some());
                save(&mut h, "5_streaming");
            }
            if started.is_some() && h.state().job.is_none() {
                let p = h.state().player.as_ref().unwrap();
                eprintln!("STREAM: generation done after {:.1}s; switched to final file: {}; pos {:.1}s; words {}",
                    t0.elapsed().as_secs_f32(), !p.now.as_ref().unwrap().streaming, p.pos(), p.now.as_ref().unwrap().words.len());
                break;
            }
        }
        assert!(started.is_some(), "streaming never started");
    }
}
