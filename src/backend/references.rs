//! The project's `.bib` files as a library, duplicates and unused entries
//! flagged, plus importing a DOI/ISBN/arXiv ID (or a hand-typed reference)
//! as a new BibTeX entry.

use super::index::{self, walk_files, CiteEntry};
use super::project::{ext_lower, Res};
use super::texparse;
use regex::Regex;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::time::Duration;
use crate::i18n::t;
use crate::tf;

#[derive(Clone, Debug, Default)]
pub struct Entry {
    pub key: String,
    pub kind: String,
    pub title: String,
    pub author: String,
    pub year: String,
    pub doi: String,
    pub file: PathBuf,
    pub used: bool,
    pub duplicate: bool,
    /// Extra BibTeX fields a fetch found (journal, publisher, eprint…).
    fields: Vec<(String, String)>,
}

static CITE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\\(?:no|paren|text|foot|super|auto)?cite[a-zA-Z]*\*?\s*(?:\[[^\]]*\]\s*)*\{([^}]*)\}").unwrap()
});

pub fn build(root: &Path) -> Res<Vec<Entry>> {
    if root.as_os_str().is_empty() {
        return Err(t("no project open").into());
    }
    let mut entries: Vec<CiteEntry> = Vec::new();
    let mut used: HashSet<String> = HashSet::new();
    walk_files(root, &mut |path, name| match ext_lower(name).as_str() {
        "bib" => entries.extend(index::parse_bib_file(path)),
        "tex" | "sty" | "cls" => cited_keys_in_file(path, &mut used),
        _ => {}
    });
    let duplicates = duplicate_ids(&entries);
    let result: Vec<Entry> = entries
        .iter()
        .map(|e| Entry {
            key: e.key.clone(),
            kind: e.kind.clone(),
            title: e.title.clone(),
            author: e.author.clone(),
            year: e.year.clone(),
            doi: e.doi.clone(),
            file: e.file.clone(),
            used: used.contains(&e.key),
            duplicate: duplicates.contains(&entry_id(e)),
            fields: vec![],
        })
        .collect();
    // In file order: the sidebar sorts them as the user chooses.
    Ok(result)
}

/// "Knuth", "Knuth and Lamport" or "Knuth et al.": the surnames of a BibTeX
/// author list, for a short byline.
pub fn short_authors(author: &str) -> String {
    let surname = |name: &str| {
        let name = name.trim();
        match name.split_once(',') {
            Some((last, _)) => last.trim().to_string(),
            None => name.rsplit(' ').next().unwrap_or(name).to_string(),
        }
    };
    let names: Vec<&str> = author.split(" and ").filter(|n| !n.trim().is_empty()).collect();
    match names.as_slice() {
        [] => String::new(),
        [one] => surname(one),
        [a, b] => format!("{} {} {}", surname(a), t("and"), surname(b)),
        [a, ..] => format!("{} et al.", surname(a)),
    }
}

/// Deletes the entry `key` from `file`, with the blank line that set it apart.
pub fn remove_entry(file: &Path, key: &str) -> Res<()> {
    let text = fs::read_to_string(file).map_err(|e| e.to_string())?;
    let spans = index::bib_spans(&text);
    let Some((_, span)) = spans.iter().find(|(k, _)| k == key) else {
        return Err(tf!("{key} is no longer in the file", key));
    };
    super::project::atomic_write(file, cut(&text, span).as_bytes())
}

/// Moves the entry `key` of `file` before the entry `before`, or to the end
/// with `None`.
pub fn move_entry(file: &Path, key: &str, before: Option<&str>) -> Res<()> {
    let text = fs::read_to_string(file).map_err(|e| e.to_string())?;
    let spans = index::bib_spans(&text);
    let Some((_, span)) = spans.iter().find(|(k, _)| k == key) else {
        return Err(tf!("{key} is no longer in the file", key));
    };
    if before == Some(key) {
        return Ok(());
    }
    let block = text[span.clone()].to_string();
    let without = cut(&text, span);
    let spans = index::bib_spans(&without);
    let out = match before.and_then(|b| spans.iter().find(|(k, _)| k == b)) {
        Some((_, target)) => format!("{}{block}\n\n{}", &without[..target.start], &without[target.start..]),
        None => {
            let end = spans.last().map(|(_, s)| s.end).unwrap_or(without.trim_end().len());
            let rest = &without[end..];
            format!("{}\n\n{block}{}", &without[..end], if rest.is_empty() { "\n" } else { rest })
        }
    };
    super::project::atomic_write(file, out.as_bytes())
}

/// `text` without the entry at `span`, the line break and one blank line
/// that separated it from the next going with it.
fn cut(text: &str, span: &std::ops::Range<usize>) -> String {
    let head = &text[..span.start];
    let mut rest = &text[span.end..];
    for _ in 0..2 {
        let line_end = rest.find('\n').map(|i| i + 1).unwrap_or(rest.len());
        if rest[..line_end].trim().is_empty() {
            rest = &rest[line_end..];
        } else {
            break;
        }
    }
    if rest.is_empty() {
        format!("{}\n", head.trim_end())
    } else {
        format!("{head}{rest}")
    }
}

fn cited_keys_in_file(path: &Path, into: &mut HashSet<String>) {
    let Ok(text) = fs::read_to_string(path) else { return };
    for line in text.lines() {
        for m in CITE_RE.captures_iter(texparse::without_comment(line)) {
            for key in m[1].split(',') {
                let key = key.trim();
                if !key.is_empty() {
                    into.insert(key.to_string());
                }
            }
        }
    }
}

fn entry_id(e: &CiteEntry) -> String {
    format!("{}\0{}", e.key, e.file.display())
}

const MIN_TITLE_FOR_MATCH: usize = 8;
const TITLE_SIMILARITY_THRESHOLD: f64 = 0.85;

/// Flags entries sharing a normalized DOI, or a title close enough (token
/// Jaccard ≥ 0.85) to another entry's.
fn duplicate_ids(entries: &[CiteEntry]) -> HashSet<String> {
    let mut flagged = HashSet::new();
    let mut by_doi: HashMap<String, Vec<usize>> = HashMap::new();
    for (i, e) in entries.iter().enumerate() {
        let doi = normalize_doi(&e.doi);
        if !doi.is_empty() {
            by_doi.entry(doi).or_default().push(i);
        }
    }
    for idxs in by_doi.values().filter(|v| v.len() > 1) {
        for &i in idxs {
            flagged.insert(entry_id(&entries[i]));
        }
    }
    let titles: Vec<String> = entries.iter().map(|e| normalize_title(&e.title)).collect();
    for i in 0..entries.len() {
        if titles[i].len() < MIN_TITLE_FOR_MATCH {
            continue;
        }
        for j in i + 1..entries.len() {
            if titles[j].len() >= MIN_TITLE_FOR_MATCH && similar_titles(&titles[i], &titles[j]) {
                flagged.insert(entry_id(&entries[i]));
                flagged.insert(entry_id(&entries[j]));
            }
        }
    }
    flagged
}

fn normalize_title(title: &str) -> String {
    let lower: String =
        title.to_lowercase().chars().map(|c| if c.is_ascii_alphanumeric() || c == ' ' { c } else { ' ' }).collect();
    lower.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn normalize_doi(doi: &str) -> String {
    let d = doi.trim().to_lowercase();
    let d = d.strip_prefix("https://doi.org/").or_else(|| d.strip_prefix("http://doi.org/")).unwrap_or(&d);
    let d = d.strip_prefix("doi:").unwrap_or(d);
    d.trim().to_string()
}

fn similar_titles(a: &str, b: &str) -> bool {
    if a == b {
        return true;
    }
    let sa: HashSet<&str> = a.split_whitespace().collect();
    let sb: HashSet<&str> = b.split_whitespace().collect();
    if sa.is_empty() || sb.is_empty() {
        return false;
    }
    let inter = sa.intersection(&sb).count();
    let union = sa.len() + sb.len() - inter;
    union > 0 && inter as f64 / union as f64 >= TITLE_SIMILARITY_THRESHOLD
}

// ---------- Import ----------

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum IdKind {
    Doi,
    Arxiv,
    Isbn,
    Unknown,
}

static DOI_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)^10\.\d{4,9}/\S+$").unwrap());
static ARXIV_NEW_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)^\d{4}\.\d{4,5}(v\d+)?$").unwrap());
static ARXIV_OLD_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^[a-z-]+(\.[a-z]{2})?/\d{7}(v\d+)?$").unwrap());

/// Strips the prefixes people paste along with the identifier and says what
/// is left.
pub fn detect_identifier(raw: &str) -> (IdKind, String) {
    let mut s = raw.trim().to_string();
    let lower = s.to_lowercase();
    for prefix in ["https://doi.org/", "http://doi.org/", "doi:", "arxiv:", "https://arxiv.org/abs/", "isbn:", "isbn "]
    {
        if lower.starts_with(prefix) {
            s = s[prefix.len()..].trim().to_string();
            break;
        }
    }
    if DOI_RE.is_match(&s) {
        return (IdKind::Doi, s);
    }
    if ARXIV_NEW_RE.is_match(&s) || ARXIV_OLD_RE.is_match(&s) {
        return (IdKind::Arxiv, s);
    }
    let digits: String = s.chars().filter(|c| c.is_ascii_digit() || *c == 'X' || *c == 'x').collect();
    if digits.len() == 10 || digits.len() == 13 {
        return (IdKind::Isbn, digits);
    }
    (IdKind::Unknown, s)
}

/// Fetches metadata for a DOI, ISBN or arXiv ID and appends an entry for it.
pub fn import(root: &Path, identifier: &str) -> Res<Entry> {
    let id = identifier.trim();
    if id.is_empty() {
        return Err(t("paste a DOI, ISBN or arXiv ID").into());
    }
    let entry = match detect_identifier(id) {
        (IdKind::Doi, s) => fetch_doi(&s)?,
        (IdKind::Arxiv, s) => fetch_arxiv(&s)?,
        (IdKind::Isbn, s) => fetch_isbn(&s)?,
        _ => return Err(t("unrecognized identifier: paste a DOI, ISBN or arXiv ID").into()),
    };
    finalize_and_write(root, entry)
}

#[derive(Clone, Debug, Default)]
pub struct ManualFields {
    pub kind: String,
    pub title: String,
    pub author: String,
    pub year: String,
    pub doi: String,
    pub note: String,
}

/// A hand-typed entry, written exactly like an imported one.
pub fn add_manual(root: &Path, f: &ManualFields) -> Res<Entry> {
    let title = f.title.trim();
    if title.is_empty() {
        return Err(t("the title is required").into());
    }
    let kind = if f.kind.trim().is_empty() { "misc".to_string() } else { f.kind.trim().to_string() };
    let entry = Entry {
        kind,
        title: title.into(),
        author: f.author.trim().into(),
        year: f.year.trim().into(),
        doi: f.doi.trim().into(),
        fields: vec![("note".into(), f.note.trim().into())],
        ..Default::default()
    };
    finalize_and_write(root, entry)
}

fn finalize_and_write(root: &Path, mut entry: Entry) -> Res<Entry> {
    let target = bib_target(root);
    let taken: HashSet<String> = build(root).unwrap_or_default().into_iter().map(|e| e.key).collect();
    entry.key = unique_key(&base_key(&entry), &taken);
    entry.file = target.clone();
    append_bib_entry(&target, &entry).map_err(|e| e.to_string())?;
    Ok(entry)
}

fn http_get(url: &str) -> Result<String, String> {
    let agent: ureq::Agent = ureq::Agent::config_builder().timeout_global(Some(Duration::from_secs(20))).build().into();
    let mut resp = agent
        .get(url)
        .header("User-Agent", "tectec (desktop; contato via app)")
        .header("Accept", "application/json, application/xml, text/xml, */*")
        .call()
        .map_err(|e| match e {
            ureq::Error::StatusCode(404) => t("not found").to_string(),
            ureq::Error::StatusCode(code) => format!("status {code}"),
            other => other.to_string(),
        })?;
    resp.body_mut().read_to_string().map_err(|e| e.to_string())
}

/// Percent-encodes everything but the RFC 3986 unreserved characters.
fn escape(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

fn fetch_doi(doi: &str) -> Res<Entry> {
    let body = http_get(&format!("https://api.crossref.org/works/{}", escape(doi)))
        .map_err(|e| tf!("could not look up the DOI \"{doi}\": {e}", doi, e))?;
    parse_crossref(&body, doi)
}

/// A Crossref `/works/<doi>` response → entry.
fn parse_crossref(body: &str, doi: &str) -> Res<Entry> {
    let v: serde_json::Value =
        serde_json::from_str(body).map_err(|_| format!("resposta inesperada do Crossref para \"{doi}\""))?;
    let msg = &v["message"];
    let first = |key: &str| msg[key].as_array().and_then(|a| a.first()).and_then(|s| s.as_str()).unwrap_or("").to_string();
    let title = first("title");
    if title.is_empty() {
        return Err(tf!("DOI \"{doi}\" not found", doi));
    }
    let year_of = |key: &str| msg[key]["date-parts"][0][0].as_i64().map(|y| y.to_string()).unwrap_or_default();
    let mut year = year_of("published-print");
    if year.is_empty() {
        year = year_of("published");
    }
    let authors: Vec<String> = msg["author"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|p| name_parts(p["given"].as_str().unwrap_or(""), p["family"].as_str().unwrap_or("")))
                .collect()
        })
        .unwrap_or_default();
    let journal = first("container-title");
    let s = |key: &str| msg[key].as_str().unwrap_or("").to_string();
    let kind = match s("type").as_str() {
        "book" | "monograph" => "book",
        "proceedings-article" => "inproceedings",
        "report" => "techreport",
        "journal-article" => "article",
        _ if journal.is_empty() => "misc",
        _ => "article",
    };
    Ok(Entry {
        kind: kind.into(),
        title,
        author: authors.join(" and "),
        year,
        doi: s("DOI"),
        fields: vec![
            ("journal".into(), journal),
            ("publisher".into(), s("publisher")),
            ("volume".into(), s("volume")),
            ("number".into(), s("issue")),
            ("pages".into(), s("page")),
            ("url".into(), s("URL")),
        ],
        ..Default::default()
    })
}

fn fetch_arxiv(id: &str) -> Res<Entry> {
    let body = http_get(&format!("https://export.arxiv.org/api/query?id_list={}", escape(id)))
        .map_err(|e| tf!("could not look up the arXiv \"{id}\": {e}", id, e))?;
    parse_arxiv(&body, id)
}

/// An arXiv API Atom feed → entry.
fn parse_arxiv(body: &str, id: &str) -> Res<Entry> {
    let doc = roxmltree::Document::parse(body).map_err(|_| format!("resposta inesperada do arXiv para \"{id}\""))?;
    let entry = doc
        .descendants()
        .find(|n| n.has_tag_name("entry"))
        .ok_or_else(|| tf!("arXiv \"{id}\" not found", id))?;
    let child_text = |node: roxmltree::Node, tag: &str| {
        node.children().find(|c| c.has_tag_name(tag)).and_then(|c| c.text()).unwrap_or("").to_string()
    };
    let title = child_text(entry, "title").split_whitespace().collect::<Vec<_>>().join(" ");
    if title.is_empty() {
        return Err(tf!("arXiv \"{id}\" not found", id));
    }
    let published = child_text(entry, "published");
    let year = published.get(..4).unwrap_or("").to_string();
    let authors: Vec<String> = entry
        .children()
        .filter(|c| c.has_tag_name("author"))
        .map(|a| bibtex_name(&child_text(a, "name")))
        .collect();
    let primary = entry
        .children()
        .find(|c| c.has_tag_name("primary_category"))
        .and_then(|c| c.attribute("term"))
        .unwrap_or("")
        .to_string();
    let entry_id = child_text(entry, "id");
    let arxiv_id = entry_id
        .split("abs/")
        .nth(1)
        .map(|s| strip_arxiv_version(s).to_string())
        .unwrap_or_else(|| id.to_string());
    Ok(Entry {
        kind: "misc".into(),
        title,
        author: authors.join(" and "),
        year,
        fields: vec![
            ("eprint".into(), arxiv_id),
            ("archivePrefix".into(), "arXiv".into()),
            ("primaryClass".into(), primary),
        ],
        ..Default::default()
    })
}

/// "1706.03762v7" → "1706.03762". Only a trailing `v<digits>`: an old-style
/// archive name can have a `v` in it (`solv-int/9901001`).
fn strip_arxiv_version(id: &str) -> &str {
    match id.rsplit_once('v') {
        Some((base, n)) if !base.is_empty() && !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()) => base,
        _ => id,
    }
}

fn fetch_isbn(isbn: &str) -> Res<Entry> {
    let body = http_get(&format!(
        "https://openlibrary.org/api/books?bibkeys=ISBN:{}&format=json&jscmd=data",
        escape(isbn)
    ))
    .map_err(|e| tf!("could not look up the ISBN \"{isbn}\": {e}", isbn, e))?;
    parse_open_library(&body, isbn)
}

/// An Open Library `/api/books?jscmd=data` response → entry.
fn parse_open_library(body: &str, isbn: &str) -> Res<Entry> {
    let v: serde_json::Value =
        serde_json::from_str(body).map_err(|_| format!("resposta inesperada da Open Library para \"{isbn}\""))?;
    let book = &v[format!("ISBN:{isbn}")];
    let title = book["title"].as_str().unwrap_or("").to_string();
    if title.is_empty() {
        return Err(tf!("ISBN \"{isbn}\" not found", isbn));
    }
    let authors: Vec<String> = book["authors"]
        .as_array()
        .map(|a| a.iter().map(|p| bibtex_name(p["name"].as_str().unwrap_or(""))).collect())
        .unwrap_or_default();
    let publisher = book["publishers"][0]["name"].as_str().unwrap_or("").to_string();
    static YEAR_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\d{4}").unwrap());
    let year = YEAR_RE.find(book["publish_date"].as_str().unwrap_or("")).map(|m| m.as_str().to_string()).unwrap_or_default();
    Ok(Entry {
        kind: "book".into(),
        title,
        author: authors.join(" and "),
        year,
        fields: vec![("publisher".into(), publisher), ("isbn".into(), isbn.into())],
        ..Default::default()
    })
}

/// "First M. Last" → "Last, First M.".
fn bibtex_name(full: &str) -> String {
    let full = full.trim();
    if full.is_empty() || full.contains(',') {
        return full.to_string();
    }
    let parts: Vec<&str> = full.split_whitespace().collect();
    if parts.len() < 2 {
        return full.to_string();
    }
    format!("{}, {}", parts[parts.len() - 1], parts[..parts.len() - 1].join(" "))
}

fn name_parts(given: &str, family: &str) -> String {
    match (given.trim(), family.trim()) {
        (g, "") => g.to_string(),
        ("", f) => f.to_string(),
        (g, f) => format!("{f}, {g}"),
    }
}

fn base_key(e: &Entry) -> String {
    let first = e.author.split(" and ").next().unwrap_or("").trim();
    let last = first.split(',').next().unwrap_or(first);
    let mut key: String = last.to_lowercase().chars().filter(|c| c.is_ascii_lowercase() || c.is_ascii_digit()).collect();
    if key.is_empty() {
        key = "ref".into();
    }
    format!("{key}{}", e.year)
}

fn unique_key(base: &str, taken: &HashSet<String>) -> String {
    if !taken.contains(base) {
        return base.to_string();
    }
    for c in 'a'..='z' {
        let candidate = format!("{base}{c}");
        if !taken.contains(&candidate) {
            return candidate;
        }
    }
    (2..).map(|i| format!("{base}-{i}")).find(|c| !taken.contains(c)).unwrap()
}

static BIBLIOGRAPHY_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\\bibliography\{([^}]+)\}").unwrap());
static ADDBIBRESOURCE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\\addbibresource(?:\[[^\]]*\])?\{([^}]+)\}").unwrap());

/// Where an import lands: a `.bib` the document already names, else the first
/// `.bib` in the project, else a new `references.bib` at the root.
fn bib_target(root: &Path) -> PathBuf {
    let (mut existing, mut missing, mut found) = (Vec::new(), Vec::new(), Vec::new());
    walk_files(root, &mut |path, name| match ext_lower(name).as_str() {
        "tex" => {
            let Ok(text) = fs::read_to_string(path) else { return };
            let dir = path.parent().unwrap_or(root);
            let mut add = |name: &str| {
                let name = name.trim();
                if name.is_empty() {
                    return;
                }
                let file = if name.to_lowercase().ends_with(".bib") { name.to_string() } else { format!("{name}.bib") };
                let p = dir.join(file);
                if p.exists() {
                    existing.push(p)
                } else {
                    missing.push(p)
                }
            };
            for m in BIBLIOGRAPHY_RE.captures_iter(&text) {
                for part in m[1].split(',') {
                    add(part);
                }
            }
            for m in ADDBIBRESOURCE_RE.captures_iter(&text) {
                add(&m[1]);
            }
        }
        "bib" => found.push(path.to_path_buf()),
        _ => {}
    });
    existing.sort();
    missing.sort();
    found.sort();
    existing.into_iter().chain(missing).chain(found).next().unwrap_or_else(|| root.join("references.bib"))
}

fn append_bib_entry(path: &Path, entry: &Entry) -> std::io::Result<()> {
    let is_new = !path.exists();
    let mut f = fs::OpenOptions::new().append(true).create(true).open(path)?;
    let mut b = String::new();
    if is_new {
        b.push_str(&format!("% {}\n\n", t("References imported by tectec.")));
    } else {
        b.push('\n');
    }
    b.push_str(&format_bib_entry(entry));
    b.push('\n');
    f.write_all(b.as_bytes())
}

const FIELD_ORDER: &[&str] = &[
    "author", "title", "journal", "booktitle", "publisher", "year", "volume", "number", "pages", "doi", "isbn", "url",
    "eprint", "archivePrefix", "primaryClass", "note",
];

fn format_bib_entry(e: &Entry) -> String {
    let mut fields: HashMap<&str, &str> =
        [("author", e.author.as_str()), ("title", e.title.as_str()), ("year", e.year.as_str()), ("doi", e.doi.as_str())]
            .into_iter()
            .collect();
    for (k, v) in &e.fields {
        if !v.is_empty() {
            fields.insert(k.as_str(), v.as_str());
        }
    }
    let mut b = format!("@{}{{{},\n", e.kind, e.key);
    for name in FIELD_ORDER {
        let value = fields.get(name).map(|v| v.trim()).unwrap_or("");
        if !value.is_empty() {
            b.push_str(&format!("  {name} = {{{value}}},\n"));
        }
    }
    b.push_str("}\n");
    b
}

#[cfg(test)]
mod tests {
    use super::*;

    const BIB: &str = "% refs\n\n@book{a,\n  title = {A}\n}\n\n@book{b,\n  title = {B}\n}\n\n@book{c,\n  title = {C}\n}\n";

    fn keys(path: &Path) -> Vec<String> {
        index::bib_spans(&fs::read_to_string(path).unwrap()).into_iter().map(|(k, _)| k).collect()
    }

    #[test]
    fn short_bylines() {
        assert_eq!(short_authors("Knuth, Donald E."), "Knuth");
        assert_eq!(short_authors("Donald Knuth"), "Knuth");
        assert_eq!(short_authors("Knuth, D. and Lamport, L. and Turing, A."), "Knuth et al.");
        assert_eq!(short_authors(""), "");
    }

    #[test]
    fn removing_entries() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("refs.bib");
        fs::write(&f, BIB).unwrap();
        remove_entry(&f, "b").unwrap();
        assert_eq!(fs::read_to_string(&f).unwrap(), "% refs\n\n@book{a,\n  title = {A}\n}\n\n@book{c,\n  title = {C}\n}\n");
        remove_entry(&f, "c").unwrap();
        assert_eq!(fs::read_to_string(&f).unwrap(), "% refs\n\n@book{a,\n  title = {A}\n}\n");
        assert!(remove_entry(&f, "zzz").is_err());
    }

    #[test]
    fn moving_entries() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("refs.bib");
        fs::write(&f, BIB).unwrap();
        move_entry(&f, "c", Some("a")).unwrap();
        assert_eq!(keys(&f), ["c", "a", "b"]);
        move_entry(&f, "c", None).unwrap();
        assert_eq!(keys(&f), ["a", "b", "c"]);
        move_entry(&f, "a", Some("c")).unwrap();
        assert_eq!(keys(&f), ["b", "a", "c"]);
        // Every move keeps one blank line between entries.
        assert_eq!(fs::read_to_string(&f).unwrap(), "% refs\n\n@book{b,\n  title = {B}\n}\n\n@book{a,\n  title = {A}\n}\n\n@book{c,\n  title = {C}\n}\n");
    }

    #[test]
    fn identifiers() {
        assert_eq!(detect_identifier("https://doi.org/10.1000/xyz").0, IdKind::Doi);
        assert_eq!(detect_identifier("arXiv:2101.00001v2").0, IdKind::Arxiv);
        assert_eq!(detect_identifier("hep-th/9901001").0, IdKind::Arxiv);
        assert_eq!(detect_identifier("ISBN 978-3-16-148410-0"), (IdKind::Isbn, "9783161484100".into()));
        assert_eq!(detect_identifier("banana").0, IdKind::Unknown);
    }

    #[test]
    fn keys_and_names() {
        assert_eq!(bibtex_name("Donald E. Knuth"), "Knuth, Donald E.");
        let e = Entry { author: "Knuth, Donald".into(), year: "1984".into(), ..Default::default() };
        let taken: HashSet<String> = ["knuth1984".to_string()].into();
        assert_eq!(unique_key(&base_key(&e), &taken), "knuth1984a");
    }

    #[test]
    fn manual_entry_and_library() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::write(root.join("main.tex"), "\\cite{silva2020}\n\\bibliography{refs}").unwrap();
        let e = add_manual(root, &ManualFields { title: "Um título qualquer".into(), author: "Silva, Ana".into(), year: "2020".into(), ..Default::default() }).unwrap();
        assert_eq!(e.file, root.join("refs.bib"));
        let lib = build(root).unwrap();
        assert_eq!(lib.len(), 1);
        assert_eq!(lib[0].key, "silva2020");
        assert!(lib[0].used);
        add_manual(root, &ManualFields { title: "Um título qualquer".into(), ..Default::default() }).unwrap();
        let lib = build(root).unwrap();
        assert!(lib.iter().all(|e| e.duplicate));
    }

    fn write(path: &Path, content: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
    }

    fn by_key(root: &Path) -> HashMap<String, Entry> {
        build(root).unwrap().into_iter().map(|e| (e.key.clone(), e)).collect()
    }

    fn field<'a>(e: &'a Entry, name: &str) -> &'a str {
        e.fields.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str()).unwrap_or("")
    }

    #[test]
    fn used_and_unused() {
        let dir = tempfile::tempdir().unwrap();
        write(
            &dir.path().join("refs.bib"),
            "@article{smith2020, title = {A Study}, author = {Smith, John}, year = {2020}}\n\
             @article{jones2019, title = {Another Study}, author = {Jones, Ann}, year = {2019}}\n",
        );
        write(
            &dir.path().join("main.tex"),
            "\\documentclass{article}\n\\begin{document}\nAs shown in \\cite{smith2020}, this works.\n\
             % \\cite{jones2019} is only mentioned in a comment\n\\end{document}\n",
        );
        let lib = by_key(dir.path());
        assert_eq!(lib.len(), 2);
        assert!(lib["smith2020"].used);
        assert!(!lib["jones2019"].used, "cited only inside a comment");
    }

    #[test]
    fn cite_variants() {
        let dir = tempfile::tempdir().unwrap();
        write(
            &dir.path().join("refs.bib"),
            "@article{a2020, title = {A}, year = {2020}}\n@article{b2020, title = {B}, year = {2020}}\n@article{c2020, title = {C}, year = {2020}}\n",
        );
        write(&dir.path().join("main.tex"), "\\citep{a2020}\n\\textcite[p.5]{b2020}\n\\cite{c2020, a2020}\n");
        for e in build(dir.path()).unwrap() {
            assert!(e.used, "{} should be used", e.key);
        }
    }

    #[test]
    fn duplicate_doi_ignores_case() {
        let dir = tempfile::tempdir().unwrap();
        write(
            &dir.path().join("refs.bib"),
            "@article{a, title = {Title One}, year = {2020}, doi = {10.1/xyz}}\n\
             @article{b, title = {Totally Different}, year = {2021}, doi = {10.1/XYZ}}\n\
             @article{c, title = {Unrelated}, year = {2022}, doi = {10.2/other}}\n",
        );
        let lib = by_key(dir.path());
        assert!(lib["a"].duplicate && lib["b"].duplicate);
        assert!(!lib["c"].duplicate);
    }

    #[test]
    fn duplicate_similar_title() {
        let dir = tempfile::tempdir().unwrap();
        write(
            &dir.path().join("refs.bib"),
            "@article{a, title = {Attention Is All You Need}, year = {2017}}\n\
             @article{b, title = {Attention is all you need!}, year = {2017}}\n\
             @article{c, title = {A Completely Unrelated Paper About Frogs}, year = {2017}}\n",
        );
        let lib = by_key(dir.path());
        assert!(lib["a"].duplicate && lib["b"].duplicate);
        assert!(!lib["c"].duplicate);
    }

    #[test]
    fn detect_identifier_cases() {
        for (inp, kind, out) in [
            ("10.1038/nphys1170", IdKind::Doi, "10.1038/nphys1170"),
            ("https://doi.org/10.1038/nphys1170", IdKind::Doi, "10.1038/nphys1170"),
            ("doi:10.1038/nphys1170", IdKind::Doi, "10.1038/nphys1170"),
            ("1706.03762", IdKind::Arxiv, "1706.03762"),
            ("arXiv:1706.03762", IdKind::Arxiv, "1706.03762"),
            ("1706.03762v7", IdKind::Arxiv, "1706.03762v7"),
            ("hep-th/9901001", IdKind::Arxiv, "hep-th/9901001"),
            ("978-0-596-52068-7", IdKind::Isbn, "9780596520687"),
            ("0596520689", IdKind::Isbn, "0596520689"),
            ("not an identifier", IdKind::Unknown, "not an identifier"),
        ] {
            assert_eq!(detect_identifier(inp), (kind, out.to_string()), "{inp:?}");
        }
    }

    #[test]
    fn bibtex_name_cases() {
        for (inp, want) in [
            ("Ashish Vaswani", "Vaswani, Ashish"),
            ("Jan Goyvaerts", "Goyvaerts, Jan"),
            ("Madonna", "Madonna"),
            ("Smith, John", "Smith, John"),
            ("Illia Polosukhin", "Polosukhin, Illia"),
        ] {
            assert_eq!(bibtex_name(inp), want);
        }
    }

    #[test]
    fn base_key_and_unique_key() {
        let e = Entry { author: "Vaswani, Ashish and Shazeer, Noam".into(), year: "2017".into(), ..Default::default() };
        assert_eq!(base_key(&e), "vaswani2017");
        let taken: HashSet<String> = ["vaswani2017".to_string(), "vaswani2017a".to_string()].into();
        assert_eq!(unique_key("vaswani2017", &taken), "vaswani2017b");
    }

    #[test]
    fn format_entry() {
        let e = Entry {
            kind: "article".into(),
            key: "vaswani2017".into(),
            title: "Attention Is All You Need".into(),
            author: "Vaswani, Ashish".into(),
            year: "2017".into(),
            fields: vec![("eprint".into(), "1706.03762".into()), ("archivePrefix".into(), "arXiv".into())],
            ..Default::default()
        };
        assert_eq!(
            format_bib_entry(&e),
            "@article{vaswani2017,\n  author = {Vaswani, Ashish},\n  title = {Attention Is All You Need},\n  year = {2017},\n\
             \x20 eprint = {1706.03762},\n  archivePrefix = {arXiv},\n}\n"
        );
    }

    #[test]
    fn import_rejects_unrecognized_identifier() {
        let dir = tempfile::tempdir().unwrap();
        assert!(import(dir.path(), "not an identifier at all").is_err());
    }

    #[test]
    fn bib_target_choice() {
        // None yet: a default at the root.
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(bib_target(dir.path()), dir.path().join("references.bib"));

        // An existing one anywhere is reused.
        write(&dir.path().join("sub/mine.bib"), "");
        assert_eq!(bib_target(dir.path()), dir.path().join("sub/mine.bib"));

        // The one \bibliography{} names beats one the document never reads
        // (which sorts first): an entry there would stay "undefined" forever.
        let dir = tempfile::tempdir().unwrap();
        write(&dir.path().join("aaa-unused.bib"), "");
        write(&dir.path().join("refs.bib"), "");
        write(&dir.path().join("main.tex"), "\\bibliography{refs}");
        assert_eq!(bib_target(dir.path()), dir.path().join("refs.bib"));

        // Named but not on disk yet: still the declared path.
        let dir = tempfile::tempdir().unwrap();
        write(&dir.path().join("main.tex"), "\\bibliography{refs}");
        assert_eq!(bib_target(dir.path()), dir.path().join("refs.bib"));

        // \addbibresource, relative to the .tex that names it.
        let dir = tempfile::tempdir().unwrap();
        write(&dir.path().join("sub/main.tex"), "\\addbibresource{mine.bib}");
        write(&dir.path().join("sub/mine.bib"), "");
        assert_eq!(bib_target(dir.path()), dir.path().join("sub/mine.bib"));
    }

    // Bodies below are trimmed copies of what the real APIs return.

    const CROSSREF_BODY: &str = r#"{"status": "ok", "message": {
        "title": ["Measured measurement"], "container-title": ["Nature Physics"],
        "publisher": "Springer Science and Business Media LLC", "type": "journal-article",
        "volume": "5", "issue": "1", "page": "11-12", "DOI": "10.1038/nphys1170",
        "URL": "https://doi.org/10.1038/nphys1170", "published-print": {"date-parts": [[2009, 1]]},
        "author": [{"given": "Markus", "family": "Aspelmeyer", "sequence": "first"},
                   {"given": "Simon", "family": "Groeblacher", "sequence": "additional"}]}}"#;

    #[test]
    fn crossref_response() {
        let e = parse_crossref(CROSSREF_BODY, "10.1038/nphys1170").unwrap();
        assert_eq!(e.title, "Measured measurement");
        assert_eq!(e.author, "Aspelmeyer, Markus and Groeblacher, Simon");
        assert_eq!((e.year.as_str(), e.kind.as_str()), ("2009", "article"));
        assert_eq!(field(&e, "journal"), "Nature Physics");
        assert!(parse_crossref(r#"{"status":"not-found"}"#, "10.0/nope").is_err());
    }

    #[test]
    fn arxiv_response() {
        let body = r#"<?xml version='1.0' encoding='UTF-8'?>
<feed xmlns="http://www.w3.org/2005/Atom" xmlns:arxiv="http://arxiv.org/schemas/atom">
  <entry>
    <id>http://arxiv.org/abs/1706.03762v7</id>
    <title>Attention Is All You Need</title>
    <published>2017-06-12T17:57:34Z</published>
    <arxiv:primary_category term="cs.CL"/>
    <author><name>Ashish Vaswani</name></author>
    <author><name>Noam Shazeer</name></author>
  </entry>
</feed>"#;
        let e = parse_arxiv(body, "1706.03762").unwrap();
        assert_eq!(e.title, "Attention Is All You Need");
        assert_eq!(e.author, "Vaswani, Ashish and Shazeer, Noam");
        assert_eq!(e.year, "2017");
        assert_eq!(field(&e, "eprint"), "1706.03762");
        assert_eq!(field(&e, "primaryClass"), "cs.CL");
        let empty = r#"<?xml version='1.0'?><feed xmlns="http://www.w3.org/2005/Atom"></feed>"#;
        assert!(parse_arxiv(empty, "0000.00000").is_err());
    }

    #[test]
    fn arxiv_version_suffix() {
        assert_eq!(strip_arxiv_version("1706.03762v7"), "1706.03762");
        assert_eq!(strip_arxiv_version("1706.03762"), "1706.03762");
        assert_eq!(strip_arxiv_version("solv-int/9901001v2"), "solv-int/9901001");
        assert_eq!(strip_arxiv_version("solv-int/9901001"), "solv-int/9901001");
    }

    #[test]
    fn open_library_response() {
        let body = r#"{"ISBN:0596520689": {"title": "Regular Expressions Cookbook",
            "authors": [{"name": "Jan Goyvaerts"}, {"name": "Steven Levithan"}],
            "publishers": [{"name": "O'Reilly Media"}], "publish_date": "2009"}}"#;
        let e = parse_open_library(body, "0596520689").unwrap();
        assert_eq!(e.title, "Regular Expressions Cookbook");
        assert_eq!(e.author, "Goyvaerts, Jan and Levithan, Steven");
        assert_eq!((e.year.as_str(), e.kind.as_str()), ("2009", "book"));
        assert_eq!(field(&e, "publisher"), "O'Reilly Media");
        assert!(parse_open_library("{}", "0000000000").is_err());
    }

    #[test]
    fn add_manual_writes_misc_with_note() {
        let dir = tempfile::tempdir().unwrap();
        let e = add_manual(
            dir.path(),
            &ManualFields {
                title: "Conversa com o orientador".into(),
                author: "Silva, Maria".into(),
                year: "2026".into(),
                note: "Comunicação pessoal, 12/08/2026".into(),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!((e.kind.as_str(), e.key.as_str()), ("misc", "silva2026"));
        let data = fs::read_to_string(&e.file).unwrap();
        assert!(data.contains("@misc{silva2026,"), "{data}");
        assert!(data.contains("note = {Comunicação pessoal, 12/08/2026},"), "{data}");
        let lib = build(dir.path()).unwrap();
        assert_eq!(lib.len(), 1);
        assert_eq!(lib[0].key, "silva2026");
    }

    #[test]
    fn add_manual_requires_title() {
        let dir = tempfile::tempdir().unwrap();
        assert!(add_manual(dir.path(), &ManualFields { author: "Silva, Maria".into(), ..Default::default() }).is_err());
    }

    #[test]
    fn add_manual_key_avoids_existing() {
        let dir = tempfile::tempdir().unwrap();
        write(&dir.path().join("refs.bib"), "@misc{silva2026, title = {Já existe}}");
        let f = ManualFields { title: "Outra coisa".into(), author: "Silva, Maria".into(), year: "2026".into(), ..Default::default() };
        assert_ne!(add_manual(dir.path(), &f).unwrap().key, "silva2026");
    }
}
