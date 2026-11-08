//! Syntax highlighting for the editor: a small hand-written LaTeX
//! lexer (and a rough Markdown one for notes) that turn the buffer into a
//! `LayoutJob`, underlining the prose words the spell checker rejects.
//!
//! Not a TeX parser, with the same limits the old CodeMirror `stex` mode had:
//! no verbatim, no catcodes.

use crate::backend::document::Language;
use crate::spell::SpellCache;
use regex::Regex;
use std::sync::LazyLock;
use egui::text::{LayoutJob, TextFormat};
use egui::{Color32, FontId, Stroke};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Lang {
    Tex,
    Markdown,
    Plain,
}

impl Lang {
    pub fn for_path(path: &std::path::Path) -> Lang {
        match crate::backend::project::ext_lower(path).as_str() {
            "tex" | "sty" | "cls" | "bib" => Lang::Tex,
            "md" => Lang::Markdown,
            _ => Lang::Plain,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Tok {
    Text,
    Command,
    Comment,
    Math,
    Bracket,
    Env,
    Arg,
    Heading,
    Word,
    Special,
}

pub struct Palette {
    text: Color32,
    command: Color32,
    comment: Color32,
    math: Color32,
    bracket: Color32,
    env: Color32,
    arg: Color32,
    heading: Color32,
    special: Color32,
    misspelled: Color32,
    /// The writing reviewer's underline.
    suggestion: Color32,
}

/// The prose leads: markup (commands, braces, keys) is quieter than the text
/// it wraps, and titles are the brightest thing after it.
impl Palette {
    pub fn new(dark: bool) -> Self {
        if dark {
            Palette {
                text: Color32::from_rgb(0xE6, 0xE1, 0xDA),
                command: Color32::from_rgb(0xC9, 0x8C, 0x74),
                comment: Color32::from_rgb(0x8C, 0x86, 0x7F),
                math: Color32::from_rgb(0x93, 0xC2, 0xA2),
                bracket: Color32::from_rgb(0x85, 0x7C, 0x73),
                env: Color32::from_rgb(0xB1, 0x96, 0xC9),
                arg: Color32::from_rgb(0x8F, 0xAF, 0xC9),
                heading: Color32::from_rgb(0xF6, 0xC6, 0x92),
                special: Color32::from_rgb(0xC9, 0xAE, 0x6A),
                misspelled: Color32::from_rgb(0xF0, 0x5A, 0x4A),
                suggestion: Color32::from_rgb(0x6F, 0xA8, 0xDC),
            }
        } else {
            Palette {
                text: Color32::from_rgb(0x26, 0x22, 0x1F),
                command: Color32::from_rgb(0x9A, 0x52, 0x38),
                comment: Color32::from_rgb(0x74, 0x6E, 0x66),
                math: Color32::from_rgb(0x1F, 0x6B, 0x42),
                bracket: Color32::from_rgb(0x8A, 0x7F, 0x73),
                env: Color32::from_rgb(0x7A, 0x4E, 0x9A),
                arg: Color32::from_rgb(0x3A, 0x65, 0x91),
                heading: Color32::from_rgb(0x8A, 0x3C, 0x08),
                special: Color32::from_rgb(0x8C, 0x64, 0x00),
                misspelled: Color32::from_rgb(0xD0, 0x20, 0x10),
                suggestion: Color32::from_rgb(0x2F, 0x6D, 0xB5),
            }
        }
    }
}

/// Commands whose first braced argument is an identifier, not prose: it is
/// neither spell checked nor coloured as text.
const NON_PROSE: &[&str] = &[
    "label", "ref", "eqref", "cref", "Cref", "autoref", "pageref", "nameref", "cite", "citep", "citet", "nocite",
    "parencite", "textcite", "autocite", "footcite", "usepackage", "RequirePackage", "documentclass", "input",
    "include", "includegraphics", "includepdf", "begin", "end", "bibliography", "bibliographystyle", "addbibresource",
    "url", "href", "newcommand", "renewcommand", "newenvironment", "setlength", "color", "textcolor", "hypersetup",
    "geometry", "pagestyle", "thispagestyle", "numberwithin", "setcounter", "graphicspath", "lstinputlisting",
    "selectlanguage", "foreignlanguage", "setmainlanguage", "setdefaultlanguage", "setotherlanguage", "setotherlanguages",
];

/// Commands whose first braced argument is a title: coloured as one, and
/// still spell checked.
const HEADINGS: &[&str] = &[
    "part", "chapter", "section", "subsection", "subsubsection", "paragraph", "subparagraph", "title", "caption",
    "frametitle",
];

/// Environments whose body is mathematics.
const MATH_ENVS: &[&str] = &[
    "equation", "equation*", "align", "align*", "gather", "gather*", "multline", "multline*", "flalign", "flalign*",
    "alignat", "alignat*", "eqnarray", "eqnarray*", "math", "displaymath", "dmath", "dmath*",
];

struct Lexer {
    spans: Vec<(usize, usize, Tok, bool)>,
}

impl Lexer {
    fn push(&mut self, start: usize, end: usize, tok: Tok, bad: bool) {
        if start >= end {
            return;
        }
        if let Some(last) = self.spans.last_mut() {
            if last.1 == start && last.2 == tok && last.3 == bad && !bad {
                last.1 = end;
                return;
            }
        }
        self.spans.push((start, end, tok, bad));
    }
}

fn is_word_char(c: char) -> bool {
    c.is_alphabetic()
}

/// Length in bytes of a prose word starting at `i` (letters, with inner
/// apostrophes and hyphens).
fn word_len(text: &str, i: usize) -> usize {
    let mut end = i;
    let mut chars = text[i..].char_indices().peekable();
    while let Some((off, c)) = chars.next() {
        if is_word_char(c) {
            end = i + off + c.len_utf8();
            continue;
        }
        if matches!(c, '\'' | '’' | '-') {
            if let Some((_, next)) = chars.peek() {
                if is_word_char(*next) {
                    continue;
                }
            }
        }
        break;
    }
    end - i
}

/// Passages written in another language than the document's: babel's
/// `otherlanguage` environments and `\\selectlanguage` switches.
/// The language of the passage `byte` is in, when it is not the document's.
pub fn language_at(text: &str, byte: usize) -> Option<Language> {
    language_regions(text).into_iter().filter(|r| r.0 <= byte && byte < r.1).max_by_key(|r| r.0).map(|r| r.2)
}

fn language_regions(text: &str) -> Vec<(usize, usize, Language)> {
    static OTHER: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\\begin\{otherlanguage\*?\}\s*\{(\w+)\}").unwrap());
    static SELECT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\\selectlanguage\s*\{(\w+)\}").unwrap());
    let language = |name: &str| match name {
        "brazil" | "brazilian" | "portuguese" | "portuges" => Some(Language::Portuguese),
        "english" | "american" | "british" | "USenglish" | "UKenglish" => Some(Language::English),
        _ => None,
    };
    let mut out = Vec::new();
    for caps in OTHER.captures_iter(text) {
        let start = caps.get(0).unwrap().end();
        let end = text[start..].find("\\end{otherlanguage").map(|e| start + e).unwrap_or(text.len());
        if let Some(l) = language(&caps[1]) {
            out.push((start, end, l));
        }
    }
    let switches: Vec<_> = SELECT.captures_iter(text).map(|c| (c.get(0).unwrap().end(), language(&c[1]))).collect();
    for (i, (start, l)) in switches.iter().enumerate() {
        let end = switches.get(i + 1).map(|n| n.0).unwrap_or(text.len());
        if let Some(l) = l {
            out.push((*start, end, *l));
        }
    }
    out
}

fn lex_tex(text: &str, spell: &mut Option<&mut SpellCache>) -> Vec<(usize, usize, Tok, bool)> {
    let mut lx = Lexer { spans: Vec::new() };
    let regions = if spell.is_some() { language_regions(text) } else { Vec::new() };
    // The innermost passage wins: an `otherlanguage` inside the range of a
    // `\selectlanguage` at the top of the document.
    let region_at = |i: usize| regions.iter().filter(|r| r.0 <= i && i < r.1).max_by_key(|r| r.0).map(|r| r.2);
    let b = text.as_bytes();
    let mut i = 0;
    let mut math: Option<&'static str> = None; // closing delimiter
    let mut arg_depth = 0usize; // >0 while inside a non-prose argument
    let mut expect_arg = false; // a NON_PROSE command wants its argument next
    let mut env_next = false; // \begin/\end: the argument is an environment name
    let mut begin_next = false; // …and it is a \begin
    let mut env_name = String::new();
    let mut math_env: Option<String> = None; // inside \begin{equation}…
    let mut heading_next = false; // a sectioning command wants its title next
    let mut heading_depth = 0usize;
    // Inside a command's `[…]` options (`[12pt,a4paper]`, `[htbp]`): keys,
    // not prose, neither spell checked nor read by the reviewer.
    let mut opt_depth = 0usize;
    let mut word_since_command = true;

    while i < b.len() {
        let c = b[i];
        // Comments end at the newline, whatever mode we are in.
        if c == b'%' {
            let end = text[i..].find('\n').map(|n| i + n).unwrap_or(b.len());
            lx.push(i, end, Tok::Comment, false);
            i = end;
            continue;
        }
        if c == b'\\' {
            let start = i;
            i += 1;
            if i < b.len() && b[i].is_ascii_alphabetic() {
                while i < b.len() && b[i].is_ascii_alphabetic() {
                    i += 1;
                }
                if i < b.len() && b[i] == b'*' {
                    i += 1;
                }
                let name = text[start + 1..i].trim_end_matches('*');
                // \end{…} of the math environment we are in closes it.
                if let Some(env) = &math_env {
                    if name == "end" && text[i..].starts_with(&format!("{{{env}}}")) {
                        math_env = None;
                    }
                }
                let in_math = math.is_some() || math_env.is_some();
                let tok = if in_math { Tok::Math } else { Tok::Command };
                lx.push(start, i, tok, false);
                if !in_math {
                    word_since_command = false;
                }
                if !in_math && arg_depth == 0 {
                    let cite_like = name.contains("cite");
                    if NON_PROSE.contains(&name) || cite_like {
                        expect_arg = true;
                        env_next = name == "begin" || name == "end";
                        begin_next = name == "begin";
                        env_name.clear();
                    } else if HEADINGS.contains(&name) && heading_depth == 0 {
                        heading_next = true;
                    }
                }
            } else if i < b.len() {
                let ch_len = text[i..].chars().next().map(|c| c.len_utf8()).unwrap_or(1);
                let next = b[i];
                i += ch_len;
                match (next, math) {
                    (b'[', None) => {
                        math = Some("\\]");
                        lx.push(start, i, Tok::Math, false);
                    }
                    (b'(', None) => {
                        math = Some("\\)");
                        lx.push(start, i, Tok::Math, false);
                    }
                    (b']', Some("\\]")) | (b')', Some("\\)")) => {
                        math = None;
                        lx.push(start, i, Tok::Math, false);
                    }
                    (_, Some(_)) => lx.push(start, i, Tok::Math, false),
                    _ => lx.push(start, i, Tok::Command, false),
                }
            }
            continue;
        }
        if c == b'$' && math_env.is_none() {
            let double = i + 1 < b.len() && b[i + 1] == b'$';
            let len = if double { 2 } else { 1 };
            match math {
                None => math = Some(if double { "$$" } else { "$" }),
                Some("$$") if double => math = None,
                Some("$") if !double => math = None,
                _ => {}
            }
            lx.push(i, i + len, Tok::Math, false);
            i += len;
            continue;
        }
        if math.is_some() || math_env.is_some() {
            let len = text[i..].chars().next().map(|c| c.len_utf8()).unwrap_or(1);
            lx.push(i, i + len, Tok::Math, false);
            i += len;
            continue;
        }
        match c {
            b'{' => {
                lx.push(i, i + 1, Tok::Bracket, false);
                if arg_depth > 0 {
                    arg_depth += 1;
                } else if expect_arg {
                    arg_depth = 1;
                    expect_arg = false;
                } else if heading_depth > 0 {
                    heading_depth += 1;
                } else if heading_next {
                    heading_depth = 1;
                    heading_next = false;
                }
                i += 1;
                continue;
            }
            b'}' => {
                lx.push(i, i + 1, Tok::Bracket, false);
                if arg_depth > 0 {
                    arg_depth -= 1;
                    if arg_depth == 0 {
                        if env_next && begin_next && MATH_ENVS.contains(&env_name.as_str()) {
                            math_env = Some(env_name.clone());
                        }
                        // `\begin{otherlanguage}{english}`: the language is no prose.
                        if env_next && begin_next && env_name.starts_with("otherlanguage") {
                            expect_arg = true;
                        }
                        env_next = false;
                        begin_next = false;
                    }
                } else {
                    heading_depth = heading_depth.saturating_sub(1);
                }
                i += 1;
                continue;
            }
            b'&' | b'~' => {
                lx.push(i, i + 1, Tok::Special, false);
                i += 1;
                continue;
            }
            b'[' => {
                lx.push(i, i + 1, Tok::Bracket, false);
                let prev = text[..i].trim_end().chars().next_back();
                if opt_depth > 0 {
                    opt_depth += 1;
                } else if !word_since_command && prev.is_some_and(|c| c.is_ascii_alphabetic() || matches!(c, '}' | '*' | ']')) {
                    opt_depth = 1;
                } else if text[..i].ends_with("\\\\") {
                    opt_depth = 1; // A line break's space: `\\[4pt]`.
                }
                i += 1;
                continue;
            }
            b']' => {
                lx.push(i, i + 1, Tok::Bracket, false);
                opt_depth = opt_depth.saturating_sub(1);
                i += 1;
                continue;
            }
            _ => {}
        }
        let ch = text[i..].chars().next().unwrap();
        if is_word_char(ch) {
            let len = word_len(text, i);
            let word = &text[i..i + len];
            if arg_depth > 0 {
                if env_next {
                    env_name.push_str(word);
                }
                lx.push(i, i + len, if env_next { Tok::Env } else { Tok::Arg }, false);
            } else if opt_depth > 0 {
                lx.push(i, i + len, Tok::Arg, false);
            } else if heading_depth > 0 {
                let bad = spell.as_mut().map(|s| s.is_misspelled_in(word, region_at(i))).unwrap_or(false);
                lx.push(i, i + len, Tok::Heading, bad);
            } else {
                word_since_command = true;
                let bad = spell.as_mut().map(|s| s.is_misspelled_in(word, region_at(i))).unwrap_or(false);
                lx.push(i, i + len, Tok::Word, bad);
            }
            i += len;
            continue;
        }
        // Anything else between a command and its argument (spaces, `[opt]`
        // content) keeps the expectation; a newline or text ends it.
        if expect_arg && !(ch == ' ' || ch == '[' || ch == ']' || ch == '=' || ch == ',') {
            if ch == '\n' {
                expect_arg = false;
            }
        }
        if arg_depth > 0 && env_next && ch == '*' {
            env_name.push('*');
        }
        let tok = if arg_depth > 0 {
            if env_next { Tok::Env } else { Tok::Arg }
        } else if opt_depth > 0 {
            Tok::Arg
        } else if heading_depth > 0 {
            Tok::Heading
        } else {
            Tok::Text
        };
        lx.push(i, i + ch.len_utf8(), tok, false);
        i += ch.len_utf8();
    }
    lx.spans
}

fn lex_markdown(text: &str, spell: &mut Option<&mut SpellCache>) -> Vec<(usize, usize, Tok, bool)> {
    let mut lx = Lexer { spans: Vec::new() };
    let mut offset = 0;
    for line in text.split_inclusive('\n') {
        let trimmed = line.trim_start();
        let heading = trimmed.starts_with('#');
        let mut i = 0;
        let lb = line.as_bytes();
        if trimmed.starts_with("- [") || trimmed.starts_with("* [") {
            let lead = line.len() - trimmed.len();
            let end = (lead + 5).min(line.len());
            lx.push(offset, offset + end, Tok::Command, false);
            i = end;
        }
        while i < lb.len() {
            let ch = line[i..].chars().next().unwrap();
            if is_word_char(ch) {
                let len = word_len(line, i);
                let bad = spell.as_mut().map(|s| s.is_misspelled(&line[i..i + len])).unwrap_or(false);
                lx.push(offset + i, offset + i + len, if heading { Tok::Heading } else { Tok::Word }, bad);
                i += len;
            } else {
                let tok = if heading { Tok::Heading } else if ch == '`' { Tok::Arg } else { Tok::Text };
                lx.push(offset + i, offset + i + ch.len_utf8(), tok, false);
                i += ch.len_utf8();
            }
        }
        offset += line.len();
    }
    lx.spans
}

/// Builds the layout job for the whole buffer.
/// What a stretch of a LaTeX source is, for the writing reviewer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Part {
    /// Prose: words, the spaces and punctuation between them, titles, `~`.
    Prose,
    /// A command name (`\\ref`) or a control symbol.
    Command,
    Math,
    Comment,
    /// Braces, brackets and the identifiers inside them (labels, keys, paths).
    Markup,
}

/// The stretches of a LaTeX source, in order, as the highlighter sees them.
pub fn parts(text: &str) -> Vec<(usize, usize, Part)> {
    let mut out: Vec<(usize, usize, Part)> = Vec::new();
    let mut covered = 0;
    let push = |out: &mut Vec<(usize, usize, Part)>, start: usize, end: usize, part: Part| {
        if let Some(last) = out.last_mut().filter(|l| l.1 == start && l.2 == part) {
            last.1 = end;
        } else if start < end {
            out.push((start, end, part));
        }
    };
    for (start, end, tok, _) in lex_tex(text, &mut None) {
        if start > covered {
            push(&mut out, covered, start, Part::Prose);
        }
        let part = match tok {
            Tok::Text | Tok::Word | Tok::Heading | Tok::Special => Part::Prose,
            Tok::Command => Part::Command,
            Tok::Math => Part::Math,
            Tok::Comment => Part::Comment,
            Tok::Bracket | Tok::Arg | Tok::Env => Part::Markup,
        };
        push(&mut out, start, end, part);
        covered = end;
    }
    if covered < text.len() {
        push(&mut out, covered, text.len(), Part::Prose);
    }
    out
}

/// Where each prose word of a LaTeX source starts (bytes), in order: the
/// words a reader sees, not commands, keys, math or comments.
pub fn prose_word_starts(text: &str) -> Vec<usize> {
    let mut out = Vec::new();
    for (start, end, part) in parts(text) {
        if part != Part::Prose {
            continue;
        }
        let mut in_word = false;
        for (i, c) in text[start..end].char_indices() {
            let word = c.is_alphanumeric();
            if word && !in_word {
                out.push(start + i);
            }
            in_word = word || (in_word && (c == '\'' || c == '-'));
        }
    }
    out
}

/// The body of a main file (between `\begin{document}` and
/// `\end{document}`), or all of it for a file without them.
pub fn body(text: &str) -> std::ops::Range<usize> {
    let start = text.find("\\begin{document}").map(|i| i + "\\begin{document}".len()).unwrap_or(0);
    let end = text[start..].find("\\end{document}").map(|i| start + i).unwrap_or(text.len());
    start..end
}

/// Prose words of a LaTeX source (its body, when it has a preamble).
pub fn prose_words(text: &str) -> usize {
    let body = body(text);
    prose_word_starts(text).into_iter().filter(|w| body.contains(w)).count()
}

/// Line height, relative to the font size: room for the eye between wrapped
/// lines of prose.
pub const LINE_SPACING: f32 = 1.5;

/// `marks`: the writing reviewer's ranges (bytes), `true` for the minor ones.
pub fn layout_job(
    text: &str,
    lang: Lang,
    font: FontId,
    palette: &Palette,
    mut spell: Option<&mut SpellCache>,
    wrap_width: f32,
    marks: &[(std::ops::Range<usize>, bool)],
) -> LayoutJob {
    let spans = match lang {
        Lang::Tex => lex_tex(text, &mut spell),
        Lang::Markdown => lex_markdown(text, &mut spell),
        Lang::Plain => vec![(0, text.len(), Tok::Text, false)],
    };
    let mut job = LayoutJob { text: text.to_string(), ..Default::default() };
    job.wrap.max_width = wrap_width;
    let mut covered = 0;
    let add = |job: &mut LayoutJob, start: usize, end: usize, tok: Tok, bad: bool| {
        let color = match tok {
            Tok::Text | Tok::Word => palette.text,
            Tok::Command => palette.command,
            Tok::Comment => palette.comment,
            Tok::Math => palette.math,
            Tok::Bracket => palette.bracket,
            Tok::Env => palette.env,
            Tok::Arg => palette.arg,
            Tok::Heading => palette.heading,
            Tok::Special => palette.special,
        };
        let mut format = TextFormat { font_id: font.clone(), color, line_height: Some((font.size * LINE_SPACING).round()), ..Default::default() };
        if tok == Tok::Comment {
            format.italics = true;
        }
        if bad {
            format.underline = Stroke::new(1.5, palette.misspelled);
        }
        // Neighbours that look the same share one section: words and the
        // spaces between them are different tokens but one colour, and
        // every section costs egui's layout time on each keystroke.
        if let Some(last) = job.sections.last_mut() {
            if last.byte_range.end.0 == start && last.format == format {
                last.byte_range.end = egui::text::ByteIndex(end);
                return;
            }
        }
        job.sections.push(egui::text::LayoutSection { leading_space: 0.0, byte_range: egui::text::ByteIndex(start)..egui::text::ByteIndex(end), format });
    };
    for (start, end, tok, bad) in spans {
        if start > covered {
            add(&mut job, covered, start, Tok::Text, false);
        }
        add(&mut job, start, end, tok, bad);
        covered = end;
    }
    if covered < text.len() || text.is_empty() {
        add(&mut job, covered, text.len(), Tok::Text, false);
    }
    underline(&mut job, marks, palette.suggestion);
    job
}

/// Merges ranges into sorted, disjoint ones.
fn union(mut ranges: Vec<std::ops::Range<usize>>) -> Vec<std::ops::Range<usize>> {
    ranges.sort_by_key(|r| r.start);
    let mut out: Vec<std::ops::Range<usize>> = Vec::new();
    for r in ranges {
        match out.last_mut() {
            Some(last) if r.start <= last.end => last.end = last.end.max(r.end),
            _ => out.push(r),
        }
    }
    out
}

/// Underlines the reviewer's ranges, splitting sections at their ends. A
/// misspelling keeps its own underline.
fn underline(job: &mut LayoutJob, marks: &[(std::ops::Range<usize>, bool)], color: Color32) {
    if marks.is_empty() {
        return;
    }
    let major = union(marks.iter().filter(|m| !m.1).map(|m| m.0.clone()).collect());
    let minor = union(marks.iter().filter(|m| m.1).map(|m| m.0.clone()).collect());
    let covers = |set: &[std::ops::Range<usize>], at: usize| {
        let i = set.partition_point(|r| r.start <= at);
        i > 0 && set[i - 1].end > at
    };
    let mut cuts: Vec<usize> = major.iter().chain(minor.iter()).flat_map(|r| [r.start, r.end]).collect();
    cuts.sort_unstable();
    cuts.dedup();
    let strong = Stroke::new(1.5, color);
    let faint = Stroke::new(1.0, color.gamma_multiply(0.45));
    let mut out = Vec::with_capacity(job.sections.len() + cuts.len());
    for section in job.sections.drain(..) {
        let (start, end) = (section.byte_range.start.0, section.byte_range.end.0);
        let first = cuts.partition_point(|&c| c <= start);
        let mut from = start;
        for &cut in cuts[first..].iter().take_while(|&&c| c < end).chain(std::iter::once(&end)) {
            let mut piece = section.clone();
            piece.byte_range = egui::text::ByteIndex(from)..egui::text::ByteIndex(cut);
            if piece.format.underline == Stroke::NONE {
                if covers(&major, from) {
                    piece.format.underline = strong;
                } else if covers(&minor, from) {
                    piece.format.underline = faint;
                }
            }
            out.push(piece);
            from = cut;
        }
    }
    job.sections = out;
}

/// The prose word (byte range) covering `byte`, if any, for the spelling
/// suggestions menu.
pub fn word_at(text: &str, byte: usize) -> Option<(usize, usize)> {
    if byte > text.len() || !text.is_char_boundary(byte) {
        return None;
    }
    let mut start = byte;
    while start > 0 {
        let prev = text[..start].chars().next_back().unwrap();
        if is_word_char(prev) || matches!(prev, '\'' | '’' | '-') {
            start -= prev.len_utf8();
        } else {
            break;
        }
    }
    while start < text.len() && !text[start..].starts_with(is_word_char) {
        start += text[start..].chars().next().unwrap().len_utf8();
    }
    if start >= text.len() || start > byte {
        return None;
    }
    let len = word_len(text, start);
    (len > 0 && start + len >= byte).then_some((start, start + len))
}

#[cfg(test)]
mod tests {
    #[test]
    fn options_are_not_prose() {
        let src = "\\documentclass[12pt,a4paper]{report}\n\\begin{figure}[htbp]\nTexto [1] aqui \\cite[p.~3]{k}.\\\\[4pt]\nFim.";
        let prose: Vec<&str> = super::prose_word_starts(src).into_iter().map(|w| &src[w..w + src[w..].find(|c: char| !c.is_alphanumeric()).unwrap_or(0)]).collect();
        assert_eq!(prose, vec!["Texto", "1", "aqui", "Fim"], "options of \\documentclass, figure, \\cite and \\\\ are keys");
    }

    #[test]
    fn language_passages() {
        let src = "Texto.\n\\begin{otherlanguage*}{english}\nThe abstract.\n\\end{otherlanguage*}\n\\selectlanguage{english}\nMore.";
        let regions = super::language_regions(src);
        let at = |w: &str| src.find(w).unwrap();
        let lang = |w: &str| regions.iter().rev().find(|r| r.0 <= at(w) && at(w) < r.1).map(|r| r.2);
        assert_eq!(lang("Texto"), None);
        assert_eq!(lang("abstract"), Some(super::Language::English));
        assert_eq!(lang("More"), Some(super::Language::English));
    }

    #[test]
    fn the_innermost_language_wins() {
        // abnTeX2's template: Portuguese from the top, an English abstract inside.
        let src = "\\begin{document}\n\\selectlanguage{brazil}\nTexto.\n\\begin{otherlanguage*}{english}\nThe same summary.\n\\end{otherlanguage*}\nFim.";
        let mut spell = crate::spell::SpellCache::loaded_for_tests();
        let flagged: Vec<&str> = super::lex_tex(src, &mut Some(&mut spell)).into_iter().filter(|s| s.3).map(|s| &src[s.0..s.1]).collect();
        assert!(flagged.is_empty(), "flagged: {flagged:?}");
    }

    use super::*;

    fn toks(src: &str) -> Vec<(String, Tok)> {
        lex_tex(src, &mut None).into_iter().map(|(s, e, t, _)| (src[s..e].to_string(), t)).collect()
    }

    #[test]
    fn lexes_tex() {
        let t = toks("\\section{Olá} $x^2$ % nota\n\\label{sec:a}");
        assert!(t.contains(&("\\section".into(), Tok::Command)));
        assert!(t.contains(&("Olá".into(), Tok::Heading)));
        assert!(t.contains(&("% nota".into(), Tok::Comment)));
        assert!(t.iter().any(|(s, k)| s.contains("sec") && *k == Tok::Arg));
        assert!(t.iter().any(|(s, k)| s.contains('x') && *k == Tok::Math));
    }

    #[test]
    fn env_names() {
        let t = toks("\\begin{itemize}");
        assert!(t.contains(&("itemize".into(), Tok::Env)));
    }

    #[test]
    fn finds_word() {
        assert_eq!(word_at("um exemplo aqui", 5), Some((3, 10)));
        assert_eq!(word_at("d'água", 4), Some((0, 7)));
    }

    #[test]
    fn math_environments_are_math() {
        let t = toks("\\begin{align*}\n  x &= y\n\\end{align*} texto $a$");
        assert!(t.contains(&("align*".into(), Tok::Env)) || t.iter().any(|(s, k)| s.contains("align") && *k == Tok::Env));
        assert!(t.iter().any(|(s, k)| s.contains("x") && *k == Tok::Math), "{t:?}");
        assert!(t.contains(&("texto".into(), Tok::Word)), "{t:?}");
        // A dollar inside the environment does not toggle anything.
        let t = toks("\\begin{equation} \\text{$}\\end{equation} fora");
        assert!(t.contains(&("fora".into(), Tok::Word)), "{t:?}");
    }

    #[test]
    fn headings_and_specials() {
        let t = toks("\\section{Intro geral} a & b~c");
        assert!(t.iter().any(|(s, k)| s.contains("Intro") && *k == Tok::Heading), "{t:?}");
        assert!(t.contains(&("a".into(), Tok::Word)));
        assert!(t.contains(&("&".into(), Tok::Special)));
        assert!(t.contains(&("~".into(), Tok::Special)));
    }
}
#[cfg(test)]
mod bench {
    #[test]
    #[ignore]
    fn bench_layout() {
        let text = std::fs::read_to_string(std::env::var("BENCH_TEX").unwrap()).unwrap();
        let pal = super::Palette::new(true);
        let t = std::time::Instant::now();
        for _ in 0..10 { let _ = super::lex_tex(&text, &mut None); }
        println!("lex: {:.1}ms", t.elapsed().as_secs_f64() * 100.0);
        let t = std::time::Instant::now();
        for _ in 0..10 { let _ = super::layout_job(&text, super::Lang::Tex, egui::FontId::monospace(14.0), &pal, None, 600.0, &[]); }
        println!("layout_job: {:.1}ms", t.elapsed().as_secs_f64() * 100.0);
        let t = std::time::Instant::now();
        for _ in 0..10 { let _ = crate::editor::text_hash(&text, &[1]); }
        println!("hash: {:.2}ms", t.elapsed().as_secs_f64() * 100.0);
        let ctx = egui::Context::default();
        let job = super::layout_job(&text, super::Lang::Tex, egui::FontId::monospace(14.0), &pal, None, 600.0, &[]);
        println!("spans: {} sections: {} paragraphs: {}", super::lex_tex(&text, &mut None).len(), job.sections.len(), text.lines().count());
        let _ = ctx.run_ui(Default::default(), |_| {});
        let t = std::time::Instant::now();
        let _ = ctx.fonts_mut(|f| f.layout_job(job.clone()));
        println!("egui layout (cold): {:.1}ms", t.elapsed().as_secs_f64() * 1000.0);
        let t = std::time::Instant::now();
        for _ in 0..10 { let _ = ctx.fonts_mut(|f| f.layout_job(job.clone())); }
        println!("egui layout (warm hit, same frame): {:.2}ms", t.elapsed().as_secs_f64() * 100.0);
        let mut job2 = job.clone();
        job2.text.push('x');
        let t = std::time::Instant::now();
        let _ = ctx.fonts_mut(|f| f.layout_job(job2));
        println!("egui layout (1 char changed): {:.1}ms", t.elapsed().as_secs_f64() * 1000.0);
    }
}
