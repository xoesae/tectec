//! The LaTeX that comes with the app: Tectonic, a XeTeX-based engine linked
//! into the binary, so writing needs no TeX installation.
//!
//! Tectonic reads the TeX Live files a document needs from a bundle on the
//! web and keeps them in a cache; ours is in the app's data folder. The app
//! ships a *seed* of that cache, with what academic writing uses (abnTeX2,
//! babel, AMS, graphics, tables, beamer…), which is copied into the cache on
//! first use: the usual thesis compiles offline, and only a rarer package is
//! downloaded, once. `--build-tex-seed DIR` makes the seed.
//!
//! The engine keeps global state, so one compilation runs at a time.

use super::latex::{parse_latex_log, CompileResult};
use crate::i18n::t;
use std::fmt::Arguments;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use tectonic::driver::{OutputFormat, ProcessingSessionBuilder};
use tectonic::status::{MessageKind, StatusBackend};
use tectonic::unstable_opts::UnstableOptions;
use tectonic_bundles::Bundle;
use tectonic_io_base::{digest::DigestData, InputHandle, IoProvider, OpenResult};

static ENGINE: Mutex<()> = Mutex::new(());

/// Where the engine keeps the files it has fetched and its format.
pub fn cache_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("TECTEC_TEX_CACHE") {
        return dir.into();
    }
    eframe::storage_dir("tectec").unwrap_or_else(std::env::temp_dir).join("tex")
}

/// The seed shipped with the app: `tex-seed/` next to the executable, or
/// `../share/tectec/tex-seed/` from it (an installed layout).
fn seed_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("TECTEC_TEX_SEED") {
        return Some(dir.into());
    }
    let exe = std::env::current_exe().ok()?;
    let bin = exe.parent()?;
    [bin.join("tex-seed"), bin.join("../share/tectec/tex-seed")].into_iter().find(|d| d.is_dir())
}

/// Copies the seed's files the cache lacks. Once per run.
fn seed_cache(cache: &Path) {
    static DONE: OnceLock<()> = OnceLock::new();
    DONE.get_or_init(|| {
        let Some(seed) = seed_dir() else { return };
        // The seed already copied, as it is now: no need to look at its
        // thousands of files again.
        let stamp = std::fs::metadata(seed.join("bundles"))
            .and_then(|m| m.modified())
            .map(|t| format!("{} {:?}", seed.display(), t))
            .unwrap_or_default();
        let marker = cache.join("seeded");
        if !stamp.is_empty() && std::fs::read_to_string(&marker).ok().as_deref() == Some(stamp.as_str()) {
            return;
        }
        let copied = copy_missing(&seed, cache);
        let _ = std::fs::write(&marker, &stamp);
        if copied > 0 {
            log::info!("TeX cache: {copied} files copied from the seed at {}", seed.display());
        }
    });
}

fn copy_missing(from: &Path, to: &Path) -> usize {
    let Ok(entries) = std::fs::read_dir(from) else { return 0 };
    let _ = std::fs::create_dir_all(to);
    let mut copied = 0;
    for entry in entries.flatten() {
        let (src, dst) = (entry.path(), to.join(entry.file_name()));
        if src.is_dir() {
            copied += copy_missing(&src, &dst);
        } else if !dst.exists() && std::fs::copy(&src, &dst).is_ok() {
            copied += 1;
        }
    }
    copied
}

/// Collects what Tectonic reports: its errors (its notes and warnings,
/// "Missing character" and such, are in the TeX log already), and the log
/// it hands over when it stops on an error.
#[derive(Default)]
struct Collect {
    messages: String,
    error_log: String,
}

impl StatusBackend for Collect {
    fn report(&mut self, kind: MessageKind, args: Arguments, err: Option<&tectonic::Error>) {
        if kind != MessageKind::Error {
            return;
        }
        self.messages.push_str(&args.to_string());
        if let Some(e) = err {
            for cause in e.chain() {
                self.messages.push_str(&format!(": {cause}"));
            }
        }
        self.messages.push('\n');
    }

    fn dump_error_logs(&mut self, output: &[u8]) {
        self.error_log.push_str(&String::from_utf8_lossy(output));
    }
}

fn bundle_url() -> String {
    tectonic::config::PersistentConfig::default().default_bundle_loc().to_owned()
}

fn open_bundle(cache: &Path, only_cached: bool) -> Result<Box<dyn Bundle>, String> {
    tectonic_bundles::detect_bundle(bundle_url(), only_cached, Some(cache.join("bundles")))
        .map_err(|e| offline_hint(&e.to_string()))?
        .ok_or_else(|| t("The LaTeX package bundle could not be opened.").to_string())
}

/// The bundle, read from the cache first: the network is only tried for a
/// file the cache lacks, and after it fails once, not again in the same
/// compilation. (Tectonic alone retries every missing file three times,
/// which offline adds up to minutes.)
struct CacheFirst {
    cached: Box<dyn Bundle>,
    online: Option<Box<dyn Bundle>>,
    cache: PathBuf,
    offline: bool,
}

impl CacheFirst {
    fn new(cache: &Path) -> Result<Self, String> {
        Ok(Self { cached: open_bundle(cache, true)?, online: None, cache: cache.to_path_buf(), offline: false })
    }
}

impl IoProvider for CacheFirst {
    fn input_open_name(&mut self, name: &str, status: &mut dyn tectonic::status::StatusBackend) -> OpenResult<InputHandle> {
        match self.cached.input_open_name(name, status) {
            OpenResult::NotAvailable if !self.offline => {}
            other => return other,
        }
        if self.online.is_none() {
            match open_bundle(&self.cache, false) {
                Ok(b) => self.online = Some(b),
                Err(_) => {
                    self.offline = true;
                    return OpenResult::NotAvailable;
                }
            }
        }
        match self.online.as_mut().unwrap().input_open_name(name, status) {
            OpenResult::Err(e) => {
                log::warn!("TeX file {name} could not be downloaded: {e}");
                self.offline = true;
                OpenResult::NotAvailable
            }
            other => other,
        }
    }

    fn input_open_format(&mut self, name: &str, status: &mut dyn tectonic::status::StatusBackend) -> OpenResult<InputHandle> {
        self.input_open_name(name, status)
    }
}

impl Bundle for CacheFirst {
    fn get_digest(&mut self) -> tectonic_errors::Result<DigestData> {
        self.cached.get_digest()
    }

    fn all_files(&self) -> Vec<String> {
        self.cached.all_files()
    }
}

/// Files read before the bundle's, where the built-in LaTeX has to differ.
///
/// `fontenc.sty`: under XeTeX an 8-bit encoding (`\usepackage[T1]{fontenc}`,
/// in most documents written for pdfLaTeX) drops every character past
/// Latin-1 from the PDF: —, “ ”, …. This one declares the encodings asked
/// for, so their commands work, but keeps the text in TU, XeTeX's Unicode
/// encoding.
fn overrides(cache: &Path) -> PathBuf {
    const FONTENC: &str = r"\NeedsTeXFormat{LaTeX2e}
\ProvidesPackage{fontenc}[2025/01/01 TecTec: encodings declared, text kept in TU under XeTeX]
\DeclareOption*{%
  \edef\tectec@enc{\lowercase{\noexpand\InputIfFileExists{\CurrentOption enc.def}{}{}}}%
  \@ifundefined{T@\CurrentOption}{\tectec@enc}{}}
\ProcessOptions*
\@ifundefined{T@TU}{\input{tuenc.def}}{}
\renewcommand\encodingdefault{TU}
\fontencoding{TU}\selectfont
";
    let dir = cache.join("overrides");
    let file = dir.join("fontenc.sty");
    if std::fs::read_to_string(&file).ok().as_deref() != Some(FONTENC) {
        let _ = std::fs::create_dir_all(&dir);
        let _ = std::fs::write(&file, FONTENC);
    }
    dir
}

/// The bundle, opened once and kept: opening it reads its 5 MB index of
/// TeX Live's files, a third of a short compilation's time.
static BUNDLE: Mutex<Option<(PathBuf, Arc<Mutex<Kept>>)>> = Mutex::new(None);

/// `CacheFirst` held across compilations.
struct Kept(CacheFirst);

// SAFETY: Tectonic's bundles are only used by the compilation holding the
// `ENGINE` lock, one thread at a time; they are trait objects without a
// `Send` bound, not types that are tied to a thread.
unsafe impl Send for Kept {}

/// What a session gets: the kept bundle, behind its lock.
struct Shared(Arc<Mutex<Kept>>);

impl Shared {
    fn with<R>(&self, f: impl FnOnce(&mut CacheFirst) -> R) -> R {
        f(&mut self.0.lock().unwrap_or_else(|e| e.into_inner()).0)
    }
}

impl IoProvider for Shared {
    fn input_open_name(&mut self, name: &str, status: &mut dyn tectonic::status::StatusBackend) -> OpenResult<InputHandle> {
        self.with(|b| b.input_open_name(name, status))
    }

    fn input_open_format(&mut self, name: &str, status: &mut dyn tectonic::status::StatusBackend) -> OpenResult<InputHandle> {
        self.with(|b| b.input_open_format(name, status))
    }
}

impl Bundle for Shared {
    fn get_digest(&mut self) -> tectonic_errors::Result<DigestData> {
        self.with(|b| b.get_digest())
    }

    fn all_files(&self) -> Vec<String> {
        self.with(|b| b.all_files())
    }
}

fn kept_bundle(cache: &Path) -> Result<Shared, String> {
    let mut kept = BUNDLE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((dir, bundle)) = kept.as_ref() {
        if dir == cache {
            let shared = Shared(bundle.clone());
            // A new compilation may try the network again.
            shared.with(|b| b.offline = false);
            return Ok(shared);
        }
    }
    let bundle = Arc::new(Mutex::new(Kept(CacheFirst::new(cache)?)));
    *kept = Some((cache.to_path_buf(), bundle.clone()));
    Ok(Shared(bundle))
}

fn session(cache: &Path) -> Result<ProcessingSessionBuilder, String> {
    seed_cache(cache);
    let bundle = Box::new(kept_bundle(cache)?);
    // Extra search paths count as an "insecure" feature to Tectonic; shell
    // escape stays off.
    let security = tectonic_bridge_core::SecuritySettings::new(tectonic_bridge_core::SecurityStance::MaybeAllowInsecures);
    let mut sb = ProcessingSessionBuilder::new_with_security(security);
    sb.unstables(unstables(cache, false));
    sb.bundle(bundle)
        .format_name("latex")
        .format_cache_path(cache.join("formats"))
        .output_format(OutputFormat::Pdf)
        // `\today`: Tectonic's default is a fixed date (1970).
        .build_date(std::time::SystemTime::now())
        .print_stdout(false);
    Ok(sb)
}

fn unstables(cache: &Path, continue_on_errors: bool) -> UnstableOptions {
    UnstableOptions { continue_on_errors, extra_search_paths: vec![overrides(cache)], ..Default::default() }
}

/// A clearer message for the error of a file that is neither cached nor
/// downloadable.
fn offline_hint(message: &str) -> String {
    if message.contains("couldn't get it from the internet") || message.contains("error sending request") || message.contains("dns error") {
        t("A LaTeX package this document needs is not in the app yet, and it could not be downloaded. Connect to the internet once so it is fetched; after that it works offline.").into()
    } else {
        message.to_string()
    }
}

/// Typesets `main_file` next to itself: the PDF, its log, the SyncTeX map
/// and the `.aux` files, as pdflatex would. The engine reruns itself and
/// runs BibTeX when the document needs it.
pub fn compile(main_file: &Path) -> Result<CompileResult, String> {
    let _engine = ENGINE.lock().unwrap_or_else(|e| e.into_inner());
    let dir = main_file.parent().unwrap_or(Path::new(".")).to_path_buf();
    let name = main_file.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let mut sb = session(&cache_dir())?;
    sb.primary_input_path(main_file)
        .tex_input_name(&name)
        .filesystem_root(&dir)
        .output_dir(&dir)
        .keep_logs(true)
        .keep_intermediates(true)
        .synctex(true)
        // A preview with an error in it beats no preview, as with pdflatex's
        // nonstop mode.
        .unstables(unstables(&cache_dir(), true));
    let mut status = Collect::default();
    let outcome = sb.create(&mut status).and_then(|mut s| s.run(&mut status));
    let log_file = main_file.with_extension("log");
    let mut log = if status.error_log.is_empty() { std::fs::read_to_string(&log_file).unwrap_or_default() } else { status.error_log.clone() };
    let mut result = CompileResult { engine: t("built-in LaTeX").into(), passes: 0, ..Default::default() };
    if let Err(e) = &outcome {
        let mut why = e.to_string();
        for cause in e.chain().skip(1) {
            why.push_str(&format!(": {cause}"));
        }
        // Tectonic's own complaint (a package it could not fetch…), as a
        // TeX-style error so the diagnostics list shows it.
        log.push_str(&format!("\n! {}\n", offline_hint(&why)));
        if !status.messages.is_empty() {
            log.push_str(&status.messages);
        }
    }
    result.errors = parse_latex_log(&log, main_file);
    result.log_text = log;
    let pdf = main_file.with_extension("pdf");
    if outcome.is_ok() && std::fs::metadata(&pdf).map(|m| m.len() > 0).unwrap_or(false) {
        result.success = !result.errors.iter().any(|e| !e.warning);
        result.pdf_path = Some(pdf);
    }
    Ok(result)
}

/// Typesets a small standalone document and returns its PDF, or the TeX log
/// of the error that stopped it. For the formula preview.
pub fn typeset(source: &str) -> Result<Vec<u8>, String> {
    let _engine = ENGINE.lock().unwrap_or_else(|e| e.into_inner());
    // An empty folder: nothing lying around is read as the formula's `.aux`.
    let scratch = tempfile_dir()?;
    let mut sb = session(&cache_dir())?;
    sb.primary_input_buffer(source.as_bytes())
        .tex_input_name("formula.tex")
        .filesystem_root(&scratch.0)
        .keep_logs(true)
        .keep_intermediates(false)
        .do_not_write_output_files();
    let mut status = Collect::default();
    let mut session = sb.create(&mut status).map_err(|e| e.to_string())?;
    let outcome = session.run(&mut status);
    let mut files = session.into_file_data();
    let log = files.remove("formula.log").map(|f| String::from_utf8_lossy(&f.data).into_owned());
    match (outcome, files.remove("formula.pdf")) {
        (Ok(()), Some(pdf)) => Ok(pdf.data),
        (outcome, _) => Err(log.filter(|l| !l.is_empty()).or((!status.error_log.is_empty()).then_some(status.error_log)).unwrap_or_else(|| {
            outcome.err().map(|e| e.to_string()).unwrap_or_else(|| t("Invalid formula").into())
        })),
    }
}

/// A scratch folder, removed when dropped.
struct Scratch(PathBuf);

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn tempfile_dir() -> Result<Scratch, String> {
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("tectec-tex-{}-{n}", std::process::id()));
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(Scratch(dir))
}

/// Builds the seed into `out`: typesets documents using what academic
/// writing needs, so the files they read end up in a cache there.
pub fn build_seed(out: &Path) -> Result<(), String> {
    std::fs::create_dir_all(out).map_err(|e| e.to_string())?;
    // SAFETY: before any other thread reads the environment (the app does
    // not start in this mode).
    unsafe { std::env::set_var("TECTEC_TEX_CACHE", out) };
    let work = out.join("..").join(".tex-seed-work");
    let _ = std::fs::remove_dir_all(&work);
    for (i, (template, english)) in super::templates::Template::ALL.iter().flat_map(|t| [(*t, false), (*t, true)]).enumerate() {
        let dir = work.join(format!("t{i}"));
        let main = super::templates::create_in(&dir, template, english)?;
        report(&main, compile(&main)?);
    }
    let extras = work.join("extras");
    std::fs::create_dir_all(&extras).map_err(|e| e.to_string())?;
    let main = extras.join("extras.tex");
    std::fs::write(&main, EXTRAS).map_err(|e| e.to_string())?;
    std::fs::write(extras.join("refs.bib"), "@book{k, author={A. Autor}, title={Título}, year={2024}, publisher={Editora}}\n").map_err(|e| e.to_string())?;
    report(&main, compile(&main)?);
    let _ = std::fs::remove_dir_all(&work);
    // Whole font families, not only the sizes and shapes the documents
    // above happened to use: Latin Modern in every encoding, Computer
    // Modern and the AMS fonts.
    let mut bundle = open_bundle(out, false)?;
    let mut status = Collect::default();
    // The index of the bundle's files is read on the first lookup.
    let _ = bundle.input_open_name("article.cls", &mut status);
    let wanted: Vec<String> = bundle.all_files().into_iter().filter(|n| whole_family(n) || ALWAYS.contains(&n.rsplit('/').next().unwrap_or(n))).collect();
    let mut fetched = 0;
    for name in &wanted {
        if let OpenResult::Ok(_) = bundle.input_open_name(name, &mut status) {
            fetched += 1;
        }
    }
    println!("fonts: {fetched} of {} files", wanted.len());
    Ok(())
}

/// Files of LaTeX itself any document may load whatever the templates use:
/// the standard classes and their font-size options (10pt is the default).
const ALWAYS: &[&str] = &[
    "article.cls", "report.cls", "book.cls", "letter.cls", "proc.cls", "slides.cls", "minimal.cls",
    "size10.clo", "size11.clo", "size12.clo", "bk10.clo", "bk11.clo", "bk12.clo", "leqno.clo", "fleqn.clo", "openbib.sty",
];

/// A file of a font family the seed carries whole.
fn whole_family(name: &str) -> bool {
    const PREFIXES: &[&str] = &[
        // Latin Modern: metrics per encoding, outlines, encodings, font definitions.
        "ec-lm", "ts1-lm", "rm-lm", "texnansi-lm", "qx-lm", "t5-lm", "l7x-lm", "cs-lm", "lm",
        "t1lm", "ot1lm", "ts1lm", "omllm", "omslm", "omxlm", "ly1lm", "qxlm", "t5lm",
        // Computer Modern and the AMS symbol fonts.
        "cmr", "cmbx", "cmti", "cmsl", "cmtt", "cmss", "cmmi", "cmsy", "cmex", "cmbsy", "cmcsc", "cmu",
        "msam", "msbm", "eufm", "eurm", "eusm",
    ];
    let stem = name.rsplit('/').next().unwrap_or(name);
    let font_file = [".tfm", ".pfb", ".enc", ".fd", ".map", ".afm", ".otf"].iter().any(|e| stem.ends_with(e));
    font_file && PREFIXES.iter().any(|p| stem.starts_with(p))
}

fn report(main: &Path, result: CompileResult) {
    let status = if result.pdf_path.is_some() { "ok" } else { "FAILED" };
    println!("{status}: {}", main.display());
    if result.pdf_path.is_none() {
        for e in result.errors.iter().filter(|e| !e.warning).take(5) {
            println!("   {}", e.message);
        }
    }
}

/// The packages a thesis, an article or a report commonly loads beyond the
/// templates'.
const EXTRAS: &str = r"\documentclass[12pt,a4paper]{article}
\usepackage[T1]{fontenc}
\usepackage[utf8]{inputenc}
\usepackage{lmodern}
\usepackage[brazil,english]{babel}
\usepackage{csquotes}
\usepackage{amsmath,amssymb,amsthm,mathtools}
\usepackage{bm}
\usepackage{siunitx}
\usepackage{graphicx}
\usepackage{float}
\usepackage{caption}
\usepackage{subcaption}
\usepackage{wrapfig}
\usepackage{booktabs,multirow,longtable,tabularx,array}
\usepackage{xcolor}
\usepackage{tikz}
\usepackage{pgfplots}
\pgfplotsset{compat=1.18}
\usepackage{listings}
\usepackage{algorithm}
\usepackage{algpseudocode}
\usepackage{geometry}
\usepackage{setspace}
\usepackage{indentfirst}
\usepackage{enumitem}
\usepackage{fancyhdr}
\usepackage{titlesec}
\usepackage{microtype}
\usepackage{url}
\usepackage{natbib}
\usepackage{hyperref}
\usepackage{cleveref}
\usepackage{glossaries}
\usepackage{todonotes}
\usepackage{lipsum}
\begin{document}
\section{Texto}
Um texto com \cite{k}, \SI{3}{\meter} e $\bm{x} \in \mathbb{R}^n$ \cref{sec}.
\begin{figure}[H]\centering
\begin{tikzpicture}\begin{axis}\addplot{x^2};\end{axis}\end{tikzpicture}
\caption{Figura}\label{sec}\end{figure}
\begin{table}[h]\centering\begin{tabular}{lr}\toprule a & b\\\midrule 1 & 2\\\bottomrule\end{tabular}\end{table}
\begin{lstlisting}
int main() { return 0; }
\end{lstlisting}
\begin{algorithmic}\State $x \gets 1$\end{algorithmic}
\bibliographystyle{plainnat}
\bibliography{refs}
\end{document}
";


