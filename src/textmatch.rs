//! Finding text seen in the PDF back in the LaTeX source.
//!
//! SyncTeX only says which *line* produced a spot of the PDF. To land the
//! cursor on the exact word clicked, or to select in the source what was
//! selected in the PDF, the source around that line is projected to roughly
//! what TeX typesets (commands, braces, `$`, comments dropped, `~` as a
//! space), keeping a map from each projected char back to the source, and
//! the PDF text is searched in it. A heuristic, like the rest of the
//! app's LaTeX reading: when a phrase cannot be matched whole, its ends
//! are matched separately.

use unicode_normalization::UnicodeNormalization;

/// Accents TeX builds from separate glyphs when fonts lack the accented
/// letter, which PDF text extraction may report as their own characters.
const SPACING_ACCENTS: &[char] = &['ˆ', '˜', '´', '`', '¨', '¸', 'ˇ', '˘', '˙', '˚', '^', '~'];

/// A char's comparable form: lowercase, accents dropped (so "ô", "o"+◌̂
/// and "ˆo" all compare as "o").
fn fold(c: char, out: &mut Vec<char>) {
    for d in c.to_lowercase().collect::<String>().nfd() {
        if !unicode_normalization::char::is_combining_mark(d) {
            out.push(d);
        }
    }
}

/// `text` lowercase and without accents, for searches that should find
/// "Seção" when "secao" is typed.
pub fn folded(text: &str) -> String {
    let mut out = Vec::new();
    for c in text.chars() {
        fold(c, &mut out);
    }
    out.into_iter().collect()
}

/// The typeset-ish projection of a stretch of source: lowercase text with
/// single spaces, and for each of its chars the source char index it came
/// from.
struct Projection {
    text: Vec<char>,
    map: Vec<usize>,
}

/// Commands whose braced argument never shows up as text.
const INVISIBLE_ARGS: &[&str] = &[
    "label", "ref", "eqref", "pageref", "cref", "Cref", "autoref", "cite", "citep", "citet", "parencite", "textcite",
    "autocite", "footcite", "nocite", "includegraphics", "input", "include", "begin", "end", "usepackage",
    "documentclass", "bibliography", "bibliographystyle", "addbibresource", "url", "hypersetup", "vspace", "hspace",
    "setlength", "newcommand", "renewcommand", "color", "pagestyle", "thispagestyle", "graphicspath",
];

fn project(chars: &[char], offset: usize) -> Projection {
    let mut p = Projection { text: Vec::new(), map: Vec::new() };
    // Every projected char maps back to the source char it came from.
    let push = |p: &mut Projection, c: char, at: usize| {
        if c.is_whitespace() {
            if p.text.last().map(|l| *l == ' ').unwrap_or(true) {
                return;
            }
            p.text.push(' ');
        } else {
            fold(c, &mut p.text);
        }
        while p.map.len() < p.text.len() {
            p.map.push(at);
        }
    };
    let mut i = 0;
    let mut skip_arg = false;
    while i < chars.len() {
        let c = chars[i];
        match c {
            '%' => {
                while i < chars.len() && chars[i] != '\n' {
                    i += 1;
                }
                continue;
            }
            '\\' => {
                let start = i;
                i += 1;
                if i < chars.len() && chars[i].is_ascii_alphabetic() {
                    while i < chars.len() && chars[i].is_ascii_alphabetic() {
                        i += 1;
                    }
                    let name: String = chars[start + 1..i].iter().collect();
                    if i < chars.len() && chars[i] == '*' {
                        i += 1;
                    }
                    skip_arg = INVISIBLE_ARGS.contains(&name.as_str()) || name.contains("cite");
                    // The space that ends a command name is not typeset.
                    if i < chars.len() && chars[i] == ' ' && !skip_arg {
                        i += 1;
                    }
                    continue;
                }
                if let Some(&next) = chars.get(i) {
                    i += 1;
                    match next {
                        '\\' => push(&mut p, ' ', offset + start),
                        '%' | '&' | '$' | '#' | '_' | '{' | '}' => push(&mut p, next, offset + start),
                        _ => {} // accents (\'a) and spacing (\,): the letter follows on its own
                    }
                }
                continue;
            }
            '{' if skip_arg => {
                // Skip the whole argument, nested braces included.
                let mut depth = 0;
                while i < chars.len() {
                    match chars[i] {
                        '{' => depth += 1,
                        '}' => {
                            depth -= 1;
                            if depth == 0 {
                                i += 1;
                                break;
                            }
                        }
                        _ => {}
                    }
                    i += 1;
                }
                skip_arg = false;
                continue;
            }
            '[' if skip_arg => {
                while i < chars.len() && chars[i] != ']' {
                    i += 1;
                }
                i += 1;
                continue;
            }
            '{' | '}' | '$' | '^' | '_' | '&' => {}
            '~' => push(&mut p, ' ', offset + i),
            _ => push(&mut p, c, offset + i),
        }
        skip_arg = false;
        i += 1;
    }
    p
}

/// Text taken from the PDF, normalized the way `project` normalizes source:
/// ligatures expanded, lowercase, single spaces.
pub fn normalize_pdf_text(text: &str) -> Vec<char> {
    let expanded = text
        .replace('ﬁ', "fi")
        .replace('ﬂ', "fl")
        .replace('ﬀ', "ff")
        .replace('ﬃ', "ffi")
        .replace('ﬄ', "ffl")
        .replace(['“', '”', '‘', '’'], "")
        .replace(['\u{2013}', '\u{2014}'], "-");
    let mut out = Vec::new();
    for c in expanded.chars() {
        if c.is_whitespace() {
            if out.last().map(|l| *l != ' ').unwrap_or(false) {
                out.push(' ');
            }
        } else if !SPACING_ACCENTS.contains(&c) {
            fold(c, &mut out);
        }
    }
    while out.last() == Some(&' ') {
        out.pop();
    }
    out
}

fn find_all(hay: &[char], needle: &[char]) -> Vec<usize> {
    if needle.is_empty() || needle.len() > hay.len() {
        return vec![];
    }
    (0..=hay.len() - needle.len()).filter(|&i| hay[i..i + needle.len()] == *needle).collect()
}

/// The source char range `[start, end)` of `phrase` (text from the PDF) in
/// `source`, searching lines `line - radius ..= line + radius` (1-based)
/// and preferring the occurrence nearest `line`.
pub fn locate(source: &str, line: usize, radius: usize, phrase: &str) -> Option<(usize, usize)> {
    locate_with(source, line, radius, phrase, project)
}

fn locate_with(
    source: &str,
    line: usize,
    radius: usize,
    phrase: &str,
    project: fn(&[char], usize) -> Projection,
) -> Option<(usize, usize)> {
    let needle = normalize_pdf_text(phrase);
    if needle.is_empty() {
        return None;
    }
    let chars: Vec<char> = source.chars().collect();
    // Char offsets of the window's first and last line.
    let mut line_starts = vec![0];
    for (i, c) in chars.iter().enumerate() {
        if *c == '\n' {
            line_starts.push(i + 1);
        }
    }
    // A stale SyncTeX map can point past the end of an edited buffer.
    let line = line.clamp(1, line_starts.len());
    let first = line.saturating_sub(radius).max(1);
    let last = (line + radius).min(line_starts.len());
    let from = line_starts[first - 1];
    let to = line_starts.get(last).copied().unwrap_or(chars.len());
    let proj = project(&chars[from..to], from);
    let target = line_starts[line - 1];
    let nearest = |hits: Vec<usize>| hits.into_iter().min_by_key(|&h| proj.map[h].abs_diff(target));
    let end_of = |i: usize| proj.map[i] + 1;

    if let Some(h) = nearest(find_all(&proj.text, &needle)) {
        return Some((proj.map[h], end_of(h + needle.len() - 1)));
    }
    // Not whole (a \ref turned into a number, math…): match the ends.
    let words: Vec<&[char]> = needle.split(|c| *c == ' ').filter(|w| !w.is_empty()).collect();
    for n in (1..=words.len().min(4)).rev() {
        let head: Vec<char> = words[..n].join(&' ');
        let Some(h) = nearest(find_all(&proj.text, &head)) else { continue };
        let start = proj.map[h];
        let mut end = end_of(h + head.len() - 1);
        if words.len() > n {
            for m in (1..=words.len().min(4)).rev() {
                let tail: Vec<char> = words[words.len() - m..].join(&' ');
                if let Some(t) = find_all(&proj.text, &tail).into_iter().find(|&t| t > h) {
                    end = end_of(t + tail.len() - 1);
                    break;
                }
            }
        }
        return Some((start, end));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pick(source: &str, range: Option<(usize, usize)>) -> Option<String> {
        range.map(|(a, b)| source.chars().skip(a).take(b - a).collect())
    }

    #[test]
    fn finds_plain_words_on_the_line() {
        let src = "\\section{Introdução}\nNeste trabalho estudamos a série harmônica.\n";
        assert_eq!(pick(src, locate(src, 2, 2, "estudamos")), Some("estudamos".into()));
        assert_eq!(pick(src, locate(src, 2, 2, "trabalho estudamos a")), Some("trabalho estudamos a".into()));
        assert_eq!(pick(src, locate(src, 1, 2, "Introdução")), Some("Introdução".into()));
    }

    #[test]
    fn looks_through_markup() {
        let src = "um \\textbf{Primeiro} ponto e~outro\n";
        assert_eq!(pick(src, locate(src, 1, 0, "Primeiro ponto")), Some("Primeiro} ponto".into()));
        assert_eq!(pick(src, locate(src, 1, 0, "e outro")), Some("e~outro".into()));
        // Wrapped in the source, one line in the PDF.
        let src = "uma frase que\ncontinua aqui % comentário\n";
        assert_eq!(pick(src, locate(src, 1, 1, "que continua")), Some("que\ncontinua".into()));
    }

    #[test]
    fn ligatures_and_case() {
        let src = "A definição final.\n";
        assert_eq!(pick(src, locate(src, 1, 0, "deﬁnição ﬁnal")), Some("definição final".into()));
    }

    #[test]
    fn accents_built_by_tex() {
        let src = "a série harmônica e ação\n";
        // What PDF readers may report for OT1 fonts: the accent as its own glyph.
        assert_eq!(pick(src, locate(src, 1, 0, "harmˆonica")), Some("harmônica".into()));
        assert_eq!(pick(src, locate(src, 1, 0, "s´erie")), Some("série".into()));
        assert_eq!(pick(src, locate(src, 1, 0, "ac\u{327}a\u{303}o")), Some("ação".into()));
    }

    #[test]
    fn falls_back_to_the_ends() {
        // "Seção 1" in the PDF is "Seção~\ref{sec:intro}" in the source.
        let src = "Veja a Seção~\\ref{sec:intro} e a Equação~\\eqref{eq:x} logo.\n";
        let r = pick(src, locate(src, 1, 0, "Veja a Seção 1 e a Equação (1) logo")).unwrap();
        assert!(r.starts_with("Veja a Seção") && r.ends_with("logo"), "{r}");
    }

    #[test]
    fn prefers_the_nearest_occurrence() {
        let src = "palavra\n\n\n\n\npalavra aqui\n";
        let (a, _) = locate(src, 6, 6, "palavra").unwrap();
        assert_eq!(a, src.find("palavra aqui").unwrap());
        assert_eq!(locate(src, 1, 1, "inexistente"), None);
    }

    #[test]
    fn line_past_the_end() {
        // The buffer lost lines since the SyncTeX map was built.
        let src = "uma linha\noutra linha\n";
        assert_eq!(pick(src, locate(src, 40, 4, "outra")), Some("outra".into()));
        assert_eq!(locate(src, 40, 4, "inexistente"), None);
        assert_eq!(locate("", 5, 0, "nada"), None);
    }
}
