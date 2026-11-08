//! Starting points for a new project: a folder with a `main.tex` that
//! compiles as it is, a `refs.bib` and a `.gitignore` for
//! the compiler's leftovers.
//! Only packages every TeX distribution ships are used, so the first
//! compilation of a fresh project never fails for a missing package; Brazilian
//! Portuguese is switched on when its babel support is installed, and the
//! ABNT template needs abnTeX2 (the app says how to install it otherwise).

use super::project::Res;
use std::fs;
use std::path::{Path, PathBuf};
use crate::i18n::t;
use crate::tf;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Template {
    Article,
    Report,
    Beamer,
    /// A Brazilian academic work (TCC, dissertação, tese) in abnTeX2.
    Abnt,
}

impl Template {
    pub const ALL: [Template; 4] = [Template::Abnt, Template::Article, Template::Report, Template::Beamer];

    pub fn label(self) -> &'static str {
        match self {
            Template::Article => t("Article"),
            Template::Report => t("Report or thesis (with chapters)"),
            Template::Beamer => t("Presentation (beamer)"),
            Template::Abnt => t("ABNT academic work (abnTeX2)"),
        }
    }

    /// The main file's text, in English or in Portuguese (ABNT is always
    /// Portuguese: the standard is Brazilian).
    fn main_tex(self, english: bool) -> &'static str {
        match (self, english) {
            (Template::Article, false) => ARTICLE,
            (Template::Article, true) => ARTICLE_EN,
            (Template::Report, false) => REPORT,
            (Template::Report, true) => REPORT_EN,
            (Template::Beamer, false) => BEAMER,
            (Template::Beamer, true) => BEAMER_EN,
            (Template::Abnt, _) => ABNT,
        }
    }

    fn main_name(self) -> &'static str {
        "main.tex"
    }
}

// lmodern: with T1 and no cm-super, pdfLaTeX would fall back to bitmap
// fonts, blurry when zoomed, and text that cannot be searched or copied.
const PREAMBLE: &str = "\\usepackage[utf8]{inputenc}
\\usepackage[T1]{fontenc}
\\usepackage{lmodern}
%BABEL%\\usepackage{amsmath,amssymb}
\\usepackage{graphicx}
\\usepackage{hyperref}
";

/// Portuguese hyphenation and names ("Figura", "Sumário"), when installed.
const BABEL: &str = "\\usepackage[brazil]{babel}\n";
/// …and how to get them when not.
const BABEL_MISSING: &str = "% Para hifenização e nomes em português, instale o suporte ao idioma
% (pacote babel-portuges; no Debian/Ubuntu, texlive-lang-portuguese) e descomente:
% \\usepackage[brazil]{babel}
";

const ABNT: &str = r"\documentclass[12pt,openright,oneside,a4paper,chapter=TITLE,section=TITLE,brazil]{abntex2}
\usepackage[T1]{fontenc}
\usepackage[utf8]{inputenc}
\usepackage{lmodern}
\usepackage{indentfirst}
\usepackage{graphicx}
\usepackage{microtype}
\usepackage[alf]{abntex2cite}

\titulo{Título do trabalho}
\autor{Seu nome}
\local{Cidade}
\data{\the\year}
\instituicao{Universidade\par Centro\par Curso}
\tipotrabalho{Trabalho de Conclusão de Curso}
\preambulo{Trabalho de Conclusão de Curso apresentado ao curso de \ldots{} como requisito parcial para a obtenção do título de \ldots.}
\orientador{Prof. Dr. Nome do orientador}

\begin{document}
\selectlanguage{brazil}
\frenchspacing

\imprimircapa
\imprimirfolhaderosto

\begin{resumo}
Um resumo de 150 a 500 palavras: objetivo, método, resultados e conclusões.

\vspace{\onelineskip}
\noindent\textbf{Palavras-chave}: primeira. segunda. terceira.
\end{resumo}

\begin{resumo}[Abstract]
\begin{otherlanguage*}{english}
The same summary, in English.

\vspace{\onelineskip}
\noindent\textbf{Keywords}: first. second. third.
\end{otherlanguage*}
\end{resumo}

\pdfbookmark[0]{\contentsname}{toc}
\tableofcontents*
\cleardoublepage

\textual

\chapter{Introdução}
\label{cap:introducao}
Comece a escrever aqui. O preview à direita se atualiza sozinho quando você
para de digitar. As citações seguem a ABNT: \citeonline{exemplo} ou
\cite{exemplo}.

\chapter{Conclusão}
\label{cap:conclusao}

\postextual
\bibliography{refs}

\end{document}
";

const ARTICLE: &str = "\\documentclass[11pt,a4paper]{article}
%PREAMBLE%
\\title{Título do artigo}
\\author{Seu nome}
\\date{\\today}

\\begin{document}
\\maketitle

\\begin{abstract}
Um resumo curto do trabalho.
\\end{abstract}

\\section{Introdução}
\\label{sec:introducao}
Comece a escrever aqui. O preview à direita se atualiza sozinho
quando você para de digitar.

\\section{Desenvolvimento}
Uma equação numerada:
\\begin{equation}
  e^{i\\pi} + 1 = 0
  \\label{eq:euler}
\\end{equation}
e uma referência a ela: \\eqref{eq:euler}. Citações vêm do
\\texttt{refs.bib}~\\cite{exemplo}.

\\bibliographystyle{plain}
\\bibliography{refs}
\\end{document}
";

const REPORT: &str = "\\documentclass[12pt,a4paper]{report}
%PREAMBLE%
\\title{Título do trabalho}
\\author{Seu nome}
\\date{\\today}

\\begin{document}
\\maketitle
\\tableofcontents

\\chapter{Introdução}
\\label{cap:introducao}
Comece a escrever aqui. Capítulos longos podem ir para arquivos
próprios e entrar com \\verb|\\input{capitulo}|.

\\chapter{Revisão bibliográfica}
Citações vêm do \\texttt{refs.bib}~\\cite{exemplo}.

\\chapter{Conclusão}

\\bibliographystyle{plain}
\\bibliography{refs}
\\end{document}
";

const BEAMER: &str = "\\documentclass{beamer}
\\usepackage[utf8]{inputenc}
\\usepackage[T1]{fontenc}
\\usepackage{lmodern}
\\usepackage{amsmath,amssymb}
\\usetheme{default}

\\title{Título da apresentação}
\\author{Seu nome}
\\date{\\today}

\\begin{document}

\\begin{frame}
  \\titlepage
\\end{frame}

\\begin{frame}{Primeiro slide}
  \\begin{itemize}
    \\item Um ponto
    \\item Outro ponto
  \\end{itemize}
\\end{frame}

\\begin{frame}{Uma equação}
  \\[
    \\int_0^1 x^2\\,dx = \\frac{1}{3}
  \\]
\\end{frame}

\\end{document}
";

const ARTICLE_EN: &str = "\\documentclass[11pt,a4paper]{article}
%PREAMBLE%
\\title{Title of the article}
\\author{Your name}
\\date{\\today}

\\begin{document}
\\maketitle

\\begin{abstract}
A short summary of the work.
\\end{abstract}

\\section{Introduction}
\\label{sec:introduction}
Start writing here. The preview on the right updates by itself
when you stop typing.

\\section{Development}
A numbered equation:
\\begin{equation}
  e^{i\\pi} + 1 = 0
  \\label{eq:euler}
\\end{equation}
and a reference to it: \\eqref{eq:euler}. Citations come from
\\texttt{refs.bib}~\\cite{exemplo}.

\\bibliographystyle{plain}
\\bibliography{refs}
\\end{document}
";

const REPORT_EN: &str = "\\documentclass[12pt,a4paper]{report}
%PREAMBLE%
\\title{Title of the work}
\\author{Your name}
\\date{\\today}

\\begin{document}
\\maketitle
\\tableofcontents

\\chapter{Introduction}
\\label{chap:introduction}
Start writing here. Long chapters can go in files of their own
and come in with \\verb|\\input{chapter}|.

\\chapter{Literature review}
Citations come from \\texttt{refs.bib}~\\cite{exemplo}.

\\chapter{Conclusion}

\\bibliographystyle{plain}
\\bibliography{refs}
\\end{document}
";

const BEAMER_EN: &str = "\\documentclass{beamer}
\\usepackage[utf8]{inputenc}
\\usepackage[T1]{fontenc}
\\usepackage{lmodern}
\\usepackage{amsmath,amssymb}
\\usetheme{default}

\\title{Title of the presentation}
\\author{Your name}
\\date{\\today}

\\begin{document}

\\begin{frame}
  \\titlepage
\\end{frame}

\\begin{frame}{First slide}
  \\begin{itemize}
    \\item One point
    \\item Another point
  \\end{itemize}
\\end{frame}

\\begin{frame}{An equation}
  \\[
    \\int_0^1 x^2\\,dx = \\frac{1}{3}
  \\]
\\end{frame}

\\end{document}
";

const BIB: &str = "@book{exemplo,
  author    = {Donald E. Knuth},
  title     = {The {\\TeX}book},
  publisher = {Addison-Wesley},
  year      = {1984}
}
";

const GITIGNORE: &str = ".tectec/
*.aux
*.log
*.out
*.toc
*.bbl
*.blg
*.fls
*.fdb_latexmk
*.synctex.gz
*.nav
*.snm
";

/// Writes the template into `dir` (created when missing) and returns the
/// path of its main file. Never overwrites: a folder that already has one is
/// refused.
pub fn create(dir: &Path, template: Template) -> Res<PathBuf> {
    create_in(dir, template, crate::i18n::locale() == crate::i18n::Locale::English)
}

/// `create`, in English or in Portuguese.
pub(crate) fn create_in(dir: &Path, template: Template, english: bool) -> Res<PathBuf> {
    fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let main = dir.join(template.main_name());
    if main.exists() {
        return Err(tf!("this folder already has a {name}; choose an empty folder", name = template.main_name()));
    }
    // English is LaTeX's own language: no babel for it.
    let babel = if english { "" } else if super::latex::has_file("brazil.ldf") { BABEL } else { BABEL_MISSING };
    let tex = template.main_tex(english).replace("%PREAMBLE%\n", &PREAMBLE.replace("%BABEL%", babel));
    fs::write(&main, tex).map_err(|e| e.to_string())?;
    if template != Template::Beamer && !dir.join("refs.bib").exists() {
        fs::write(dir.join("refs.bib"), BIB).map_err(|e| e.to_string())?;
    }
    if !dir.join(".gitignore").exists() {
        fs::write(dir.join(".gitignore"), GITIGNORE).map_err(|e| e.to_string())?;
    }
    Ok(main)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn english_templates() {
        let dir = tempfile::tempdir().unwrap();
        let tex = fs::read_to_string(create_in(dir.path(), Template::Report, true).unwrap()).unwrap();
        assert!(tex.contains("\\chapter{Introduction}") && !tex.contains("babel") && !tex.contains("%BABEL%"));
        let abnt = tempfile::tempdir().unwrap();
        let tex = fs::read_to_string(create_in(abnt.path(), Template::Abnt, true).unwrap()).unwrap();
        assert!(tex.contains("\\chapter{Introdução}"), "ABNT stays Portuguese");
        // English templates compile as they are, too.
        if crate::backend::latex::which("pdflatex") {
            for t in [Template::Article, Template::Report] {
                let d = tempfile::tempdir().unwrap();
                let main = create_in(d.path(), t, true).unwrap();
                let r = crate::backend::latex::compile_system(std::sync::Arc::new(false.into()), &main).unwrap();
                assert!(r.success, "{t:?}: {:?}", r.errors);
            }
        }
    }

    #[test]
    fn portuguese_when_installed() {
        let dir = tempfile::tempdir().unwrap();
        let tex = fs::read_to_string(create_in(dir.path(), Template::Article, false).unwrap()).unwrap();
        let on = tex.lines().any(|l| l == "\\usepackage[brazil]{babel}");
        assert_eq!(on, crate::backend::latex::has_file("brazil.ldf"));
        assert!(!tex.contains("%BABEL%"));
    }

    #[test]
    fn creates_without_overwriting() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("novo");
        let main = create(&root, Template::Article).unwrap();
        let tex = fs::read_to_string(&main).unwrap();
        assert!(tex.contains("\\usepackage{hyperref}") && !tex.contains("%PREAMBLE%"));
        assert!(root.join("refs.bib").exists() && root.join(".gitignore").exists());
        assert!(create(&root, Template::Report).is_err(), "a second template must not overwrite main.tex");
    }

    /// Every template compiles as created, where a TeX distribution exists.
    #[test]
    fn templates_compile() {
        use crate::backend::latex;
        if !latex::which("pdflatex") {
            return;
        }
        for t in Template::ALL {
            // beamer needs pgf, which minimal installs leave out; ABNT, abnTeX2.
            if (t == Template::Beamer && !latex::installed("pgfmath.sty")) || (t == Template::Abnt && !latex::installed("abntex2.cls")) {
                continue;
            }
            let dir = tempfile::tempdir().unwrap();
            let main = create(dir.path(), t).unwrap();
            let r = latex::compile_system(std::sync::Arc::new(false.into()), &main).unwrap();
            let fatal: Vec<_> = r.errors.iter().filter(|e| !e.warning).collect();
            assert!(r.success, "{t:?}: {fatal:?}\n{}", &r.log_text[r.log_text.len().saturating_sub(1500)..]);
        }
    }
}
