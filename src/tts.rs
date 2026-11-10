//! TTS engines are external worker processes speaking JSON lines over stdin/stdout
//! (see workers/common.py). Swapping engine or voice = starting a different command.
use crate::player::Word;
use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::{Arc, Condvar, Mutex};

pub struct Job {
    pub text: String,
    pub out: PathBuf,
    /// Some(start_char): stream audio as it is made, starting at that char
    pub stream: Option<usize>,
}

pub enum Event {
    Started(PathBuf),
    /// streaming began: raw 16-bit PCM is appended to this file
    StreamStart { out: PathBuf, pcm: PathBuf, sr: u32 },
    /// a piece was streamed: `samples` new samples, word timings relative to stream start
    Chunk { out: PathBuf, samples: usize, words: Vec<Word> },
    Progress(PathBuf, f32),
    /// finished; `stream_offset` = where the streamed part starts in the final file (s)
    Done { out: PathBuf, stream_offset: f32 },
    Failed(PathBuf, String),
}

#[derive(Default)]
struct Queue {
    jobs: VecDeque<Job>,
    closed: bool,
}

pub struct Tts {
    queue: Arc<(Mutex<Queue>, Condvar)>,
    pub rx: std::sync::mpsc::Receiver<Event>,
    child: Arc<Mutex<Option<Child>>>,
}

impl Tts {
    pub fn start(cmd: Vec<String>, ctx: eframe::egui::Context) -> Self {
        let queue: Arc<(Mutex<Queue>, Condvar)> = Arc::default();
        let (ev_tx, rx) = std::sync::mpsc::channel::<Event>();
        let child: Arc<Mutex<Option<Child>>> = Arc::default();
        let (queue_t, child_t) = (queue.clone(), child.clone());
        std::thread::spawn(move || {
            let mut io: Option<(ChildStdin, BufReader<ChildStdout>)> = None;
            let mut id = 0u64;
            let send = |ev: Event| {
                let _ = ev_tx.send(ev);
                ctx.request_repaint();
            };
            loop {
                let job = {
                    let (lock, cv) = &*queue_t;
                    let mut q = lock.lock().unwrap();
                    loop {
                        if q.closed {
                            break None;
                        }
                        if let Some(j) = q.jobs.pop_front() {
                            break Some(j);
                        }
                        q = cv.wait(q).unwrap();
                    }
                };
                let Some(job) = job else { break };
                send(Event::Started(job.out.clone()));
                match run_job(&cmd, &child_t, &mut io, &mut id, &job, &send) {
                    Ok(stream_offset) => send(Event::Done { out: job.out, stream_offset }),
                    Err(e) => {
                        io = None;
                        if let Some(mut c) = child_t.lock().unwrap().take() {
                            let _ = c.kill();
                            let _ = c.wait();
                        }
                        send(Event::Failed(job.out, e));
                    }
                }
            }
            if let Some(mut c) = child_t.lock().unwrap().take() {
                let _ = c.kill();
                let _ = c.wait();
            }
        });
        Tts { queue, rx, child }
    }

    /// Queue a job; `front` puts it ahead of everything waiting (the one running is not interrupted).
    pub fn submit(&self, job: Job, front: bool) {
        let (lock, cv) = &*self.queue;
        let mut q = lock.lock().unwrap();
        q.jobs.retain(|j| j.out != job.out);
        if front { q.jobs.push_front(job) } else { q.jobs.push_back(job) }
        cv.notify_one();
    }

    /// Drop every queued job and stop the one in progress.
    pub fn cancel(&self) {
        self.queue.0.lock().unwrap().jobs.clear();
        if let Some(c) = self.child.lock().unwrap().as_mut() {
            let _ = c.kill();
        }
    }
}

impl Drop for Tts {
    fn drop(&mut self) {
        self.cancel();
        let (lock, cv) = &*self.queue;
        lock.lock().unwrap().closed = true;
        cv.notify_all();
    }
}

fn run_job(
    cmd: &[String],
    child: &Mutex<Option<Child>>,
    io: &mut Option<(ChildStdin, BufReader<ChildStdout>)>,
    id: &mut u64,
    job: &Job,
    send: &dyn Fn(Event),
) -> Result<f32, String> {
    if io.is_none() {
        let (prog, args) = cmd.split_first().ok_or("empty engine command")?;
        let data = crate::book::data_dir();
        let mut command = Command::new(prog);
        command
            .args(args)
            .env("INKVOICE_DATA", &data)
            .env("HF_HOME", data.join("hf"))
            .env("PYTHONIOENCODING", "utf-8");
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        }
        let mut c = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|e| format!("cannot start the voice engine ({e}). Install it in Settings → Voice engines."))?;
        *io = Some((c.stdin.take().unwrap(), BufReader::new(c.stdout.take().unwrap())));
        *child.lock().unwrap() = Some(c);
    }
    let (stdin, out) = io.as_mut().unwrap();
    *id += 1;
    let mut req = serde_json::json!({"id": *id, "text": job.text, "out": job.out});
    if let Some(sc) = job.stream {
        req["stream"] = true.into();
        req["start_char"] = sc.into();
    }
    writeln!(stdin, "{req}").and_then(|_| stdin.flush()).map_err(|e| e.to_string())?;
    loop {
        let mut line = String::new();
        if out.read_line(&mut line).map_err(|e| e.to_string())? == 0 {
            return Err("engine stopped (see terminal output)".into());
        }
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) else { continue };
        if v.get("id").and_then(|i| i.as_u64()) != Some(*id) {
            continue;
        }
        if let Some(pcm) = v.get("stream").and_then(|p| p.as_str()) {
            let sr = v.get("sr").and_then(|s| s.as_u64()).unwrap_or(24000) as u32;
            send(Event::StreamStart { out: job.out.clone(), pcm: pcm.into(), sr });
            continue;
        }
        if let Some(c) = v.get("chunk") {
            let samples = c.get("samples").and_then(|s| s.as_u64()).unwrap_or(0) as usize;
            let words = c.get("words").and_then(|w| serde_json::from_value(w.clone()).ok()).unwrap_or_default();
            send(Event::Chunk { out: job.out.clone(), samples, words });
        }
        if let Some(p) = v.get("progress").and_then(|p| p.as_f64()) {
            send(Event::Progress(job.out.clone(), p as f32));
            continue;
        }
        return match v.get("error").and_then(|e| e.as_str()) {
            Some(e) => Err(e.to_string()),
            None => Ok(v.get("stream_offset").and_then(|s| s.as_f64()).unwrap_or(0.0) as f32),
        };
    }
}
