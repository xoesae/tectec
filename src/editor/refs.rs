//! What a position in the source points at: the label of a `\ref`, the key
//! of a `\cite`, the file of an `\input`/`\includegraphics`, or a command,
//! for "go to definition" and the hover card. Pure text analysis; the app
//! resolves the target against the project index.

use regex::Regex;
use std::sync::LazyLock;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum RefTarget {
    Label(String),
    Cite(String),
    /// `\input`/`\include`/`\subfile` (a .tex, extension optional).
    TexFile(String),
    /// `\includegraphics` and friends.
    Graphic(String),
    /// `\bibliography`/`\addbibresource`.
    BibFile(String),
    /// A command name with its backslash.
    Command(String),
}

static ARG_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\\([A-Za-z]+)\*?\s*(?:\[[^\]]*\]\s*)*\{([^{}]*)\}").unwrap()
});

fn classify(command: &str, item: &str) -> Option<RefTarget> {
    let item = item.trim().to_string();
    if item.is_empty() {
        return None;
    }
    Some(match command {
        "ref" | "eqref" | "pageref" | "autoref" | "nameref" | "cref" | "Cref" | "vref" => RefTarget::Label(item),
        c if c.contains("cite") => RefTarget::Cite(item),
        "input" | "include" | "subfile" | "includeonly" => RefTarget::TexFile(item),
        "includegraphics" | "includepdf" | "includesvg" => RefTarget::Graphic(item),
        "bibliography" | "addbibresource" => RefTarget::BibFile(item),
        _ => return None,
    })
}

/// With the cursor (char index `pos`) inside the braces of a `\cite{…}`:
/// where to add another key (a char index, at the closing brace) and the
/// text to add there, so `\cite{a}` becomes `\cite{a,key}`.
pub fn cite_append_at(text: &str, pos: usize, key: &str) -> Option<(usize, String)> {
    let byte = text.char_indices().nth(pos).map(|(b, _)| b).unwrap_or(text.len());
    let line_start = text[..byte].rfind('\n').map(|b| b + 1).unwrap_or(0);
    let line_end = text[byte..].find('\n').map(|b| byte + b).unwrap_or(text.len());
    let line = &text[line_start..line_end];
    let at = byte - line_start;
    let m = ARG_RE.captures_iter(line).find(|m| {
        let arg = m.get(2).unwrap();
        m[1].contains("cite") && at >= arg.start() && at <= arg.end()
    })?;
    let arg = m.get(2).unwrap();
    if arg.as_str().split(',').any(|k| k.trim() == key) {
        return Some((text[..line_start + arg.end()].chars().count(), String::new()));
    }
    let sep = if arg.as_str().trim().is_empty() || arg.as_str().trim_end().ends_with(',') { "" } else { "," };
    Some((text[..line_start + arg.end()].chars().count(), format!("{sep}{key}")))
}

/// The target under char index `pos`, looking only at its own line.
pub fn target_at(text: &str, pos: usize) -> Option<RefTarget> {
    let byte = text.char_indices().nth(pos).map(|(b, _)| b).unwrap_or(text.len());
    let line_start = text[..byte].rfind('\n').map(|b| b + 1).unwrap_or(0);
    let line_end = text[byte..].find('\n').map(|b| byte + b).unwrap_or(text.len());
    let line = &text[line_start..line_end];
    let at = byte - line_start;

    for m in ARG_RE.captures_iter(line) {
        let whole = m.get(0).unwrap();
        if at < whole.start() || at > whole.end() {
            continue;
        }
        let arg = m.get(2).unwrap();
        // In a comma list (\cite{a,b}), the item under the pointer; on the
        // command name itself, the first one.
        let offset = at.saturating_sub(arg.start()).min(arg.as_str().len());
        let item = if at < arg.start() {
            arg.as_str().split(',').next().unwrap_or("")
        } else {
            let before = &arg.as_str()[..offset];
            let from = before.rfind(',').map(|c| c + 1).unwrap_or(0);
            let to = arg.as_str()[offset..].find(',').map(|c| offset + c).unwrap_or(arg.as_str().len());
            &arg.as_str()[from..to]
        };
        if let Some(t) = classify(&m[1], item) {
            return Some(t);
        }
    }

    // A bare command under the pointer.
    let bytes = line.as_bytes();
    let mut start = at.min(line.len());
    while start > 0 && bytes[start - 1].is_ascii_alphabetic() {
        start -= 1;
    }
    if start > 0 && bytes[start - 1] == b'\\' {
        let mut end = start;
        while end < line.len() && bytes[end].is_ascii_alphabetic() {
            end += 1;
        }
        if end > start {
            return Some(RefTarget::Command(format!("\\{}", &line[start..end])));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn citing_inside_a_cite() {
        let text = "Ver \\cite{knuth} e mais.";
        // Cursor inside the braces: the key joins the list.
        assert_eq!(cite_append_at(text, 13, "lamport"), Some((15, ",lamport".into())));
        assert_eq!(cite_append_at("\\cite{}", 6, "a"), Some((6, "a".into())));
        assert_eq!(cite_append_at("\\citep[p.~2]{a,}", 15, "b"), Some((15, "b".into())));
        // Already cited: nothing to add, the cursor goes after the list.
        assert_eq!(cite_append_at(text, 12, "knuth"), Some((15, String::new())));
        // Outside the braces, or not a citation: a new \cite.
        assert_eq!(cite_append_at(text, 2, "lamport"), None);
        assert_eq!(cite_append_at("\\ref{x}", 5, "a"), None);
    }

    fn at(text: &str, needle: &str) -> Option<RefTarget> {
        let pos = text[..text.find(needle).unwrap()].chars().count();
        target_at(text, pos)
    }

    #[test]
    fn labels_citations_and_files() {
        let t = "Veja a Seção~\\ref{sec:intro} e \\eqref{eq:x}.";
        assert_eq!(at(t, "intro"), Some(RefTarget::Label("sec:intro".into())));
        assert_eq!(at(t, "\\eqref"), Some(RefTarget::Label("eq:x".into())));
        let t = "como em~\\cite[p.~3]{knuth1984, lamport}.";
        assert_eq!(at(t, "lamport"), Some(RefTarget::Cite("lamport".into())));
        assert_eq!(at(t, "knuth"), Some(RefTarget::Cite("knuth1984".into())));
        assert_eq!(at("\\input{cap/um}", "um"), Some(RefTarget::TexFile("cap/um".into())));
        assert_eq!(
            at("\\includegraphics[width=\\linewidth]{fig/a.png}", "fig"),
            Some(RefTarget::Graphic("fig/a.png".into()))
        );
        assert_eq!(at("\\bibliography{refs}", "refs"), Some(RefTarget::BibFile("refs".into())));
    }

    #[test]
    fn bare_commands_and_nothing() {
        assert_eq!(at("seja $\\norm{x}$", "orm"), Some(RefTarget::Command("\\norm".into())));
        assert_eq!(at("texto comum", "comum"), None);
        assert_eq!(at("\\textbf{negrito}", "negrito"), None);
    }
}
