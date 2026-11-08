//! Primitives for reading LaTeX *source*: where a comment starts and where a
//! command's argument ends. Shared by `index` and `outline`, which walk the
//! same files for different reasons.
//!
//! Not a LaTeX parser: no macro expansion, no verbatim, no `\catcode`.

use regex::Regex;
use std::sync::LazyLock;

static COMMAND_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\\[a-zA-Z]+\s*").unwrap());
static ESCAPED_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\\([&%$#_{}])").unwrap());
static LOOSE_BACKSLASH_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\\[^a-zA-Z]").unwrap());

/// Cuts whatever comes after an unescaped `%`.
///
/// "Unescaped" is decided by the *parity* of the backslashes right before it:
/// `\%` is a literal percent, but `\\%` is a line break followed by a comment.
pub fn without_comment(line: &str) -> &str {
    let bytes = line.as_bytes();
    for i in 0..bytes.len() {
        if bytes[i] != b'%' {
            continue;
        }
        let mut backslashes = 0;
        let mut j = i;
        while j > 0 && bytes[j - 1] == b'\\' {
            backslashes += 1;
            j -= 1;
        }
        if backslashes % 2 == 0 {
            return &line[..i];
        }
    }
    line
}

/// Closes a command's argument at the first `}` of the right level and strips
/// the most common title markup. `rest` is what comes *after* the opening
/// brace; an argument that never closes returns everything it was given.
pub fn argument_text(rest: &str) -> String {
    let mut level = 1;
    let mut end = rest.len();
    for (i, b) in rest.bytes().enumerate() {
        match b {
            b'{' => level += 1,
            b'}' => {
                level -= 1;
                if level == 0 {
                    end = i;
                    break;
                }
            }
            _ => {}
        }
    }
    let text = &rest[..end];
    let text = ESCAPED_RE.replace_all(text, "$1");
    let text = COMMAND_RE.replace_all(&text, "");
    let text = LOOSE_BACKSLASH_RE.replace_all(&text, "");
    let text: String = text
        .chars()
        .filter(|c| *c != '{' && *c != '}')
        .map(|c| if c == '~' { ' ' } else { c })
        .collect();
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn comment_parity() {
        assert_eq!(without_comment(r"a % b"), "a ");
        assert_eq!(without_comment(r"50\% off"), r"50\% off");
        assert_eq!(without_comment(r"row \\% note"), r"row \\");
    }

    #[test]
    fn argument() {
        assert_eq!(argument_text(r"Intro \emph{to} it} rest"), "Intro to it");
        assert_eq!(argument_text(r"A \& B}"), "A & B");
        assert_eq!(argument_text(r"gr\'afico}"), "grafico");
        assert_eq!(argument_text("never closes"), "never closes");
    }

    #[test]
    fn without_comment_cases() {
        for (inp, want) in [
            ("texto simples", "texto simples"),
            ("texto % comentário", "texto "),
            ("% a linha inteira", ""),
            (r"100\% de acerto", r"100\% de acerto"),
            (r"100\% de acerto % mas isto sai", r"100\% de acerto "),
            ("", ""),
            // `\\` ends a line in LaTeX, so the `%` after it is a comment.
            (r"fim \\% comentário", r"fim \\"),
        ] {
            assert_eq!(without_comment(inp), want, "{inp:?}");
        }
    }

    #[test]
    fn argument_text_closes_at_the_right_brace() {
        for (inp, want) in [
            ("Simples}", "Simples"),
            ("Simples} e o resto da linha", "Simples"),
            ("Com {um grupo} dentro} e resto", "Com um grupo dentro"),
            ("Aninhado {a {b} c} fim} resto", "Aninhado a b c fim"),
            ("Sem fechar", "Sem fechar"),
            ("}", ""),
        ] {
            assert_eq!(argument_text(inp), want, "{inp:?}");
        }
    }

    #[test]
    fn argument_text_strips_markup() {
        for (inp, want) in [
            (r"Sobre \emph{isto}}", "Sobre isto"),
            (r"\textbf{Negrito} e \textit{itálico}}", "Negrito e itálico"),
            (r"Custo \& benefício}", "Custo & benefício"),
            (r"50\% e R\$10 e a\_b}", "50% e R$10 e a_b"),
            (r"Espa\c{c}os}", "Espacos"),
            ("Muitos      espaços     seguidos}", "Muitos espaços seguidos"),
            ("Sem~quebra}", "Sem quebra"),
            ("   Aparado   }", "Aparado"),
        ] {
            assert_eq!(argument_text(inp), want, "{inp:?}");
        }
    }
}
