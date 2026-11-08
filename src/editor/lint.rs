//! The writing reviewer: small, certain mistakes in the prose and the markup
//! around it, found as the text is typed, each with the one fix it needs.
//! Only prose is read (the highlighter's `parts`): math, comments, labels,
//! keys and verbatim blocks never raise anything.

use super::highlight::{parts, Part};
use crate::i18n::{t, tn};
use crate::tf;
use std::ops::Range;

/// A sentence with more words than this is flagged.
const LONG_SENTENCE: usize = 40;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rule {
    RepeatedWord,
    SpaceBeforePunctuation,
    NoSpaceAfterComma,
    StraightQuotes,
    TieBeforeReference,
    Ellipsis,
    FloatWithoutLabel,
    LabelBeforeCaption,
    LongSentence,
    /// The document cites but prints no bibliography.
    NoBibliography,
    /// T1 fonts with no vector face: pdfLaTeX falls back to bitmaps.
    BitmapFonts,
}

/// What the reviewer knows about the document beyond this file's text.
#[derive(Clone, Debug, Default, Hash)]
pub struct Context {
    /// For the main file of a document that cites but prints no
    /// bibliography: what to insert before `\\end{document}`.
    pub bibliography: Option<String>,
    /// The installation lacks cm-super, so T1 text needs a vector font
    /// package (lmodern…) or it is typeset in bitmap fonts.
    pub t1_bitmap: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Issue {
    pub rule: Rule,
    /// What gets underlined (bytes).
    pub range: Range<usize>,
    /// The replacement that fixes it: bytes to replace and the new text.
    pub fix: Option<(Range<usize>, String)>,
    /// The word, environment or count the message names.
    pub detail: String,
}

impl Issue {
    pub fn message(&self) -> String {
        match self.rule {
            Rule::RepeatedWord => tf!("“{word}” is repeated.", word = self.detail),
            Rule::SpaceBeforePunctuation => t("Space before punctuation.").into(),
            Rule::NoSpaceAfterComma => t("Missing space after the comma.").into(),
            Rule::StraightQuotes => t("Straight quotes print as ” on both sides; LaTeX opens quotes with ``.").into(),
            Rule::TieBeforeReference => {
                tf!("A line break can separate “{word}” from the reference; tie them with ~.", word = self.detail)
            }
            Rule::Ellipsis => t("Three periods: \\dots spaces an ellipsis properly.").into(),
            Rule::FloatWithoutLabel => {
                let env = if self.detail == "table" { t("table") } else { t("figure") };
                tf!("This {env} has no \\label, so the text cannot refer to it.", env)
            }
            Rule::LabelBeforeCaption => t("\\label before \\caption: \\ref will give the section's number, not the float's.").into(),
            Rule::NoBibliography => t("The document cites sources but prints no bibliography, so every citation shows “?”.").into(),
            Rule::BitmapFonts => t("T1 without a vector font: the PDF gets bitmap fonts, blurry when zoomed, and its text cannot be searched or copied.").into(),
            Rule::LongSentence => tn(
                "Long sentence ({n} word): consider splitting it.",
                "Long sentence ({n} words): consider splitting it.",
                self.detail.parse().unwrap_or(0),
            ),
        }
    }

    /// What applying the fix does, for its menu entry.
    pub fn fix_label(&self) -> Option<String> {
        let (_, insert) = self.fix.as_ref()?;
        Some(match self.rule {
            Rule::RepeatedWord => tf!("Remove the second “{word}”", word = self.detail),
            Rule::SpaceBeforePunctuation => t("Remove the space").into(),
            Rule::NoSpaceAfterComma => t("Add a space").into(),
            Rule::StraightQuotes => t("Use ``…''").into(),
            Rule::TieBeforeReference => t("Use ~").into(),
            Rule::Ellipsis => t("Use \\dots").into(),
            Rule::FloatWithoutLabel => tf!("Add {label}", label = insert.trim()),
            Rule::NoBibliography => tf!("Insert {command}", command = insert.lines().last().unwrap_or_default()),
            Rule::BitmapFonts => tf!("Insert {command}", command = insert.trim()),
            Rule::LabelBeforeCaption | Rule::LongSentence => return None,
        })
    }

    /// Advice rather than a mistake: drawn fainter.
    pub fn minor(&self) -> bool {
        self.rule == Rule::LongSentence
    }
}

/// Every issue in a LaTeX source, in text order.
#[cfg(test)]
pub fn check(text: &str) -> Vec<Issue> {
    check_document(text, &Context::default())
}

/// `check`, with what is known about the rest of the document.
pub fn check_document(text: &str, context: &Context) -> Vec<Issue> {
    let mut parts = parts(text);
    mask_verbatim(text, &mut parts);
    let mut out = Vec::new();
    for run in prose_runs(&parts) {
        let words = words_in(text, run.clone());
        repeated_words(text, &words, &mut out);
        punctuation(text, run.clone(), &mut out);
    }
    quotes(text, &parts, &mut out);
    ties(text, &parts, &mut out);
    floats(text, &parts, &mut out);
    long_sentences(text, &parts, &mut out);
    if let Some(insert) = &context.bibliography {
        no_bibliography(text, &parts, insert, &mut out);
    }
    if context.t1_bitmap {
        bitmap_fonts(text, &mut out);
    }
    out.sort_by_key(|i| (i.range.start, i.range.end));
    out.dedup_by(|a, b| a.range == b.range && a.rule == b.rule);
    out
}

/// Environments whose body is not prose, whatever the lexer made of it.
const VERBATIM: &[&str] = &["verbatim", "verbatim*", "Verbatim", "lstlisting", "minted", "comment", "tikzpicture", "tabular", "tabular*", "tabularx", "array"];

fn mask_verbatim(text: &str, parts: &mut [(usize, usize, Part)]) {
    let mut from = 0;
    while let Some(i) = text[from..].find("\\begin{").map(|i| from + i) {
        let name_start = i + 7;
        let Some(name_len) = text[name_start..].find('}') else { break };
        let name = &text[name_start..name_start + name_len];
        from = name_start + name_len;
        if !VERBATIM.contains(&name) {
            continue;
        }
        let end = text[from..].find(&format!("\\end{{{name}}}")).map(|e| from + e).unwrap_or(text.len());
        for p in parts.iter_mut().filter(|p| p.0 >= from && p.1 <= end && p.2 == Part::Prose) {
            p.2 = Part::Markup;
        }
        from = end.max(from);
    }
}

/// Byte ranges of consecutive prose.
fn prose_runs(parts: &[(usize, usize, Part)]) -> Vec<Range<usize>> {
    parts.iter().filter(|p| p.2 == Part::Prose).map(|p| p.0..p.1).collect()
}

/// Words (letters with inner apostrophes and hyphens) of a prose run.
fn words_in(text: &str, run: Range<usize>) -> Vec<Range<usize>> {
    let mut out = Vec::new();
    let mut start = None;
    let s = &text[run.clone()];
    for (i, c) in s.char_indices() {
        let inner = (c == '\'' || c == '-') && start.is_some() && s[i + c.len_utf8()..].starts_with(char::is_alphabetic);
        match (c.is_alphabetic() || inner, start) {
            (true, None) => start = Some(i),
            (false, Some(a)) => {
                out.push(run.start + a..run.start + i);
                start = None;
            }
            _ => {}
        }
    }
    if let Some(a) = start {
        out.push(run.start + a..run.end);
    }
    out
}

fn repeated_words(text: &str, words: &[Range<usize>], out: &mut Vec<Issue>) {
    for pair in words.windows(2) {
        let (a, b) = (&pair[0], &pair[1]);
        let gap = &text[a.end..b.start];
        if gap.is_empty() || !gap.chars().all(|c| c == ' ' || c == '\t' || c == '\n') || gap.matches('\n').count() > 1 {
            continue;
        }
        let (wa, wb) = (&text[a.clone()], &text[b.clone()]);
        if wa.to_lowercase() == wb.to_lowercase() {
            out.push(Issue { rule: Rule::RepeatedWord, range: a.start..b.end, fix: Some((a.end..b.end, String::new())), detail: wb.to_string() });
        }
    }
}

/// Inside a command's `[…]` options (`\\documentclass[12pt,a4paper]`),
/// which the lexer leaves as text: lists there take no spaces.
fn in_options(text: &str, at: usize) -> bool {
    let line = &text[text[..at].rfind('\n').map(|n| n + 1).unwrap_or(0)..at];
    line.rfind('[').is_some_and(|open| !line[open..].contains(']'))
}

fn punctuation(text: &str, run: Range<usize>, out: &mut Vec<Issue>) {
    let s = &text[run.clone()];
    let b = s.as_bytes();
    for (i, c) in s.char_indices() {
        let at = run.start + i;
        if matches!(c, ',' | '.' | ';' | ':' | '!' | '?') && in_options(text, at) {
            continue;
        }
        if matches!(c, ',' | '.' | ';' | ':' | '!' | '?') && i > 0 && (b[i - 1] == b' ' || b[i - 1] == b'\t') {
            // Spaces back to the previous character on the same line.
            let mut sp = i;
            while sp > 0 && (b[sp - 1] == b' ' || b[sp - 1] == b'\t') {
                sp -= 1;
            }
            let line_start = text[..run.start + sp].rfind('\n').map(|n| n + 1).unwrap_or(0);
            let mid_line = !text[line_start..run.start + sp].trim().is_empty();
            // `. . .` and a lone `.5` are not this mistake.
            let next = s[i + 1..].chars().next();
            if mid_line && !(c == '.' && (next == Some('.') || next.is_some_and(|n| n.is_ascii_digit()))) {
                out.push(Issue {
                    rule: Rule::SpaceBeforePunctuation,
                    range: run.start + sp..at + 1,
                    fix: Some((run.start + sp..at, String::new())),
                    detail: String::new(),
                });
            }
        }
        if c == ',' {
            let prev = s[..i].chars().next_back();
            let next = s[i + 1..].chars().next();
            if prev.is_some_and(char::is_alphabetic) && next.is_some_and(char::is_alphabetic) {
                let n = next.unwrap().len_utf8();
                out.push(Issue { rule: Rule::NoSpaceAfterComma, range: at..at + 1 + n, fix: Some((at + 1..at + 1, " ".into())), detail: String::new() });
            }
        }
        if c == '.' && s[i..].starts_with("...") && !s[..i].ends_with('.') {
            let after = s[i + 3..].chars().next();
            let insert = if after.is_some_and(|a| a.is_whitespace() || a.is_alphabetic()) { "\\dots{}" } else { "\\dots" };
            out.push(Issue { rule: Rule::Ellipsis, range: at..at + 3, fix: Some((at..at + 3, insert.into())), detail: String::new() });
        }
    }
}

/// Straight double quotes in prose, paired within a paragraph.
fn quotes(text: &str, parts: &[(usize, usize, Part)], out: &mut Vec<Issue>) {
    let mut open: Option<usize> = None;
    for (start, end, part) in parts {
        let chunk = &text[*start..*end];
        if *part != Part::Prose {
            continue;
        }
        for (i, c) in chunk.char_indices() {
            let at = start + i;
            if let Some(o) = open {
                if text[o..at].contains("\n\n") {
                    open = None;
                }
            }
            if c != '"' {
                continue;
            }
            match open.take() {
                None => open = Some(at),
                Some(o) => {
                    let inner = &text[o + 1..at];
                    out.push(Issue {
                        rule: Rule::StraightQuotes,
                        range: o..at + 1,
                        fix: Some((o..at + 1, format!("``{inner}''"))),
                        detail: String::new(),
                    });
                }
            }
        }
    }
}

/// Commands that print a number or a citation right after a word.
const TIED: &[&str] = &["\\ref", "\\eqref", "\\pageref", "\\cref", "\\autoref", "\\vref", "\\cite", "\\parencite", "\\autocite"];

fn ties(text: &str, parts: &[(usize, usize, Part)], out: &mut Vec<Issue>) {
    for (start, end, part) in parts {
        if *part != Part::Command {
            continue;
        }
        let name = text[*start..*end].trim_end_matches('*');
        if !TIED.contains(&name) {
            continue;
        }
        let before = &text[..*start];
        let spaces = before.len() - before.trim_end_matches([' ', '\t']).len();
        if spaces == 0 {
            continue;
        }
        let word_end = *start - spaces;
        let prev = before[..word_end].chars().next_back();
        if !prev.is_some_and(|c| c.is_alphanumeric()) {
            continue;
        }
        let word_start = before[..word_end].rfind(|c: char| !c.is_alphanumeric()).map(|i| i + 1).unwrap_or(0);
        out.push(Issue {
            rule: Rule::TieBeforeReference,
            // On the word: the command keeps its own hover card.
            range: word_start..*start,
            fix: Some((word_end..*start, "~".into())),
            detail: text[word_start..word_end].to_string(),
        });
    }
}

const FLOATS: &[&str] = &["figure", "figure*", "table", "table*", "wrapfigure", "wraptable"];

fn floats(text: &str, parts: &[(usize, usize, Part)], out: &mut Vec<Issue>) {
    let is_command = |at: usize, name: &str| {
        parts.iter().any(|p| p.2 == Part::Command && p.0 == at && &text[p.0..p.1] == name)
    };
    let mut from = 0;
    while let Some(i) = text[from..].find("\\begin{").map(|i| from + i) {
        from = i + 7;
        let Some(len) = text[from..].find('}') else { break };
        let env = &text[from..from + len];
        if !FLOATS.contains(&env) || !is_command(i, "\\begin") {
            continue;
        }
        let head_end = from + len + 1;
        let end = text[head_end..].find(&format!("\\end{{{env}}}")).map(|e| head_end + e).unwrap_or(text.len());
        let body = &text[head_end..end];
        let find = |name: &str| {
            let mut k = 0;
            while let Some(p) = body[k..].find(name).map(|p| k + p) {
                let after = body[p + name.len()..].chars().next();
                if is_command(head_end + p, name) && !after.is_some_and(|c| c.is_ascii_alphabetic()) {
                    return Some(head_end + p);
                }
                k = p + name.len();
            }
            None
        };
        let (caption, label) = (find("\\caption"), find("\\label"));
        let kind = env.trim_end_matches('*').replace("wrap", "");
        match (caption, label) {
            (Some(c), None) => {
                let fix = caption_end(text, c).map(|close| {
                    let line_start = text[..c].rfind('\n').map(|n| n + 1).unwrap_or(0);
                    let indent: String = text[line_start..c].chars().take_while(|c| c.is_whitespace()).collect();
                    let prefix = if kind == "table" { "tab" } else { "fig" };
                    let slug = slug(&text[c..close]);
                    (close..close, format!("\n{indent}\\label{{{prefix}:{slug}}}"))
                });
                out.push(Issue { rule: Rule::FloatWithoutLabel, range: i..head_end, fix, detail: kind });
            }
            (Some(c), Some(l)) if l < c => {
                out.push(Issue { rule: Rule::LabelBeforeCaption, range: l..l + 6, fix: None, detail: String::new() });
            }
            _ => {}
        }
        from = end.max(from);
    }
}

/// The byte after `\caption[…]{…}`'s closing brace.
fn caption_end(text: &str, at: usize) -> Option<usize> {
    let mut i = at + "\\caption".len();
    let b = text.as_bytes();
    while i < b.len() && b[i].is_ascii_whitespace() {
        i += 1;
    }
    if b.get(i) == Some(&b'[') {
        i += text[i..].find(']')? + 1;
    }
    while i < b.len() && b[i].is_ascii_whitespace() {
        i += 1;
    }
    if b.get(i) != Some(&b'{') {
        return None;
    }
    let mut depth = 0;
    for (k, c) in text[i..].char_indices() {
        match c {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i + k + 1);
                }
            }
            _ => {}
        }
    }
    None
}

/// A label from the first words of a caption, as plain ASCII.
fn slug(caption: &str) -> String {
    use unicode_normalization::UnicodeNormalization;
    let inner = caption.split_once('{').map(|(_, r)| r).unwrap_or(caption);
    let plain: String = inner.nfd().filter(|c| c.is_ascii()).collect::<String>().to_lowercase();
    let words: Vec<&str> = plain
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|w| w.len() > 2 || w.chars().all(|c| c.is_ascii_digit()))
        .filter(|w| !w.is_empty())
        .take(3)
        .collect();
    if words.is_empty() { "nova".into() } else { words.join("-") }
}

fn no_bibliography(text: &str, parts: &[(usize, usize, Part)], insert: &str, out: &mut Vec<Issue>) {
    let Some(end) = parts.iter().rev().find(|p| p.2 == Part::Command && &text[p.0..p.1] == "\\end" && text[p.1..].starts_with("{document}")) else {
        return;
    };
    let line_start = text[..end.0].rfind('\n').map(|n| n + 1).unwrap_or(0);
    let range = end.0..end.1 + "{document}".len();
    out.push(Issue { rule: Rule::NoBibliography, range, fix: Some((line_start..line_start, insert.to_string())), detail: String::new() });
}

/// Packages that give T1 text vector fonts.
const VECTOR_FONTS: &[&str] = &[
    "lmodern", "times", "mathptmx", "newtxtext", "newpxtext", "libertine", "libertinus", "mathpazo", "palatino",
    "charter", "XCharter", "tgtermes", "tgpagella", "tgheros", "tgschola", "tgbonum", "fourier", "kpfonts", "ebgaramond",
    "garamondx", "helvet", "utopia", "erewhon", "stix", "stix2", "baskervald", "bookman", "fontspec", "cochineal", "crimson",
];

fn bitmap_fonts(text: &str, out: &mut Vec<Issue>) {
    static PACKAGE: std::sync::LazyLock<regex::Regex> =
        std::sync::LazyLock::new(|| regex::Regex::new(r"\\usepackage\s*(?:\[([^\]]*)\])?\s*\{([^}]*)\}").unwrap());
    let preamble_end = text.find("\\begin{document}").unwrap_or(text.len());
    let mut t1 = None;
    for (number, line) in text[..preamble_end].split('\n').enumerate() {
        let code = crate::backend::texparse::without_comment(line);
        for caps in PACKAGE.captures_iter(code) {
            let names: Vec<&str> = caps[2].split(',').map(str::trim).collect();
            if names.iter().any(|n| VECTOR_FONTS.contains(n)) {
                return;
            }
            let options = caps.get(1).map(|o| o.as_str()).unwrap_or("");
            if names.contains(&"fontenc") && options.split(',').any(|o| o.trim() == "T1") {
                let line_start: usize = text.split('\n').take(number).map(|l| l.len() + 1).sum();
                let m = caps.get(0).unwrap();
                t1 = Some((line_start + m.start()..line_start + m.end(), line_start + line.len()));
            }
        }
    }
    if let Some((range, line_end)) = t1 {
        let at = (line_end + 1).min(text.len());
        let insert = if line_end >= text.len() { "\n\\usepackage{lmodern}".to_string() } else { "\\usepackage{lmodern}\n".to_string() };
        out.push(Issue { rule: Rule::BitmapFonts, range, fix: Some((at..at, insert)), detail: String::new() });
    }
}

/// Commands that end a sentence's run of words: structure, not prose.
const BREAKS: &[&str] = &["\\section", "\\subsection", "\\subsubsection", "\\chapter", "\\paragraph", "\\item", "\\begin", "\\end", "\\caption", "\\par", "\\footnote"];

fn long_sentences(text: &str, parts: &[(usize, usize, Part)], out: &mut Vec<Issue>) {
    let mut words = 0usize;
    let mut first: Option<usize> = None;
    let mut last_end = 0usize;
    let flush = |words: &mut usize, first: &mut Option<usize>, end: usize, out: &mut Vec<Issue>| {
        if *words > LONG_SENTENCE {
            if let Some(a) = *first {
                out.push(Issue { rule: Rule::LongSentence, range: a..end, fix: None, detail: words.to_string() });
            }
        }
        *words = 0;
        *first = None;
    };
    for (start, end, part) in parts {
        match part {
            Part::Command if BREAKS.contains(&text[*start..*end].trim_end_matches('*')) => flush(&mut words, &mut first, last_end, out),
            Part::Prose => {
                let chunk = &text[*start..*end];
                let mut in_word = false;
                for (i, c) in chunk.char_indices() {
                    let at = start + i;
                    if c.is_alphanumeric() {
                        if !in_word {
                            words += 1;
                            first.get_or_insert(at);
                        }
                        in_word = true;
                        last_end = at + c.len_utf8();
                        continue;
                    }
                    in_word = false;
                    let next = chunk[i + c.len_utf8()..].chars().next();
                    let ends = matches!(c, '.' | '!' | '?') && next.is_none_or(char::is_whitespace);
                    let blank = c == '\n' && next == Some('\n');
                    if ends {
                        flush(&mut words, &mut first, at + 1, out);
                    } else if blank {
                        flush(&mut words, &mut first, last_end, out);
                    }
                }
            }
            _ => {}
        }
    }
    flush(&mut words, &mut first, last_end, out);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rules(text: &str) -> Vec<(Rule, String)> {
        check(text).into_iter().map(|i| (i.rule, text[i.range].to_string())).collect()
    }

    fn fixed(text: &str) -> String {
        let mut s = text.to_string();
        let mut issues = check(text);
        issues.reverse();
        for i in issues {
            if let Some((r, insert)) = i.fix {
                s.replace_range(r, &insert);
            }
        }
        s
    }

    #[test]
    fn repeated_words() {
        assert_eq!(rules("a velocidade de de uma ideia"), vec![(Rule::RepeatedWord, "de de".into())]);
        assert_eq!(fixed("de de uma"), "de uma");
        assert!(rules("de \\emph{de}").is_empty(), "markup in between is deliberate");
        assert!(rules("fim.\n\nFim de").is_empty());
    }

    #[test]
    fn punctuation() {
        assert_eq!(fixed("não linear , como"), "não linear, como");
        assert_eq!(fixed("rede ; outra"), "rede; outra");
        assert_eq!(fixed("sim,não"), "sim, não");
        assert!(rules("x = 3,14 e 2 .5").is_empty());
        assert!(rules("  , no começo da linha").is_empty());
        assert_eq!(fixed("e depois... fim"), "e depois\\dots{} fim");
        assert_eq!(fixed("fim..."), "fim\\dots");
        assert!(rules("$1, \\ldots, n$ e $a ,b$").is_empty(), "math is not prose");
        assert!(rules("% comentário , aqui").is_empty());
        assert!(rules("\\documentclass[12pt,a4paper]{report}\n\\usepackage[english,brazil]{babel}").is_empty());
        assert!(rules("\\includegraphics[width=0.5\\linewidth,angle=90]{a}").is_empty());
    }

    #[test]
    fn quotes() {
        assert_eq!(fixed("por \"modelos epidêmicos\" clássicos"), "por ``modelos epidêmicos'' clássicos");
        assert!(rules("\\begin{verbatim}\nx = \"a\"\n\\end{verbatim}").is_empty());
    }

    #[test]
    fn ties() {
        assert_eq!(fixed("na Figura \\ref{fig:a} e em \\cite{k}."), "na Figura~\\ref{fig:a} e em~\\cite{k}.");
        assert!(rules("Figura~\\ref{a}").is_empty());
        assert!(rules("(\\ref{a})").is_empty());
        assert!(rules("\\label{x} \\ref{y}").is_empty(), "after markup there is no word to tie");
    }

    #[test]
    fn floats() {
        let src = "\\begin{figure}[htbp]\n  \\centering\n  \\includegraphics{a}\n  \\caption{Rede de colaboração.}\n\\end{figure}";
        let issues = check(src);
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].rule, Rule::FloatWithoutLabel);
        assert_eq!(fixed(src), src.replace("colaboração.}", "colaboração.}\n  \\label{fig:rede-colaboracao}"));
        let labelled = "\\begin{table}\n\\caption{T}\n\\label{tab:t}\n\\end{table}";
        assert!(check(labelled).is_empty());
        let early = "\\begin{figure}\n\\label{fig:a}\n\\caption{A}\n\\end{figure}";
        assert_eq!(check(early)[0].rule, Rule::LabelBeforeCaption);
        assert!(check("% \\begin{figure}\\caption{x}\\end{figure}").is_empty());
    }

    #[test]
    fn no_bibliography() {
        let src = "\\begin{document}\nVer \\cite{k}.\n\\end{document}\n";
        assert!(check(src).iter().all(|i| i.rule != Rule::NoBibliography), "only for the main file of such a document");
        let context = Context { bibliography: Some("\\bibliographystyle{plain}\n\\bibliography{refs}\n".into()), ..Default::default() };
        let issues = check_document(src, &context);
        let issue = issues.iter().find(|i| i.rule == Rule::NoBibliography).unwrap();
        assert_eq!(issue.fix_label().unwrap(), "Insert \\bibliography{refs}");
        let mut s = src.to_string();
        let (r, insert) = issue.fix.clone().unwrap();
        s.replace_range(r, &insert);
        assert!(s.ends_with("\\bibliographystyle{plain}\n\\bibliography{refs}\n\\end{document}\n"));
    }

    #[test]
    fn bitmap_fonts() {
        let src = "\\documentclass{article}\n\\usepackage[T1]{fontenc}\n\\begin{document}\nOi.\n\\end{document}\n";
        let bitmap = Context { t1_bitmap: true, ..Default::default() };
        let issues = check_document(src, &bitmap);
        let issue = issues.iter().find(|i| i.rule == Rule::BitmapFonts).unwrap();
        let mut s = src.to_string();
        let (r, insert) = issue.fix.clone().unwrap();
        s.replace_range(r, &insert);
        assert!(s.contains("\\usepackage[T1]{fontenc}\n\\usepackage{lmodern}\n\\begin{document}"));
        assert!(check_document(&s, &bitmap).iter().all(|i| i.rule != Rule::BitmapFonts), "fixed");
        assert!(check_document(src, &Context::default()).iter().all(|i| i.rule != Rule::BitmapFonts), "cm-super installed");
    }

    #[test]
    fn long_sentences() {
        // Numbered, so no word repeats.
        let words = |w: &str, n: usize| (0..n).map(|i| format!("{w}{i}")).collect::<Vec<_>>().join(" ");
        let long = format!("{}.", words("palavra", 45));
        let issues = check(&long);
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].rule, Rule::LongSentence);
        assert_eq!(issues[0].detail, "45");
        let short = format!("{}. {}.", words("a", 30), words("b", 30));
        assert!(check(&short).is_empty());
        let split = format!("{}\n\\section{{X}}\n{}.", words("a", 30), words("b", 30));
        assert!(check(&split).is_empty(), "a heading ends the run");
    }
}
