//! Compiles the document and turns the log into diagnostics.
//!
//! The input is the absolute path of the main `.tex` (already validated by
//! the caller); cancellation comes from a shared flag so a new compilation can
//! drop the previous one.

use regex::Regex;
use crate::i18n::t;
use crate::tf;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock};
use std::time::{Duration, Instant};

/// Safety ceiling: a pathological document must not hold the compiler forever.
const COMPILE_TIMEOUT: Duration = Duration::from_secs(180);
/// Extra pdflatex passes accepted in the fallback without latexmk.
const MAX_EXTRA_PASSES: usize = 3;

#[derive(Clone, Debug, Default)]
pub struct EngineInfo {
    /// "latexmk", "pdflatex" or "".
    pub name: String,
    pub available: bool,
    pub bibtex: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CompileError {
    pub file: Option<PathBuf>,
    /// 1-based; 0 when the log does not say.
    pub line: usize,
    pub message: String,
    pub warning: bool,
}

#[derive(Clone, Debug, Default)]
pub struct CompileResult {
    pub success: bool,
    pub pdf_path: Option<PathBuf>,
    pub log_text: String,
    pub errors: Vec<CompileError>,
    pub engine: String,
    pub passes: usize,
    /// Dropped by a newer compilation (or out of time).
    pub canceled: bool,
    /// Refused because it would overwrite a PDF this app never produced.
    pub overwrite_risk: bool,
}

/// Whether documents go through the TeX installed on the system instead of
/// the one built into the app (`embedded`). Set from the preferences.
static SYSTEM_TEX: AtomicBool = AtomicBool::new(false);

pub fn set_system_tex(on: bool) {
    SYSTEM_TEX.store(on, Ordering::Relaxed);
}

pub fn system_tex() -> bool {
    SYSTEM_TEX.load(Ordering::Relaxed)
}

/// Whether the TeX in use has a file. The built-in one fetches any file of
/// TeX Live it lacks, so it has them all.
pub fn has_file(file: &str) -> bool {
    !system_tex() || installed(file)
}

pub fn which(bin: &str) -> bool {
    let Some(paths) = std::env::var_os("PATH") else { return false };
    std::env::split_paths(&paths).any(|dir| {
        let p = dir.join(bin);
        p.is_file() || (cfg!(windows) && dir.join(format!("{bin}.exe")).is_file())
    })
}

/// Whether the TeX installation has a file (`brazil.ldf`, `abntex2.cls`),
/// asking `kpsewhich`. False when it cannot be asked.
pub fn installed(file: &str) -> bool {
    std::process::Command::new("kpsewhich")
        .arg(file)
        .stdin(Stdio::null())
        .output()
        .map(|o| o.status.success() && !o.stdout.is_empty())
        .unwrap_or(false)
}

/// The file a compilation error says is missing: `File `x.sty' not found`,
/// or the `.ldf` of a language babel does not know.
pub fn missing_file(message: &str) -> Option<String> {
    static FILE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"File [`'‘]([^'’]+)['’] not found").unwrap());
    static BABEL: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)babel.*unknown option [`'‘]([a-z]+)['’]").unwrap());
    // fontspec: `Font \TU/lmr/m/n/10=[lmroman10-regular]:+tlig; at 10pt not loadable`.
    static FONT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"Font \\\S+=\[?([^\]:;]+)\]?[^ ]*.*not loadable").unwrap());
    if let Some(c) = FILE.captures(message) {
        return Some(c[1].to_string());
    }
    if let Some(c) = FONT.captures(message) {
        return Some(format!("{}.otf", c[1].trim()));
    }
    BABEL.captures(message).map(|c| format!("{}.ldf", &c[1]))
}

/// How the system installs the TeX package that provides `file`, as a
/// command to paste in a terminal; `None` where the distribution installs
/// on its own (MiKTeX) or cannot be told.
pub fn install_command(file: &str) -> Option<String> {
    if !system_tex() {
        return None; // The built-in LaTeX fetches it on its own.
    }
    let stem = file.rsplit_once('.').map(|(s, _)| s).unwrap_or(file);
    // TeX Live's package name, and Debian's, for the files people miss most.
    let (tlmgr, debian) = match stem {
        "brazil" | "brazilian" | "portuges" | "portuguese" => ("babel-portuges", "texlive-lang-portuguese"),
        "english" | "american" | "british" => ("babel-english", "texlive-lang-english"),
        s if s.starts_with("abntex2") => ("abntex2", "texlive-publishers"),
        "biblatex" => ("biblatex", "texlive-bibtex-extra biber"),
        "fontspec" | "polyglossia" | "xelatex" | "xetex" => ("xetex", "texlive-xetex"),
        "lualatex" | "luatex" | "luaotfload-main" => ("luaotfload", "texlive-luatex"),
        s if s.starts_with("lmroman") || s.starts_with("lmsans") || s.starts_with("lmmono") => ("lm", "fonts-lmodern"),
        _ => (stem, "texlive-latex-extra"),
    };
    match installer() {
        Installer::Apt => Some(format!("sudo apt install {debian}")),
        Installer::Dnf => Some(format!("sudo dnf install 'tex({file})'")),
        Installer::Tlmgr => Some(format!("tlmgr install {tlmgr}")),
        Installer::Unknown => None,
    }
}

#[derive(Clone, Copy)]
enum Installer {
    /// TeX Live from Debian's or Ubuntu's packages (their tlmgr does not install).
    Apt,
    Dnf,
    Tlmgr,
    Unknown,
}

/// Asked once per process.
fn installer() -> Installer {
    static FOUND: std::sync::OnceLock<Installer> = std::sync::OnceLock::new();
    *FOUND.get_or_init(|| {
        let system = kpsewhich_path("article.cls").is_some_and(|p| p.starts_with("/usr/share/texlive"));
        if system && Path::new("/etc/debian_version").exists() {
            Installer::Apt
        } else if system && Path::new("/etc/fedora-release").exists() {
            Installer::Dnf
        } else if which("tlmgr") {
            Installer::Tlmgr
        } else {
            Installer::Unknown
        }
    })
}

fn kpsewhich_path(file: &str) -> Option<String> {
    let out = std::process::Command::new("kpsewhich").arg(file).stdin(Stdio::null()).output().ok()?;
    Some(String::from_utf8_lossy(&out.stdout).trim().to_string()).filter(|p| !p.is_empty())
}

pub fn detect_engine() -> EngineInfo {
    if !system_tex() {
        return EngineInfo { name: "built-in".into(), available: true, bibtex: true };
    }
    if which("latexmk") {
        return EngineInfo { name: "latexmk".into(), available: true, bibtex: true };
    }
    if which("pdflatex") {
        return EngineInfo { name: "pdflatex".into(), available: true, bibtex: which("bibtex") };
    }
    EngineInfo::default()
}

/// `-file-line-error` makes pdflatex emit `file:line: message`;
/// `-synctex=1` produces the map the synced scrolling uses.
fn common_args(dir: &Path) -> Vec<String> {
    vec![
        "-interaction=nonstopmode".into(),
        "-file-line-error".into(),
        "-synctex=1".into(),
        format!("-output-directory={}", dir.display()),
    ]
}

/// Runs a command and returns its combined output. The exit status is ignored
/// on purpose: whether the PDF exists decides success. `Err` only means the
/// run was canceled or timed out.
pub(crate) fn run(cancel: &AtomicBool, deadline: Instant, dir: &Path, bin: &str, args: &[String]) -> Result<String, ()> {
    let mut child = match Command::new(bin)
        .args(args)
        .current_dir(dir)
        // TeX wraps log lines at 79 columns, which splits long paths in
        // `file:line:` diagnostics; kpathsea reads these from the environment.
        .env("max_print_line", "10000")
        .env("error_line", "254")
        .env("half_error_line", "238")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => return Ok(tf!("could not run {bin}: {e}", bin, e) + "\n"),
    };
    let mut stdout = child.stdout.take().unwrap();
    let mut stderr = child.stderr.take().unwrap();
    let out_thread = std::thread::spawn(move || {
        let mut s = Vec::new();
        let _ = stdout.read_to_end(&mut s);
        s
    });
    let err_thread = std::thread::spawn(move || {
        let mut s = Vec::new();
        let _ = stderr.read_to_end(&mut s);
        s
    });
    loop {
        if cancel.load(Ordering::Relaxed) || Instant::now() > deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(());
        }
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) => std::thread::sleep(Duration::from_millis(40)),
            Err(_) => break,
        }
    }
    let mut out = out_thread.join().unwrap_or_default();
    out.extend(err_thread.join().unwrap_or_default());
    Ok(String::from_utf8_lossy(&out).into_owned())
}

static RERUN_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(rerun to get|rerun latex|label\(s\) may have changed|there were undefined (references|citations))")
        .unwrap()
});

fn needs_another_pass(log: &str) -> bool {
    RERUN_RE.is_match(log)
}

fn has_citations(aux: &Path) -> bool {
    fs::read_to_string(aux).map(|s| s.contains("\\citation{")).unwrap_or(false)
}

fn with_ext(path: &Path, ext: &str) -> PathBuf {
    path.with_extension(ext)
}

/// Compiles `main_file` (absolute) and returns the result with the
/// diagnostics already positioned.
/// The TeX engine a document is typeset with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Program {
    PdfLatex,
    XeLatex,
    LuaLatex,
}

impl Program {
    pub fn binary(self) -> &'static str {
        match self {
            Program::PdfLatex => "pdflatex",
            Program::XeLatex => "xelatex",
            Program::LuaLatex => "lualatex",
        }
    }

    fn latexmk_flag(self) -> &'static str {
        match self {
            Program::PdfLatex => "-pdf",
            Program::XeLatex => "-xelatex",
            Program::LuaLatex => "-lualatex",
        }
    }
}

/// The engine a main file asks for: a `% !TEX program = xelatex` line near
/// its top (the TeXstudio / TeXShop / VS Code convention) wins; else a
/// preamble loading fontspec, unicode-math or polyglossia (which only
/// XeLaTeX and LuaLaTeX run) gets XeLaTeX, or LuaLaTeX where XeLaTeX is not
/// installed; else pdfLaTeX.
pub fn program_for(text: &str, installed: impl Fn(&str) -> bool) -> Program {
    static MAGIC: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?mi)^\s*%\s*!\s*TeX\s+(?:TS-)?program\s*=\s*(\w+)").unwrap());
    static UNICODE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"\\usepackage\s*(?:\[[^\]]*\])?\s*\{[^}]*\b(fontspec|unicode-math|polyglossia)\b").unwrap());
    let head: String = text.lines().take(30).collect::<Vec<_>>().join("\n");
    if let Some(c) = MAGIC.captures(&head) {
        match c[1].to_lowercase().as_str() {
            "xelatex" | "xetex" => return Program::XeLatex,
            "lualatex" | "luatex" => return Program::LuaLatex,
            "pdflatex" | "pdftex" => return Program::PdfLatex,
            _ => {}
        }
    }
    let preamble_end = text.find("\\begin{document}").unwrap_or(text.len());
    let preamble: String = text[..preamble_end].lines().map(super::texparse::without_comment).collect::<Vec<_>>().join("\n");
    if UNICODE.is_match(&preamble) {
        return if installed("xelatex") || !installed("lualatex") { Program::XeLatex } else { Program::LuaLatex };
    }
    Program::PdfLatex
}

/// T1-encoded text has no vector fonts of its own without cm-super: pdfLaTeX
/// then typesets in bitmaps unless a font package (lmodern…) is loaded.
/// Asked once per process.
pub fn t1_bitmap() -> bool {
    static BITMAP: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    // XeTeX, under the built-in LaTeX, uses vector fonts either way.
    system_tex() && *BITMAP.get_or_init(|| which("kpsewhich") && !installed("cm-super-t1.enc"))
}

/// Whether an engine can run: its binary, and for LuaLaTeX the luaotfload
/// module too (Debian ships the binary without it, and then no font loads).
pub fn engine_ready(binary: &str) -> bool {
    which(binary) && (binary != "lualatex" || installed("luaotfload-main.lua"))
}

pub fn compile(cancel: Arc<AtomicBool>, main_file: &Path) -> Result<CompileResult, String> {
    if !system_tex() {
        if cancel.load(Ordering::Relaxed) {
            return Ok(CompileResult { canceled: true, ..Default::default() });
        }
        return super::embedded::compile(main_file);
    }
    compile_system(cancel, main_file)
}

/// `compile` with the TeX installed on the system.
pub fn compile_system(cancel: Arc<AtomicBool>, main_file: &Path) -> Result<CompileResult, String> {
    let engine = detect_engine();
    if !engine.available {
        return Err(t("no LaTeX compiler found on the PATH").into());
    }
    let program = program_for(&fs::read_to_string(main_file).unwrap_or_default(), engine_ready);
    if !engine_ready(program.binary()) {
        let name = program.binary();
        return Err(match install_command(name) {
            Some(command) => tf!("{engine} is not installed. To install it: {command}", engine = name, command),
            None => tf!("{engine} is not installed.", engine = name),
        });
    }
    let deadline = Instant::now() + COMPILE_TIMEOUT;
    let dir = main_file.parent().unwrap_or(Path::new(".")).to_path_buf();
    let base = main_file.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();

    let shown = match (engine.name.as_str(), program) {
        ("latexmk", Program::PdfLatex) => "latexmk".to_string(),
        ("latexmk", p) => format!("latexmk ({})", p.binary()),
        (_, p) => p.binary().to_string(),
    };
    let mut result = CompileResult { engine: shown, ..Default::default() };
    match run_passes(&cancel, deadline, &engine, program, &dir, &base) {
        Err((log, passes)) => {
            result.log_text = log;
            result.passes = passes;
            result.canceled = true;
            return Ok(result);
        }
        Ok((log, passes)) => {
            result.log_text = log;
            result.passes = passes;
        }
    }
    result.errors = parse_latex_log(&result.log_text, main_file);
    let pdf = with_ext(main_file, "pdf");
    if fs::metadata(&pdf).map(|m| m.len() > 0).unwrap_or(false) {
        result.success = !result.errors.iter().any(|e| !e.warning);
        result.pdf_path = Some(pdf);
    }
    Ok(result)
}

/// Whether the output PDF exists but was never produced by a compilation from
/// this app: every real compilation leaves an `.aux` next to it.
pub fn foreign_pdf(main_file: &Path) -> bool {
    let pdf_ok = fs::metadata(with_ext(main_file, "pdf")).map(|m| m.len() > 0).unwrap_or(false);
    pdf_ok && !with_ext(main_file, "aux").exists()
}

#[derive(Clone, Debug)]
pub struct PdfStatus {
    pub path: PathBuf,
    pub exists: bool,
    pub stale: bool,
}

/// The PDF already next to the main file, and whether it is older than it.
pub fn inspect_pdf(main_file: &Path) -> PdfStatus {
    let path = with_ext(main_file, "pdf");
    let pdf_mod = fs::metadata(&path).ok().filter(|m| m.len() > 0).and_then(|m| m.modified().ok());
    let Some(pdf_mod) = pdf_mod else {
        return PdfStatus { path, exists: false, stale: true };
    };
    let stale = fs::metadata(main_file).and_then(|m| m.modified()).map(|src| src > pdf_mod).unwrap_or(false);
    PdfStatus { path, exists: true, stale }
}

type PassOutcome = Result<(String, usize), (String, usize)>;

fn run_passes(cancel: &AtomicBool, deadline: Instant, engine: &EngineInfo, program: Program, dir: &Path, base: &str) -> PassOutcome {
    if engine.name == "latexmk" {
        let mut args: Vec<String> = vec![program.latexmk_flag().into(), "-halt-on-error".into()];
        args.extend(common_args(dir));
        args.push(base.into());
        return match run(cancel, deadline, dir, "latexmk", &args) {
            Ok(log) => Ok((log, 1)),
            Err(()) => Err((String::new(), 1)),
        };
    }

    // Fallback without latexmk: a single pass never resolves \ref, \cite or
    // \tableofcontents, because those read the .aux the previous pass wrote.
    let mut logs = String::new();
    let mut passes = 0;
    let pdflatex = |logs: &mut String, passes: &mut usize| -> Result<(), ()> {
        let mut args = common_args(dir);
        args.push(base.into());
        let out = run(cancel, deadline, dir, program.binary(), &args)?;
        logs.push_str(&out);
        *passes += 1;
        Ok(())
    };

    if pdflatex(&mut logs, &mut passes).is_err() {
        return Err((logs, passes));
    }
    let stem = Path::new(base).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let aux = dir.join(format!("{stem}.aux"));
    if engine.bibtex && has_citations(&aux) {
        match run(cancel, deadline, dir, "bibtex", &[stem.clone()]) {
            Ok(out) => logs.push_str(&out),
            Err(()) => return Err((logs, passes)),
        }
        if pdflatex(&mut logs, &mut passes).is_err() {
            return Err((logs, passes));
        }
    }
    for _ in 0..MAX_EXTRA_PASSES {
        if !needs_another_pass(&logs) {
            break;
        }
        let before = logs.len();
        if pdflatex(&mut logs, &mut passes).is_err() {
            return Err((logs, passes));
        }
        if !needs_another_pass(&logs[before..]) {
            break;
        }
    }
    Ok((logs, passes))
}

// ---------- Log parser ----------

static FILE_LINE_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(.*?):(\d+):\s*(.+)$").unwrap());
static BANG_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^!\s*(.+)$").unwrap());
static WARNING_LINE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(?:LaTeX|Package \w+) Warning:\s*(.+?)(?:\s+on input line (\d+))?\.$").unwrap()
});

static CONTEXT_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^l\.\d+\s?(.*)$").unwrap());
static LAST_COMMAND_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(\\[A-Za-z@]+\*?)\s*$").unwrap());

/// For an error, the `l.<n> …` line TeX prints a few lines below it: the
/// source up to the point where it stopped. "Undefined control sequence"
/// gains the command's name from it.
fn with_context(message: &str, lines: &[&str], at: usize) -> String {
    let context = lines[at + 1..(at + 8).min(lines.len())].iter().find_map(|l| CONTEXT_RE.captures(l.trim_end()).map(|c| c[1].to_string()));
    match context {
        Some(ctx) if message.starts_with("Undefined control sequence") => match LAST_COMMAND_RE.captures(ctx.trim_end()) {
            Some(cmd) => format!("Undefined control sequence: {}", &cmd[1]),
            None => message.to_string(),
        },
        _ => message.to_string(),
    }
}

/// The files TeX has open, from the log: `(./cap/um.tex` opens one, `)`
/// closes the innermost. Other parentheses in messages (`(12pt too wide)`)
/// open and close as nothing.
#[derive(Default)]
struct FileStack(Vec<Option<String>>);

impl FileStack {
    fn update(&mut self, line: &str) {
        for (i, c) in line.char_indices() {
            match c {
                '(' => {
                    let rest = &line[i + 1..];
                    let end = rest.find(|c: char| c.is_whitespace() || c == ')' || c == '(').unwrap_or(rest.len());
                    let token = &rest[..end];
                    let ext = super::project::ext_lower(token);
                    let file = (token.contains('/') || token.contains('.')) && !ext.is_empty() && ext.len() <= 4;
                    self.0.push(file.then(|| token.to_string()));
                }
                ')' => {
                    self.0.pop();
                }
                _ => {}
            }
        }
    }

    /// The innermost `.tex` file open.
    fn tex(&self) -> Option<&str> {
        self.0.iter().rev().flatten().find(|f| super::project::ext_lower(f) == "tex").map(String::as_str)
    }
}

/// Keeps only what is actionable: errors, and the undefined reference and
/// citation warnings.
pub fn parse_latex_log(log: &str, main_file: &Path) -> Vec<CompileError> {
    let dir = main_file.parent().unwrap_or(Path::new("."));
    let mut out: Vec<CompileError> = Vec::new();
    let mut add = |e: CompileError| {
        if !out.contains(&e) {
            out.push(e);
        }
    };
    let lines: Vec<&str> = log.split('\n').map(|l| l.trim_end_matches(['\r', ' '])).collect();
    let mut stack = FileStack::default();
    for (at, line) in lines.iter().enumerate() {
        let line = *line;
        if line.is_empty() {
            continue;
        }
        // LuaLaTeX without luaotfload: every font error after it is this one.
        if line.contains("module 'luaotfload-main' not found") {
            add(CompileError { file: None, line: 0, message: "File `luaotfload-main.lua' not found.".into(), warning: false });
            continue;
        }
        // The file this line's messages are about, then this line's own
        // openings and closings.
        let current = stack.tex().map(|f| absolutize(dir, f)).unwrap_or_else(|| main_file.to_path_buf());
        stack.update(line);
        if let Some(m) = FILE_LINE_RE.captures(line) {
            let file = m[1].trim();
            let ext = super::project::ext_lower(file);
            if ext != "tex" && ext != "sty" && ext != "cls" {
                continue;
            }
            let msg = m[3].trim();
            if msg.starts_with("==>") {
                continue;
            }
            add(CompileError {
                file: Some(absolutize(dir, file)),
                line: m[2].parse().unwrap_or(0),
                message: with_context(msg.strip_prefix("LaTeX Error: ").unwrap_or(msg), &lines, at),
                warning: false,
            });
            continue;
        }
        if let Some(m) = BANG_RE.captures(line) {
            let msg = m[1].trim();
            if msg.starts_with("==>") {
                continue;
            }
            add(CompileError {
                file: None,
                line: 0,
                message: with_context(msg.strip_prefix("LaTeX Error: ").unwrap_or(msg), &lines, at),
                warning: false,
            });
            continue;
        }
        if let Some(m) = WARNING_LINE_RE.captures(line) {
            let msg = m[1].trim();
            if !msg.to_lowercase().contains("undefined") {
                continue;
            }
            add(CompileError {
                // "on input line N" is in the file TeX was reading.
                file: Some(current),
                line: m.get(2).and_then(|n| n.as_str().parse().ok()).unwrap_or(0),
                message: msg.to_string(),
                warning: true,
            });
        }
    }
    out
}

/// A short version of a common LaTeX message, in the interface's language:
/// what the editor shows next to the line. `None` keeps the original.
pub fn summary(message: &str) -> Option<String> {
    static QUOTED: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[`'‘]([^'’]+)['’]").unwrap());
    static SIZE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\(([0-9.]+pt) too (wide|high)\)").unwrap());
    let quoted = || QUOTED.captures(message).map(|c| c[1].to_string()).unwrap_or_default();
    let m = message.to_lowercase();
    Some(if m.contains("citation") && m.contains("undefined") {
        tf!("Citation “{key}” is undefined", key = quoted())
    } else if m.contains("reference") && m.contains("undefined") && !m.contains("there were") {
        tf!("Reference “{label}” is undefined", label = quoted())
    } else if m.contains("there were undefined") {
        t("Some references are undefined").into()
    } else if m.contains("multiply defined") {
        tf!("Label “{label}” is defined more than once", label = quoted())
    } else if m.starts_with("overfull") {
        let size = SIZE.captures(message).map(|c| c[1].to_string()).unwrap_or_default();
        tf!("Line sticks out of the margin by {size}", size)
    } else if m.starts_with("underfull") {
        t("Loose line: too much space between words").into()
    } else if let Some(file) = missing_file(message) {
        tf!("Missing file: {file}", file)
    } else if m.contains("undefined control sequence") {
        t("Unknown command").into()
    } else if m.contains("missing $ inserted") {
        t("Math symbol outside $…$").into()
    } else if m.contains("environment") && m.contains("undefined") {
        tf!("Unknown environment “{name}”", name = quoted())
    } else {
        return None;
    })
}

/// A plain-language explanation of the common LaTeX errors, and what to do.
pub fn hint(message: &str) -> Option<&'static str> {
    let m = message.to_lowercase();
    let has = |s: &str| m.contains(s);
    Some(if has("undefined control sequence") {
        t("Unknown command: check the spelling, or load the package that defines it with \\usepackage{…}.")
    } else if has("missing $ inserted") {
        t("A math symbol (_, ^, \\alpha…) appeared outside $…$. Wrap the expression in $…$, or write \\_ for a literal underscore.")
    } else if has("environment") && has("undefined") {
        t("Unknown environment: check the spelling, or load the package that defines it.")
    } else if has("not found") && (has(".sty") || has(".cls")) {
        t("Package not found: check its name, or install it in your LaTeX distribution (tlmgr install … / the MiKTeX console).")
    } else if has("not found") {
        t("File not found: check the path, relative to the main file's folder.")
    } else if has("ended by \\end") {
        t("A \\begin{…} was closed by the \\end of another environment: check the order of the \\end.")
    } else if has("missing } inserted") || has("extra }") || has("missing { inserted") {
        t("Unbalanced braces: check that every { has its }.")
    } else if has("runaway argument") || has("paragraph ended before") || has("file ended while scanning") {
        t("An argument was not closed: a } is probably missing before the end of the paragraph.")
    } else if has("misplaced alignment tab") {
        t("& outside a table or align. For the symbol itself, write \\&.")
    } else if has("no line here to end") {
        t("\\\\ with no line to end. To start a new paragraph, leave a blank line.")
    } else if has("can be used only in preamble") {
        t("This command can only go in the preamble, before \\begin{document}.")
    } else if has("missing \\begin{document}") {
        t("There is text in the preamble: everything printed goes after \\begin{document}.")
    } else if has("citation") && has("undefined") {
        t("A citation key that is in no .bib of the project. Check the key.")
    } else if has("reference") && has("undefined") {
        t("Unknown label. Check the matching \\label (it shows as \"??\" in the PDF).")
    } else if has("there were undefined") {
        t("Some references or citations have no target; they show as \"??\" in the PDF.")
    } else if has("emergency stop") || has("fatal error") {
        t("LaTeX stopped at the first serious error: fix the error listed before this one.")
    } else if has("double superscript") || has("double subscript") {
        t("Two ^ or two _ in a row: group them with braces, as in x^{a^b}.")
    } else if has("illegal unit of measure") {
        t("A length is missing its unit (pt, cm, em, \\linewidth…).")
    } else {
        return None;
    })
}

fn absolutize(dir: &Path, file: &str) -> PathBuf {
    let p = Path::new(file);
    if p.is_absolute() {
        super::project::clean(p)
    } else {
        super::project::clean(&dir.join(p))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn programs() {
        let all = |_: &str| true;
        let only_lua = |b: &str| b == "lualatex";
        assert_eq!(program_for("\\documentclass{article}", all), Program::PdfLatex);
        assert_eq!(program_for("% !TEX program = xelatex\n\\documentclass{article}", all), Program::XeLatex);
        assert_eq!(program_for("%!TeX TS-program = LuaLaTeX\n", all), Program::LuaLatex);
        let unicode = "\\documentclass{article}\n\\usepackage{fontspec}\n\\begin{document}\\end{document}";
        assert_eq!(program_for(unicode, all), Program::XeLatex);
        assert_eq!(program_for(unicode, only_lua), Program::LuaLatex);
        assert_eq!(program_for("% \\usepackage{fontspec}\n\\begin{document}", all), Program::PdfLatex, "a commented package is not loaded");
        assert_eq!(program_for("% !TEX program = pdflatex\n\\usepackage{fontspec}", all), Program::PdfLatex, "the magic line wins");
    }

    /// Compiles with the engine the document asks for, where it is installed;
    /// where it is not, says how to install it.
    #[test]
    fn compiles_with_the_requested_engine() {
        for (program, binary) in [(Program::LuaLatex, "lualatex"), (Program::XeLatex, "xelatex")] {
            let dir = tempfile::tempdir().unwrap();
            let main = dir.path().join("main.tex");
            let src = format!("% !TEX program = {binary}\n\\documentclass{{article}}\n\\usepackage{{fontspec}}\n\\begin{{document}}\nOlá, ção.\n\\end{{document}}\n");
            fs::write(&main, src).unwrap();
            assert_eq!(program_for(&fs::read_to_string(&main).unwrap(), |_| true), program);
            let result = compile_system(Arc::new(false.into()), &main);
            if engine_ready(binary) {
                let r = result.unwrap();
                assert!(r.success, "{binary}: {:?}\n{}", r.errors, &r.log_text[r.log_text.len().saturating_sub(800)..]);
                assert!(r.engine.contains(binary), "{}", r.engine);
            } else if which("pdflatex") || which("latexmk") {
                let e = result.unwrap_err();
                assert!(e.starts_with(&format!("{binary} is not installed")), "{e}");
            }
        }
    }

    #[test]
    fn summaries() {
        assert_eq!(summary("Citation `pastor2001' on page 1 undefined on input line 14."), Some("Citation “pastor2001” is undefined".into()));
        assert_eq!(summary("Reference `fig:rede' on page 1 undefined"), Some("Reference “fig:rede” is undefined".into()));
        assert_eq!(summary("Overfull \\hbox (12.3pt too wide) in paragraph at lines 3--4"), Some("Line sticks out of the margin by 12.3pt".into()));
        assert_eq!(summary("LaTeX Error: File `foo.sty' not found."), Some("Missing file: foo.sty".into()));
        assert_eq!(summary("Something exotic"), None);
    }

    #[test]
    fn missing_files() {
        assert_eq!(missing_file("LaTeX Error: File `abntex2.cls' not found."), Some("abntex2.cls".into()));
        assert_eq!(missing_file("Package babel Error: Unknown option 'brazil'. Either you misspelled it"), Some("brazil.ldf".into()));
        assert_eq!(missing_file("Undefined control sequence."), None);
        let log = "[\\directlua]:1: module 'luaotfload-main' not found:\n\tno field package.preload['luaotfload-main']";
        let errors = parse_latex_log(log, Path::new("/p/main.tex"));
        assert_eq!(errors[0].message, "File `luaotfload-main.lua' not found.");
        assert_eq!(missing_file(&errors[0].message), Some("luaotfload-main.lua".into()));
        let font = "Font \\TU/lmr/m/n/10=[lmroman10-regular]:+tlig; at 10pt not loadable: metric data not found or bad.";
        assert_eq!(missing_file(font), Some("lmroman10-regular.otf".into()));
        assert_eq!(summary(font), Some("Missing file: lmroman10-regular.otf".into()));
    }

    #[test]
    fn parses_log() {
        let log = "./cap/um.tex:12: Undefined control sequence.\n\
                   ./cap/um.tex:12: Undefined control sequence.\n\
                   ! LaTeX Error: File `foo.sty' not found.\n\
                   ! ==> Fatal error occurred, no output PDF file produced!\n\
                   LaTeX Warning: Reference `fig:x' on page 1 undefined on input line 42.\n\
                   LaTeX Warning: Something harmless.\n\
                   foo.log:3: ignored\n";
        let main = Path::new("/p/main.tex");
        let errs = parse_latex_log(log, main);
        assert_eq!(errs.len(), 3);
        assert_eq!(errs[0].file.as_deref(), Some(Path::new("/p/cap/um.tex")));
        assert_eq!(errs[0].line, 12);
        assert_eq!(errs[1].message, "File `foo.sty' not found.");
        assert!(errs[2].warning);
        assert_eq!(errs[2].line, 42);
    }

    #[test]
    fn rerun_detection() {
        assert!(needs_another_pass("LaTeX Warning: Label(s) may have changed. Rerun to get cross-references right."));
        assert!(!needs_another_pass("Output written on main.pdf"));
    }

    #[test]
    fn parse_log_captures_file_and_line() {
        let log = "This is pdfTeX\n(./main.tex\n./main.tex:12: Undefined control sequence.\nl.12 \\doesnotexist\n\
./cap/one.tex:3: LaTeX Error: Environment foo undefined.\n! LaTeX Error: File `missing.sty' not found.\n\
! ==> Fatal error occurred, no output PDF file produced!\n\
LaTeX Warning: Reference `fig:x' on page 1 undefined on input line 42.\nLaTeX Warning: There were undefined references.\n\
Overfull \\hbox (5.0pt too wide) in paragraph at lines 7--9\n";
        let errs = parse_latex_log(log, Path::new("/proj/main.tex"));
        let e = &errs[0];
        assert_eq!(e.file.as_deref(), Some(Path::new("/proj/main.tex")));
        assert_eq!((e.line, e.message.as_str(), e.warning), (12, "Undefined control sequence: \\doesnotexist", false));
        assert_eq!(errs[1].file.as_deref(), Some(Path::new("/proj/cap/one.tex")));
        assert_eq!(errs[1].line, 3);
        assert!(!errs[1].message.starts_with("LaTeX Error:"), "prefix not stripped: {}", errs[1].message);
        assert!(errs.iter().any(|e| e.message.contains("missing.sty")));
        let reference = errs.iter().find(|e| e.message.contains("fig:x")).expect("undefined reference warning");
        assert!(reference.warning);
        assert_eq!(reference.line, 42);
        assert!(!errs.iter().any(|e| e.message.contains("Overfull")), "typographic noise");
        assert!(!errs.iter().any(|e| e.message.starts_with("==>")));
        let errs = parse_latex_log("./main.tex:5: ==> Fatal error occurred, no output PDF file produced!\n", Path::new("/p/main.tex"));
        assert!(errs.is_empty(), "{errs:?}");
    }

    #[test]
    fn warnings_land_in_the_file_being_read() {
        let log = "(./main.tex (/usr/share/texlive/texmf-dist/tex/latex/base/report.cls\n\
Document Class: report 2023/05/17 v1.4n Standard LaTeX document class\n\
(/usr/share/texlive/texmf-dist/tex/latex/base/size12.clo))\n\
(./capitulos/introducao.tex\n\
Chapter 1.\n\
LaTeX Warning: Citation `pastor2001' on page 1 undefined on input line 3.\n\
Overfull \\hbox (12.3pt too wide) in paragraph at lines 5--6\n\
LaTeX Warning: Reference `fig:x' on page 2 undefined on input line 7.\n\
) (./capitulos/metodo.tex\n\
Chapter 2.\n\
)\n\
LaTeX Warning: Citation `k' on page 3 undefined on input line 20.\n\
[3] (./main.aux) )\n";
        let main = Path::new("/p/main.tex");
        let got: Vec<(String, usize)> = parse_latex_log(log, main)
            .into_iter()
            .filter(|e| e.warning)
            .map(|e| (e.file.unwrap().display().to_string(), e.line))
            .collect();
        assert_eq!(
            got,
            vec![("/p/capitulos/introducao.tex".into(), 3), ("/p/capitulos/introducao.tex".into(), 7), ("/p/main.tex".into(), 20)]
        );
    }

    #[test]
    fn parse_log_does_not_repeat_across_passes() {
        let errs = parse_latex_log(&"./main.tex:5: Undefined control sequence.\n".repeat(4), Path::new("/proj/main.tex"));
        assert_eq!(errs.len(), 1);
    }

    #[test]
    fn needs_another_pass_cases() {
        for log in [
            "LaTeX Warning: Label(s) may have changed. Rerun to get cross-references right.",
            "There were undefined references.",
            "There were undefined citations.",
        ] {
            assert!(needs_another_pass(log), "{log}");
        }
    }

    /// A single pdflatex pass renders "??" where \ref goes: the multi-pass
    /// has to fix that.
    #[test]
    fn compile_resolves_cross_references() {
        if !which("pdflatex") {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let main = dir.path().join("main.tex");
        fs::write(
            &main,
            "\\documentclass{article}\n\\begin{document}\n\\section{Primeira}\\label{sec:um}\n\\newpage\n\\section{Segunda}\n\
             Veja a secao \\ref{sec:um} na pagina \\pageref{sec:um}.\n\\end{document}\n",
        )
        .unwrap();
        let r = compile_system(Arc::new(AtomicBool::new(false)), &main).unwrap();
        assert!(r.success, "{}", r.log_text);
        if r.engine == "pdflatex" {
            assert!(r.passes >= 2, "plain pdflatex needs 2+ passes, ran {}", r.passes);
        }
        let pdf = r.pdf_path.unwrap();
        let pages = crate::backend::pdf::info(&pdf).unwrap().len();
        let text: Vec<String> = (1..=pages).flat_map(|p| crate::backend::pdf::words(&pdf, p).unwrap()).map(|w| w.text).collect();
        let text = text.join(" ");
        assert!(!text.contains("??"), "unresolved references:\n{text}");
        assert!(text.contains("secao 1"), "{text}");
        assert!(!foreign_pdf(&main), "a PDF this app compiled is not foreign");
    }

    #[test]
    fn compile_honours_cancellation() {
        if !which("pdflatex") {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let main = dir.path().join("main.tex");
        fs::write(&main, "\\documentclass{article}\\begin{document}oi\\end{document}\n").unwrap();
        let r = compile_system(Arc::new(AtomicBool::new(true)), &main).unwrap();
        assert!(r.canceled && !r.success);
    }

    #[test]
    fn inspect_pdf_staleness() {
        let dir = tempfile::tempdir().unwrap();
        let main = dir.path().join("main.tex");
        let pdf = dir.path().join("main.pdf");
        fs::write(&main, "\\documentclass{article}").unwrap();
        let set_mtime = |p: &Path, t: std::time::SystemTime| fs::File::options().write(true).open(p).unwrap().set_modified(t).unwrap();
        let now = std::time::SystemTime::now();

        // No PDF yet: stale, compiling is the only way to a preview.
        let st = inspect_pdf(&main);
        assert!(st.path == pdf && !st.exists && st.stale);

        fs::write(&pdf, "%PDF-1.5").unwrap();
        set_mtime(&main, now - Duration::from_secs(3600));
        let st = inspect_pdf(&main);
        assert!(st.exists && !st.stale, "a PDF newer than the source is current");

        set_mtime(&main, now + Duration::from_secs(3600));
        let st = inspect_pdf(&main);
        assert!(st.exists && st.stale, "editing the source leaves it behind");
    }

    #[test]
    fn foreign_pdf_needs_an_aux() {
        let dir = tempfile::tempdir().unwrap();
        let main = dir.path().join("main.tex");
        fs::write(&main, "\\documentclass{article}").unwrap();
        assert!(!foreign_pdf(&main), "no PDF, nothing to protect");
        fs::write(dir.path().join("main.pdf"), "%PDF-1.5").unwrap();
        assert!(foreign_pdf(&main), "a PDF with no .aux was never compiled here");
        fs::write(dir.path().join("main.aux"), "\\relax").unwrap();
        assert!(!foreign_pdf(&main));
    }

    #[test]
    fn undefined_command_gets_its_name_and_hints() {
        let log = "./main.tex:35: Undefined control sequence.\nl.35 Texto com erro \\erro\n                        \n";
        let errs = parse_latex_log(log, Path::new("/p/main.tex"));
        assert_eq!(errs[0].message, "Undefined control sequence: \\erro");
        assert!(hint(&errs[0].message).unwrap().contains("Unknown command"));
        assert!(hint("Missing $ inserted.").is_some());
        assert!(hint("File `foo.sty' not found.").unwrap().contains("Package"));
        assert!(hint("Environment foo undefined.").unwrap().contains("Unknown environment"));
        assert_eq!(hint("Something exotic"), None);
    }
}
