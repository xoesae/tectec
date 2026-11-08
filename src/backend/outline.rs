//! The sectioning structure of one LaTeX source, in document order.
//!
//! It takes the *content*, not a path: the outline describes the buffer in the
//! editor, not the file on disk (the app only saves after a pause).

use super::texparse;
use regex::Regex;
use std::sync::LazyLock;
use crate::i18n::t;

#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    /// 0 for \part … 4 for \subsubsection.
    pub level: usize,
    pub kind: String,
    pub starred: bool,
    pub title: String,
    /// 1-based.
    pub line: usize,
}

// Longest alternative first, and the title is *not* captured: one line can
// carry two headings.
static SECTIONING_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\\(subsubsection|subsection|section|chapter|part)(\*?)\s*(?:\[[^\]]*\])?\s*\{").unwrap()
});

fn level_of(kind: &str) -> usize {
    match kind {
        "part" => 0,
        "chapter" => 1,
        "section" => 2,
        "subsection" => 3,
        _ => 4,
    }
}

pub fn parse(content: &str) -> Vec<Entry> {
    let mut entries = Vec::new();
    for (number, line) in content.split('\n').enumerate() {
        if !line.contains('\\') {
            continue;
        }
        let text = texparse::without_comment(line);
        for caps in SECTIONING_RE.captures_iter(text) {
            let whole = caps.get(0).unwrap();
            let kind = caps.get(1).unwrap().as_str();
            let mut title = texparse::argument_text(&text[whole.end()..]);
            if title.is_empty() {
                title = t("(untitled)").into();
            }
            entries.push(Entry {
                level: level_of(kind),
                kind: kind.to_string(),
                starred: !caps.get(2).unwrap().as_str().is_empty(),
                title,
                line: number + 1,
            });
        }
    }
    entries
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Flattens entries into "kind:line:title" so a failure prints the whole
    /// structure instead of one field of one entry.
    fn check(content: &str, want: &[&str]) {
        let got: Vec<String> = parse(content)
            .iter()
            .map(|e| format!("{}{}:{}:{}", e.kind, if e.starred { "*" } else { "" }, e.line, e.title))
            .collect();
        assert_eq!(got, want);
    }

    #[test]
    fn parses_headings() {
        let src = "\\chapter{Um}\n% \\section{no}\n\\section*{Dois} \\subsection{Três}\n\\subsubsection[x]{}";
        let e = parse(src);
        assert_eq!(e.len(), 4);
        assert_eq!(e[0].title, "Um");
        assert_eq!(e[0].level, 1);
        assert!(e[1].starred);
        assert_eq!(e[2].title, "Três");
        assert_eq!(e[2].line, 3);
        assert_eq!(e[3].title, "(untitled)");
        assert_eq!(e[3].level, 4);
    }

    #[test]
    fn keeps_document_order_and_lines() {
        check(
            "\\documentclass{book}\n\\begin{document}\n\\part{Fundamentos}\n\\chapter{Introdução}\nTexto do capítulo.\n\
             \\section{Contexto}\n\\subsection{Trabalhos relacionados}\n\\subsubsection{Detalhe}\n\\chapter{Método}\n\\end{document}\n",
            &[
                "part:3:Fundamentos",
                "chapter:4:Introdução",
                "section:6:Contexto",
                "subsection:7:Trabalhos relacionados",
                "subsubsection:8:Detalhe",
                "chapter:9:Método",
            ],
        );
    }

    #[test]
    fn gives_the_standard_levels() {
        let levels: Vec<usize> =
            parse("\\part{a}\n\\chapter{b}\n\\section{c}\n\\subsection{d}\n\\subsubsection{e}").iter().map(|e| e.level).collect();
        assert_eq!(levels, [0, 1, 2, 3, 4]);
    }

    #[test]
    fn ignores_commented_out_sections() {
        check(
            "\\section{Real}\n% \\section{Comentada}\nTexto \\% com porcento escapado\n  %\\subsection{Também comentada}",
            &["section:1:Real"],
        );
    }

    #[test]
    fn cuts_the_comment_before_matching() {
        check("código \\emph{x} % \\chapter{Não}", &[]);
    }

    #[test]
    fn reads_two_headings_on_one_line() {
        check("\\section{Primeira} \\subsection{Segunda}", &["section:1:Primeira", "subsection:1:Segunda"]);
    }

    #[test]
    fn handles_starred_and_optional_title() {
        check(
            "\\section*{Agradecimentos}\n\\chapter[Curto]{Um título bem mais longo}",
            &["section*:1:Agradecimentos", "chapter:2:Um título bem mais longo"],
        );
    }

    #[test]
    fn strips_markup_from_titles() {
        check(
            "\\section{Sobre \\emph{isto} e aquilo}\n\\section{Custo \\& benefício}\n\\subsection{Espa\\c{c}os    demais}",
            &["section:1:Sobre isto e aquilo", "section:2:Custo & benefício", "subsection:3:Espacos demais"],
        );
    }

    #[test]
    fn closes_the_title_at_the_right_brace() {
        check("\\section{Um {grupo} dentro} \\label{sec:x}", &["section:1:Um grupo dentro"]);
    }

    #[test]
    fn names_an_empty_title() {
        check("\\section{}", &["section:1:(untitled)"]);
    }

    /// The outline describes a buffer, including one that is not valid LaTeX yet.
    #[test]
    fn accepts_an_unfinished_line() {
        check("\\section{Ainda digitando", &["section:1:Ainda digitando"]);
    }

    /// A prefix is not a command: `\partial` shows up all over a maths thesis.
    #[test]
    fn ignores_non_sectioning_commands() {
        check(
            "\\documentclass{book}\n\\usepackage{graphicx}\n\\subsectionfoo{Não é um comando de seção}\n\
             \\[ \\partial {u} / \\partial {t} \\]\n\\sectionmark{cabeçalho}\n\\label{sec:x}\n\\sec{tion}",
            &[],
        );
    }

    #[test]
    fn handles_crlf() {
        check("\\section{Windows}\r\n\\subsection{Também}\r\n", &["section:1:Windows", "subsection:2:Também"]);
    }
}
