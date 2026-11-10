use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

const PAGE_CHARS: usize = 1800;
const PDF_PAGES_PER_CHAPTER: usize = 10;

#[derive(Serialize, Deserialize, Clone)]
pub struct Chapter {
    pub title: String,
    pub pages: Vec<String>,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct Book {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub author: String,
    /// original file, for re-import
    #[serde(default)]
    pub source: String,
    pub chapters: Vec<Chapter>,
    /// last read (chapter, page); lives in progress.json, not book.json
    #[serde(skip, default)]
    pub pos: (usize, usize),
    /// unix seconds the book was last opened (progress.json)
    #[serde(skip, default)]
    pub opened: u64,
}

#[derive(Serialize, Deserialize, Default)]
struct Progress {
    ch: usize,
    pg: usize,
    opened: u64,
}

pub fn now_secs() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// Where books, audio, voice engines and their models live
/// (~/.local/share/inkvoice on Linux, %LOCALAPPDATA%\\inkvoice on Windows).
pub fn data_dir() -> PathBuf {
    dirs::data_local_dir().unwrap_or_else(|| PathBuf::from(".")).join("inkvoice")
}

pub fn config_dir() -> PathBuf {
    dirs::config_dir().unwrap_or_else(|| PathBuf::from(".")).join("inkvoice")
}

/// Early versions were called BookReader: move their data and settings over once.
pub fn migrate_legacy() {
    let moves = [
        (dirs::data_dir().map(|d| d.join("bookreader")), data_dir()),
        (dirs::config_dir().map(|d| d.join("bookreader")), config_dir()),
    ];
    for (old, new) in moves {
        if let Some(old) = old {
            if old.is_dir() && !new.exists() {
                let _ = std::fs::create_dir_all(new.parent().unwrap_or(&new));
                let _ = std::fs::rename(&old, &new);
            }
        }
    }
}
pub fn books_dir() -> PathBuf {
    data_dir().join("books")
}
impl Book {
    pub fn dir(&self) -> PathBuf {
        books_dir().join(&self.id)
    }
    pub fn save(&self) -> Result<()> {
        std::fs::create_dir_all(self.dir())?;
        std::fs::write(self.dir().join("book.json"), serde_json::to_vec(self)?)?;
        Ok(())
    }
    pub fn cover_path(&self) -> PathBuf {
        self.dir().join("cover.png")
    }
    /// Load position + last-opened from progress.json (also accepts the old `[ch, pg]` format).
    fn load_progress(&mut self) {
        let bytes = std::fs::read(self.dir().join("progress.json")).unwrap_or_default();
        let p = serde_json::from_slice::<Progress>(&bytes)
            .or_else(|_| serde_json::from_slice::<(usize, usize)>(&bytes).map(|(ch, pg)| Progress { ch, pg, opened: 0 }))
            .unwrap_or_default();
        let valid = p.ch < self.chapters.len() && p.pg < self.chapters[p.ch].pages.len();
        self.pos = if valid { (p.ch, p.pg) } else { (0, 0) };
        self.opened = p.opened;
    }
    pub fn save_progress(&self) {
        let p = Progress { ch: self.pos.0, pg: self.pos.1, opened: self.opened };
        let _ = std::fs::write(self.dir().join("progress.json"), serde_json::to_vec(&p).unwrap());
    }
    /// Fraction of pages before the current position.
    pub fn read_fraction(&self) -> f32 {
        let before: usize = self.chapters[..self.pos.0].iter().map(|c| c.pages.len()).sum::<usize>() + self.pos.1;
        before as f32 / self.page_count().max(1) as f32
    }
    /// Char offset (Unicode scalar count) where each page starts inside `chapter_text(ci)`.
    pub fn page_char_starts(&self, ci: usize) -> Vec<usize> {
        let mut out = Vec::new();
        let mut at = 0;
        for p in &self.chapters[ci].pages {
            out.push(at);
            at += p.chars().count() + 2; // "\n\n" separator
        }
        out
    }
    pub fn page_count(&self) -> usize {
        self.chapters.iter().map(|c| c.pages.len()).sum()
    }
    pub fn chapter_text(&self, ci: usize) -> String {
        self.chapters[ci].pages.join("\n\n")
    }
}

pub fn load_library() -> Vec<Book> {
    let mut v: Vec<Book> = std::fs::read_dir(books_dir())
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| std::fs::read(e.path().join("book.json")).ok())
        .filter_map(|b| serde_json::from_slice::<Book>(&b).ok())
        .map(|mut b| {
            b.load_progress();
            b
        })
        .collect();
    sort_library(&mut v);
    v
}

/// Most recently opened first, then by title.
pub fn sort_library(v: &mut [Book]) {
    v.sort_by(|a, b| b.opened.cmp(&a.opened).then_with(|| a.title.to_lowercase().cmp(&b.title.to_lowercase())));
}

pub fn import(path: &Path) -> Result<Book> {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase();
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("book").to_string();
    let (mut book, cover) = match ext.as_str() {
        "epub" => import_epub(path, &stem)?,
        "pdf" => (import_pdf(path, &stem)?, None),
        _ => bail!("Unsupported file type (use .epub or .pdf)"),
    };
    book.source = path.to_string_lossy().into_owned();
    book.id = stem
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '_' })
        .collect::<String>()
        .to_lowercase();
    if book.chapters.is_empty() {
        bail!("No readable text found (scanned PDF?)");
    }
    book.save()?;
    if let Some(bytes) = cover {
        // store a small thumbnail; a missing/odd cover is not an import error
        if let Ok(img) = image::load_from_memory(&bytes) {
            let _ = img.thumbnail(320, 480).save(book.cover_path());
        }
    }
    book.load_progress();
    Ok(book)
}

fn import_epub(path: &Path, stem: &str) -> Result<(Book, Option<Vec<u8>>)> {
    let mut doc = epub::doc::EpubDoc::new(path)?;
    let title = doc.mdata("title").map(|m| m.value.trim().to_string()).unwrap_or_else(|| stem.to_string());
    let author = doc.mdata("creator").map(|m| m.value.trim().to_string()).unwrap_or_default();
    let cover = doc.get_cover().map(|(bytes, _mime)| bytes);
    // Prefer the book's own table of contents (real chapter titles, and chapters that share a
    // file are split at their anchors); fall back to one chapter per file.
    let mut chapters = chapters_from_toc(&mut doc);
    if chapters.len() < 2 {
        chapters = chapters_from_spine(&mut doc);
    }
    Ok((Book { id: String::new(), title, author, source: String::new(), chapters, pos: (0, 0), opened: 0 }, cover))
}

/// One chapter per spine file, titled from its first line.
fn chapters_from_spine(doc: &mut epub::doc::EpubDoc<std::io::BufReader<std::fs::File>>) -> Vec<Chapter> {
    let mut chapters = Vec::new();
    for i in 0..doc.spine.len() {
        doc.set_current_chapter(i);
        let Some((html, _)) = doc.get_current_str() else { continue };
        let text = html_to_text(&html);
        if text.chars().count() < 80 {
            continue;
        }
        let title = title_from_text(&text).unwrap_or_else(|| format!("Section {}", chapters.len() + 1));
        chapters.push(Chapter { title, pages: paginate(&text) });
    }
    chapters
}

fn title_from_text(text: &str) -> Option<String> {
    // first real line (skip picture placeholders such as "image")
    let first = text
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty() && !l.eq_ignore_ascii_case("image") && !l.eq_ignore_ascii_case("[image]"))?;
    (first.chars().count() <= 70).then(|| first.to_string())
}

fn chapters_from_toc(doc: &mut epub::doc::EpubDoc<std::io::BufReader<std::fs::File>>) -> Vec<Chapter> {
    // flatten the TOC: (label, file, fragment)
    fn flat(n: &[epub::doc::NavPoint], out: &mut Vec<(String, PathBuf, Option<String>)>) {
        for p in n {
            let s = p.content.to_string_lossy().into_owned();
            let (file, frag) = match s.split_once('#') {
                Some((f, fr)) => (PathBuf::from(f), Some(fr.to_string())),
                None => (PathBuf::from(&s), None),
            };
            let label = p.label.split_whitespace().collect::<Vec<_>>().join(" ");
            out.push((label, file, frag));
            flat(&p.children, out);
        }
    }
    let mut toc = Vec::new();
    let nav = doc.toc.clone();
    flat(&nav, &mut toc);
    if toc.len() < 2 {
        return Vec::new();
    }

    // (title, text) in reading order; text before a file's first anchor continues the previous chapter
    let mut parts: Vec<(String, String)> = Vec::new();
    let spine: Vec<String> = doc.spine.iter().map(|s| s.idref.clone()).collect();
    for idref in spine {
        let Some(file) = doc.resources.get(&idref).map(|r| r.path.clone()) else { continue };
        let Some(html) = doc.get_resource_str_by_path(&file) else { continue };
        let mut cuts: Vec<(usize, String)> = toc
            .iter()
            .filter(|(_, f, _)| *f == file)
            .filter_map(|(label, _, frag)| match frag {
                None => Some((0, label.clone())),
                Some(fr) => anchor_pos(&html, fr).map(|p| (p, label.clone())),
            })
            .collect();
        cuts.sort_by_key(|c| c.0);
        cuts.dedup_by_key(|c| c.0);
        let mut bounds: Vec<usize> = cuts.iter().map(|c| c.0).collect();
        bounds.push(html.len());
        let head_end = bounds.first().copied().unwrap_or(html.len());
        // text before the first chapter start in this file
        if head_end > 0 {
            let text = html_to_text(&html[..head_end]);
            if !text.trim().is_empty() {
                match parts.last_mut() {
                    Some(last) => {
                        last.1.push_str("\n\n");
                        last.1.push_str(&text);
                    }
                    None => parts.push((title_from_text(&text).unwrap_or_else(|| "Beginning".into()), text)),
                }
            }
        }
        for (k, (start, label)) in cuts.iter().enumerate() {
            let text = html_to_text(&html[*start..bounds[k + 1]]);
            parts.push((label.clone(), text));
        }
    }
    parts
        .into_iter()
        .filter(|(_, t)| t.trim().chars().count() >= 20)
        .map(|(title, text)| Chapter {
            title: if title.is_empty() { "Untitled".into() } else { title.chars().take(80).collect() },
            pages: paginate(text.trim()),
        })
        .collect()
}

/// Byte offset of the tag carrying `id="frag"` (start of the chapter inside a shared file).
fn anchor_pos(html: &str, frag: &str) -> Option<usize> {
    for q in ['"', '\''] {
        for attr in ["id=", "name="] {
            let needle = format!("{attr}{q}{frag}{q}");
            if let Some(i) = html.find(&needle) {
                return html[..i].rfind('<');
            }
        }
    }
    None
}

fn html_to_text(html: &str) -> String {
    // plain, undecorated text: no markdown `*emphasis*`, `[link][1]` footnotes or `#` headers,
    // which would otherwise show up in the reader and be read aloud by the voice
    let raw = html2text::config::plain_no_decorate().string_from_read(html.as_bytes(), 100_000).unwrap_or_default();
    raw.lines()
        .map(clean_line)
        .collect::<Vec<_>>()
        .join("\n")
        .replace("\n\n\n", "\n\n")
        .trim()
        .to_string()
}

/// Drop leftover markup from the converter: `# ` heading marks and `[link]` brackets.
fn clean_line(l: &str) -> String {
    let l = l.trim_end();
    let l = if l.starts_with('#') { l.trim_start_matches('#').trim_start() } else { l };
    l.chars().filter(|c| *c != '[' && *c != ']').collect()
}

fn import_pdf(path: &Path, stem: &str) -> Result<Book> {
    let pages = pdf_extract::extract_text_by_pages(path)?;
    let pages: Vec<String> = pages
        .into_iter()
        .map(|p| p.split_whitespace().collect::<Vec<_>>().join(" "))
        .collect();
    let chapters = pages
        .chunks(PDF_PAGES_PER_CHAPTER)
        .enumerate()
        .map(|(i, c)| {
            let start = i * PDF_PAGES_PER_CHAPTER + 1;
            Chapter {
                title: format!("Pages {}-{}", start, start + c.len() - 1),
                pages: c.to_vec(),
            }
        })
        .collect();
    Ok(Book { id: String::new(), title: stem.to_string(), author: String::new(), source: String::new(), chapters, pos: (0, 0), opened: 0 })
}

/// Split text into ~PAGE_CHARS pages on paragraph (then sentence/word) boundaries.
fn paginate(text: &str) -> Vec<String> {
    let mut pages = Vec::new();
    let mut cur = String::new();
    for para in text.split("\n\n").map(str::trim).filter(|p| !p.is_empty()) {
        for piece in split_long(para) {
            if !cur.is_empty() && cur.chars().count() + piece.chars().count() > PAGE_CHARS {
                pages.push(std::mem::take(&mut cur));
            }
            if !cur.is_empty() {
                cur.push_str("\n\n");
            }
            cur.push_str(&piece);
        }
    }
    if !cur.is_empty() {
        pages.push(cur);
    }
    pages
}

fn split_long(p: &str) -> Vec<String> {
    if p.chars().count() <= PAGE_CHARS {
        return vec![p.to_string()];
    }
    let mut out = Vec::new();
    let mut cur = String::new();
    for w in p.split_whitespace() {
        if cur.chars().count() + w.len() + 1 > PAGE_CHARS {
            out.push(std::mem::take(&mut cur));
        }
        if !cur.is_empty() {
            cur.push(' ');
        }
        cur.push_str(w);
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

#[cfg(test)]
mod tests {
    #[test]
    fn imports_epub() {
        let Ok(p) = std::env::var("TEST_EPUB") else { return };
        let (b, _) = super::import_epub(std::path::Path::new(&p), "x").unwrap();
        assert_eq!(b.title, "Test Book");
        assert_eq!(b.chapters.len(), 2);
        assert_eq!(b.chapters[0].title, "Chapter One");
        assert!(b.chapters[0].pages.len() > 1);
        assert!(b.chapters[0].pages.iter().all(|p| p.chars().count() <= 2000));
        println!("{}", b.chapters[0].pages[0]);
    }
}
