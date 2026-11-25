//! The reference library: codes, specifications and measurement standards the user imports (a PDF read page by page, or a web page
//! fetched from a link) and then cites as the source of a value while working.
//!
//! The library lives in a folder of its own (not in a project): `index.json` lists the entries and `<id>.json` holds one entry's
//! page texts. An entry carries only what the user typed or the file/link itself gave: an edition or an issuer the user did not state
//! stays empty and is shown as "not stated", never guessed. Nothing here fills a number into a calculation: a search finds the page,
//! and the user reads it and enters the value with the entry's citation as its source.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

/// Pages longer than this are cut at a paragraph (links have no pages of their own, so the text is split to keep hits specific).
const LINK_PAGE_CHARS: usize = 3000;
/// A single imported text longer than this is refused (the full code volumes are far below it).
pub const MAX_TEXT_CHARS: usize = 12_000_000;

#[derive(Debug, thiserror::Error)]
pub enum LibraryError {
    #[error("library: {0}")]
    Invalid(String),
    #[error("library storage: {0}")]
    Io(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    /// A building or safety code (e.g. a code volume).
    Code,
    /// A technical specification.
    Spec,
    /// A measurement or unit standard, a table of measures.
    Metric,
    Other,
}

impl Kind {
    pub fn parse(s: &str) -> Result<Kind, LibraryError> {
        match s {
            "code" => Ok(Kind::Code),
            "spec" => Ok(Kind::Spec),
            "metric" => Ok(Kind::Metric),
            "other" => Ok(Kind::Other),
            _ => Err(LibraryError::Invalid(format!("unknown kind '{s}' (code, spec, metric or other)"))),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Origin {
    File { name: String },
    Link { url: String },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    pub id: String,
    pub kind: Kind,
    pub title: String,
    pub issuer: String,
    pub edition: String,
    pub origin: Origin,
    pub pages: usize,
    pub chars: usize,
}

impl Entry {
    /// The text a value's source field carries: `title, edition` (and the page when one is given). A missing edition is left out, not guessed.
    pub fn citation(&self, page: Option<usize>) -> String {
        let mut s = self.title.clone();
        if !self.edition.is_empty() {
            s.push_str(&format!(" ({})", self.edition));
        }
        if let Some(p) = page {
            s.push_str(&format!(", p. {p}"));
        }
        s
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Doc {
    pages: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Hit {
    pub id: String,
    pub citation: String,
    pub kind: Kind,
    pub page: usize,
    /// How many of the query's words the page contains, and whether it has all of them.
    pub found: usize,
    pub all: bool,
    /// The query's words appear together, in order, on the page.
    pub phrase: bool,
    pub excerpt: String,
}

/// What an import asks for.
pub struct NewEntry {
    pub kind: Kind,
    pub title: String,
    pub issuer: String,
    pub edition: String,
    pub origin: Origin,
    pub pages: Vec<String>,
}

pub struct Library {
    dir: PathBuf,
}

/// Folds text for matching: lower case, no Arabic diacritics or tatweel, alef/ya/ta-marbuta variants merged.
pub fn fold(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if matches!(c, '\u{064B}'..='\u{0670}' | '\u{0640}') {
            continue;
        }
        match c {
            'إ' | 'أ' | 'آ' => out.push('ا'),
            'ى' => out.push('ي'),
            'ة' => out.push('ه'),
            _ => out.extend(c.to_lowercase()),
        }
    }
    out
}

fn terms(query: &str) -> Vec<String> {
    let mut v: Vec<String> = fold(query).split(|c: char| !c.is_alphanumeric()).filter(|w| w.chars().count() >= 2).map(str::to_string).collect();
    v.sort();
    v.dedup();
    v
}

fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.chars().all(|c| c.is_ascii_hexdigit())
}

impl Library {
    pub fn open(dir: impl Into<PathBuf>) -> Result<Library, LibraryError> {
        let dir = dir.into();
        std::fs::create_dir_all(&dir).map_err(|e| LibraryError::Io(format!("{}: {e}", dir.display())))?;
        Ok(Library { dir })
    }

    fn index_path(&self) -> PathBuf {
        self.dir.join("index.json")
    }

    fn doc_path(&self, id: &str) -> Result<PathBuf, LibraryError> {
        if !valid_id(id) {
            return Err(LibraryError::Invalid(format!("bad entry id '{id}'")));
        }
        Ok(self.dir.join(format!("{id}.json")))
    }

    fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), LibraryError> {
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, bytes).map_err(|e| LibraryError::Io(format!("{}: {e}", tmp.display())))?;
        std::fs::rename(&tmp, path).map_err(|e| LibraryError::Io(format!("{}: {e}", path.display())))
    }

    pub fn list(&self) -> Result<Vec<Entry>, LibraryError> {
        match std::fs::read(self.index_path()) {
            Ok(b) => serde_json::from_slice(&b).map_err(|e| LibraryError::Io(format!("index.json is not valid: {e}"))),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
            Err(e) => Err(LibraryError::Io(format!("index.json: {e}"))),
        }
    }

    fn save_index(&self, entries: &[Entry]) -> Result<(), LibraryError> {
        let b = serde_json::to_vec_pretty(entries).map_err(|e| LibraryError::Io(e.to_string()))?;
        Self::write_atomic(&self.index_path(), &b)
    }

    /// Adds an entry. The same text under the same title is refused as a duplicate (the existing entry is named).
    pub fn add(&self, n: NewEntry) -> Result<Entry, LibraryError> {
        let title = n.title.trim().to_string();
        if title.is_empty() {
            return Err(LibraryError::Invalid("give the entry a title (it becomes the source text)".into()));
        }
        let pages: Vec<String> = n.pages.into_iter().map(|p| p.split_whitespace().collect::<Vec<_>>().join(" ")).collect();
        let chars: usize = pages.iter().map(|p| p.chars().count()).sum();
        if chars == 0 {
            return Err(LibraryError::Invalid("no text was found to keep (a scanned PDF has none: read it with OCR first)".into()));
        }
        if chars > MAX_TEXT_CHARS {
            return Err(LibraryError::Invalid(format!("the text has {chars} characters; the limit is {MAX_TEXT_CHARS}")));
        }
        let mut h = Sha256::new();
        h.update(title.as_bytes());
        for p in &pages {
            h.update(p.as_bytes());
            h.update([0]);
        }
        let id: String = h.finalize().iter().take(8).map(|b| format!("{b:02x}")).collect();
        let mut entries = self.list()?;
        if let Some(e) = entries.iter().find(|e| e.id == id) {
            return Err(LibraryError::Invalid(format!("'{}' is already in the library", e.title)));
        }
        let entry = Entry { id: id.clone(), kind: n.kind, title, issuer: n.issuer.trim().into(), edition: n.edition.trim().into(), origin: n.origin, pages: pages.len(), chars };
        let doc = serde_json::to_vec(&Doc { pages }).map_err(|e| LibraryError::Io(e.to_string()))?;
        Self::write_atomic(&self.doc_path(&id)?, &doc)?;
        entries.push(entry.clone());
        self.save_index(&entries)?;
        Ok(entry)
    }

    pub fn remove(&self, id: &str) -> Result<(), LibraryError> {
        let path = self.doc_path(id)?;
        let mut entries = self.list()?;
        let before = entries.len();
        entries.retain(|e| e.id != id);
        if entries.len() == before {
            return Err(LibraryError::Invalid(format!("no entry '{id}'")));
        }
        self.save_index(&entries)?;
        match std::fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(LibraryError::Io(e.to_string())),
        }
    }

    fn doc(&self, id: &str) -> Result<Doc, LibraryError> {
        let b = std::fs::read(self.doc_path(id)?).map_err(|e| LibraryError::Io(format!("entry {id}: {e}")))?;
        serde_json::from_slice(&b).map_err(|e| LibraryError::Io(format!("entry {id} is not valid: {e}")))
    }

    /// The text of one page (1-based) with the entry's citation for it.
    pub fn page(&self, id: &str, page: usize) -> Result<(Entry, String), LibraryError> {
        let entry = self.list()?.into_iter().find(|e| e.id == id).ok_or_else(|| LibraryError::Invalid(format!("no entry '{id}'")))?;
        let doc = self.doc(id)?;
        let text = page.checked_sub(1).and_then(|i| doc.pages.get(i)).ok_or_else(|| LibraryError::Invalid(format!("'{}' has {} page(s); page {page} does not exist", entry.title, entry.pages)))?;
        Ok((entry, text.clone()))
    }

    /// Pages that hold at least half of the query's words, best first (the whole phrase, then all words, then how densely the words recur on the page).
    pub fn search(&self, query: &str, kind: Option<Kind>, limit: usize) -> Result<Vec<Hit>, LibraryError> {
        let words = terms(query);
        if words.is_empty() {
            return Ok(Vec::new());
        }
        let phrase_text = fold(query).split_whitespace().collect::<Vec<_>>().join(" ");
        let mut hits = Vec::new();
        for e in self.list()?.into_iter().filter(|e| kind.is_none_or(|k| e.kind == k)) {
            let doc = self.doc(&e.id)?;
            for (i, text) in doc.pages.iter().enumerate() {
                let folded = fold(text);
                let at: Vec<Option<usize>> = words.iter().map(|w| folded.find(w.as_str())).collect();
                let found = at.iter().flatten().count();
                if found == 0 || found * 2 < words.len() {
                    continue;
                }
                let phrase_at = if words.len() > 1 { folded.find(&phrase_text) } else { None };
                let first = phrase_at.unwrap_or_else(|| at.iter().flatten().copied().min().unwrap_or(0));
                // `folded` may differ in length from `text` (diacritics dropped): cut the excerpt from the folded text's own characters.
                let start = folded[..first].chars().count().saturating_sub(100);
                let excerpt: String = text.chars().skip(start).take(300).collect();
                let occurrences: usize = words.iter().map(|w| folded.matches(w.as_str()).count()).sum();
                let density = occurrences * 100_000 / folded.len().max(1);
                hits.push((density, Hit { id: e.id.clone(), citation: e.citation(Some(i + 1)), kind: e.kind, page: i + 1, found, all: found == words.len(), phrase: phrase_at.is_some(), excerpt }));
            }
        }
        hits.sort_by(|(da, a), (db, b)| b.phrase.cmp(&a.phrase).then(b.all.cmp(&a.all)).then(db.cmp(da)).then(b.found.cmp(&a.found)).then(a.id.cmp(&b.id)).then(a.page.cmp(&b.page)));
        let mut hits: Vec<Hit> = hits.into_iter().map(|(_, h)| h).collect();
        hits.truncate(limit);
        Ok(hits)
    }
}

/// Text of an HTML page split into pages of about 3000 characters, with its `<title>` if it has one. Scripts, styles and tags are
/// dropped; block tags become paragraph breaks.
pub fn html_to_pages(html: &str) -> (Option<String>, Vec<String>) {
    let lower = html.to_ascii_lowercase();
    let title = lower.find("<title").and_then(|s| {
        let from = lower[s..].find('>')? + s + 1;
        let to = lower[from..].find("</title>")? + from;
        let t = decode_entities(html.get(from..to)?).split_whitespace().collect::<Vec<_>>().join(" ");
        (!t.is_empty()).then_some(t)
    });
    let mut text = String::with_capacity(html.len() / 2);
    let (mut i, bytes) = (0, html.as_bytes());
    while i < bytes.len() {
        if bytes[i] == b'<' {
            let rest = &lower[i..];
            let skip_to = ["script", "style", "noscript", "svg", "head"].iter().find_map(|t| {
                (rest.starts_with(&format!("<{t}")) && rest[t.len() + 1..].starts_with(|c: char| c == '>' || c.is_whitespace() || c == '/')).then(|| rest.find(&format!("</{t}")).map(|p| i + p))
            });
            if let Some(Some(end)) = skip_to {
                i = lower[end..].find('>').map_or(bytes.len(), |p| end + p + 1);
                continue;
            }
            let end = lower[i..].find('>').map_or(bytes.len(), |p| i + p + 1);
            let tag = lower[i + 1..end.saturating_sub(1).max(i + 1)].trim_start_matches('/').split(|c: char| !c.is_alphanumeric()).next().unwrap_or("");
            if matches!(tag, "p" | "div" | "br" | "li" | "tr" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "section" | "article" | "table" | "ul" | "ol") {
                text.push('\n');
            } else {
                text.push(' ');
            }
            i = end;
        } else {
            let next = lower[i..].find('<').map_or(bytes.len(), |p| i + p);
            text.push_str(&html[i..next]);
            i = next;
        }
    }
    let text = decode_entities(&text);
    let mut pages = Vec::new();
    let mut cur = String::new();
    for para in text.lines().map(|l| l.split_whitespace().collect::<Vec<_>>().join(" ")).filter(|l| !l.is_empty()) {
        if !cur.is_empty() && cur.chars().count() + para.chars().count() > LINK_PAGE_CHARS {
            pages.push(std::mem::take(&mut cur));
        }
        if !cur.is_empty() {
            cur.push(' ');
        }
        cur.push_str(&para);
    }
    if !cur.is_empty() {
        pages.push(cur);
    }
    (title, pages)
}

fn decode_entities(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(p) = rest.find('&') {
        out.push_str(&rest[..p]);
        rest = &rest[p..];
        let decoded = rest.find(';').filter(|&e| e <= 10).and_then(|e| {
            let name = &rest[1..e];
            let ch = match name {
                "amp" => Some('&'),
                "lt" => Some('<'),
                "gt" => Some('>'),
                "quot" => Some('"'),
                "apos" => Some('\''),
                "nbsp" => Some(' '),
                _ => name.strip_prefix('#').and_then(|n| match n.strip_prefix(['x', 'X']) {
                    Some(h) => u32::from_str_radix(h, 16).ok(),
                    None => n.parse().ok(),
                }).and_then(char::from_u32),
            };
            ch.map(|c| (c, e + 1))
        });
        match decoded {
            Some((c, n)) => {
                out.push(c);
                rest = &rest[n..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("nnx-lib-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    fn new(title: &str, pages: &[&str]) -> NewEntry {
        NewEntry { kind: Kind::Code, title: title.into(), issuer: String::new(), edition: "2018".into(), origin: Origin::File { name: "x.pdf".into() }, pages: pages.iter().map(|s| s.to_string()).collect() }
    }

    #[test]
    fn an_imported_entry_is_found_by_its_words_and_cited_with_its_page_and_edition() {
        let lib = Library::open(tmp("find")).unwrap();
        let e = lib.add(new("SBC 201", &["general provisions", "الحد الأدنى لمساحة غرفة النوم ٩ م٢", "stairs width 1000 mm"])).unwrap();
        let hits = lib.search("الحد الادنى لمساحه غرفه النوم", None, 5).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!((hits[0].page, hits[0].all), (2, true));
        assert_eq!(hits[0].citation, "SBC 201 (2018), p. 2");
        assert!(hits[0].excerpt.contains("غرفة النوم"));
        assert_eq!(e.pages, 3);
        let r = Library::open(tmp("rank")).unwrap();
        r.add(new("R", &["door width and also corridor mention", "x", "the corridor width must be 1200 mm"])).unwrap();
        let h = r.search("corridor width", None, 5).unwrap();
        assert_eq!((h[0].page, h[0].phrase, h[1].page, h[1].phrase), (3, true, 1, false), "the page with the whole phrase comes first");
        let none = lib.search("elevator", None, 5).unwrap();
        assert!(none.is_empty());
        assert_eq!(lib.search("stairs width", Some(Kind::Metric), 5).unwrap().len(), 0);
        assert_eq!(lib.page(&e.id, 3).unwrap().1, "stairs width 1000 mm");
        assert!(lib.page(&e.id, 4).is_err() && lib.page(&e.id, 0).is_err());
    }

    #[test]
    fn a_missing_edition_is_left_out_and_the_library_survives_reopening_and_removal() {
        let dir = tmp("keep");
        let lib = Library::open(&dir).unwrap();
        let mut n = new("Spec A", &["concrete cover 40 mm"]);
        n.edition = String::new();
        let e = lib.add(n).unwrap();
        assert_eq!(e.citation(None), "Spec A");
        assert_eq!(e.citation(Some(1)), "Spec A, p. 1");
        assert!(lib.add(new("Spec A", &["concrete cover 40 mm"])).is_err(), "same title and text is a duplicate");
        let again = Library::open(&dir).unwrap();
        assert_eq!(again.list().unwrap().len(), 1);
        again.remove(&e.id).unwrap();
        assert!(again.list().unwrap().is_empty() && again.remove(&e.id).is_err());
        assert!(again.remove("../x").is_err() && again.page("zz", 1).is_err());
    }

    #[test]
    fn empty_text_and_empty_titles_are_refused() {
        let lib = Library::open(tmp("bad")).unwrap();
        assert!(lib.add(new("T", &["   ", ""])).unwrap_err().to_string().contains("OCR"));
        assert!(lib.add(new("  ", &["text"])).is_err());
        assert!(Kind::parse("code").is_ok() && Kind::parse("law").is_err());
    }

    #[test]
    fn html_gives_a_title_and_text_without_scripts_or_tags() {
        let (t, pages) = html_to_pages("<html><head><title>Cover &amp; spacing</title><style>p{}</style></head><body><script>var x=1</script><h1>Clause 4</h1><p>Minimum cover is 40&nbsp;mm &#8211; see table.</p></body></html>");
        assert_eq!(t.as_deref(), Some("Cover & spacing"));
        assert_eq!(pages.len(), 1);
        assert_eq!(pages[0], "Clause 4 Minimum cover is 40 mm – see table.");
        let long = format!("<body>{}</body>", "<p>".to_string() + &"word ".repeat(400) + "</p>").repeat(3);
        assert!(html_to_pages(&long).1.len() >= 2);
    }
}
