//! LaTeX-aware editing on top of the plain text box: closing brackets and
//! `$` as they are typed, `\end{…}` and `\item` on Enter, indentation that
//! follows the line, block indent/outdent, comment toggling, moving and
//! duplicating lines, and finding the bracket that matches the one at the
//! cursor.
//!
//! Everything here is a pure function over the text and a selection (char
//! indices), returning the edit to apply, so it is tested without a UI.

use regex::Regex;
use std::sync::LazyLock;

/// Replace chars `[from, to)` with `insert`, then select `sel`.
#[derive(Clone, Debug, PartialEq)]
pub struct Edit {
    pub from: usize,
    pub to: usize,
    pub insert: String,
    pub sel: (usize, usize),
}

impl Edit {
    /// Only moves the cursor.
    fn caret(at: usize) -> Self {
        Edit { from: at, to: at, insert: String::new(), sel: (at, at) }
    }

    /// Applies the edit to a `Vec<char>` copy, handy in tests.
    #[cfg(test)]
    fn apply(&self, text: &str) -> String {
        let chars: Vec<char> = text.chars().collect();
        let mut out: String = chars[..self.from].iter().collect();
        out.push_str(&self.insert);
        out.extend(&chars[self.to..]);
        out
    }
}

const INDENT: &str = "  ";

fn closing(c: char) -> Option<char> {
    match c {
        '{' => Some('}'),
        '[' => Some(']'),
        '(' => Some(')'),
        _ => None,
    }
}

/// Whether the char at `i` is escaped by an odd run of backslashes.
fn escaped(chars: &[char], i: usize) -> bool {
    let mut n = 0;
    let mut j = i;
    while j > 0 && chars[j - 1] == '\\' {
        n += 1;
        j -= 1;
    }
    n % 2 == 1
}

/// What may follow an auto-inserted closer without the pair getting in the
/// way: end of text, whitespace or punctuation.
fn pairs_before(next: Option<char>) -> bool {
    match next {
        None => true,
        Some(c) => c.is_whitespace() || matches!(c, ')' | ']' | '}' | '$' | ',' | '.' | ';' | ':' | '~'),
    }
}

/// A single typed char. `None` leaves it to the text box.
pub fn on_char(text: &str, sel: (usize, usize), c: char) -> Option<Edit> {
    let chars: Vec<char> = text.chars().collect();
    let (a, b) = (sel.0.min(sel.1), sel.0.max(sel.1));
    let next = chars.get(b).copied();

    if a != b {
        // Typing an opener over a selection wraps it.
        let close = if c == '$' { Some('$') } else { closing(c) }?;
        let inner: String = chars[a..b].iter().collect();
        return Some(Edit { from: a, to: b, insert: format!("{c}{inner}{close}"), sel: (a + 1, b + 1) });
    }

    // Typing the closer that is already there steps over it.
    if matches!(c, '}' | ']' | ')' | '$') && next == Some(c) && !escaped(&chars, a) {
        return Some(Edit::caret(a + 1));
    }
    if escaped(&chars, a) {
        return None; // `\{`, `\$`: literal characters, no pairing.
    }
    if let Some(close) = closing(c) {
        if !pairs_before(next) {
            return None;
        }
        return Some(Edit { from: a, to: a, insert: format!("{c}{close}"), sel: (a + 1, a + 1) });
    }
    if c == '$' {
        let prev = a.checked_sub(1).map(|i| chars[i]);
        let opens = prev.map(|p| p.is_whitespace() || matches!(p, '(' | '{' | '[' | '~' | '-')).unwrap_or(true);
        if opens && pairs_before(next) {
            return Some(Edit { from: a, to: a, insert: "$$".into(), sel: (a + 1, a + 1) });
        }
    }
    None
}

/// Backspace between an empty pair deletes both halves.
pub fn on_backspace(text: &str, sel: (usize, usize)) -> Option<Edit> {
    if sel.0 != sel.1 || sel.0 == 0 {
        return None;
    }
    let chars: Vec<char> = text.chars().collect();
    let a = sel.0;
    let (prev, next) = (chars[a - 1], chars.get(a).copied()?);
    let pair = matches!((prev, next), ('{', '}') | ('[', ']') | ('(', ')') | ('$', '$'));
    (pair && !escaped(&chars, a - 1)).then(|| Edit { from: a - 1, to: a + 1, insert: String::new(), sel: (a - 1, a - 1) })
}

fn line_start(chars: &[char], i: usize) -> usize {
    chars[..i].iter().rposition(|c| *c == '\n').map(|p| p + 1).unwrap_or(0)
}

fn line_end(chars: &[char], i: usize) -> usize {
    chars[i..].iter().position(|c| *c == '\n').map(|p| i + p).unwrap_or(chars.len())
}

fn indentation(line: &[char]) -> String {
    line.iter().take_while(|c| **c == ' ' || **c == '\t').collect()
}

static BEGIN_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\\begin\{([^}]+)\}").unwrap());
const LIST_ENVS: &[&str] = &["itemize", "enumerate", "description"];

/// Whether an `\end{env}` at `indent` already closes the environment opened
/// just before `from`, a balanced scan, with the indentation telling a
/// nested block's own `\end` from an enclosing one.
fn already_closed(chars: &[char], from: usize, env: &str, indent: &str) -> bool {
    let rest: String = chars[from..].iter().collect();
    let re = Regex::new(&format!(r"\\(begin|end)\{{{}\}}", regex::escape(env))).unwrap();
    let mut depth = 1i32;
    for m in re.captures_iter(&rest) {
        let whole = m.get(0).unwrap();
        if &m[1] == "begin" {
            depth += 1;
            continue;
        }
        depth -= 1;
        if depth == 0 {
            let before = &rest[..whole.start()];
            let line = &before[before.rfind('\n').map(|p| p + 1).unwrap_or(0)..];
            return line.chars().all(|c| c == ' ' || c == '\t') && line == indent;
        }
    }
    false
}

/// Enter: keeps the indentation, closes a `\begin{…}` just typed and
/// continues (or ends) an `\item` list.
pub fn on_enter(text: &str, sel: (usize, usize)) -> Edit {
    let chars: Vec<char> = text.chars().collect();
    let (a, b) = (sel.0.min(sel.1), sel.0.max(sel.1));
    let ls = line_start(&chars, a);
    let le = line_end(&chars, b);
    let before: String = chars[ls..a].iter().collect();
    let after: String = chars[b..le].iter().collect();
    let indent = indentation(&chars[ls..a]);
    let trimmed = before.trim();

    // \begin{env}| → body line plus \end{env}.
    if after.trim().is_empty() {
        if let Some(m) = BEGIN_RE.captures_iter(&before).last() {
            let env = m[1].to_string();
            let closed_on_line = before[m.get(0).unwrap().end()..].contains(&format!("\\end{{{env}}}"));
            if !closed_on_line && !already_closed(&chars, b, &env, &indent) {
                let item = if LIST_ENVS.contains(&env.as_str()) { "\\item " } else { "" };
                let body = format!("\n{indent}{INDENT}{item}");
                let insert = format!("{body}\n{indent}\\end{{{env}}}");
                let at = a + body.chars().count();
                return Edit { from: a, to: b, insert, sel: (at, at) };
            }
        }
    }

    // \item lists: an empty item ends the list, anything else continues it.
    if trimmed.starts_with("\\item") && !trimmed.starts_with("\\items") {
        if trimmed == "\\item" && after.trim().is_empty() {
            // Right before the list's \end: leave the list, below it.
            if le < chars.len() {
                let next_end = line_end(&chars, le + 1);
                let next: String = chars[le + 1..next_end].iter().collect();
                if LIST_ENVS.iter().any(|e| next.trim_start().starts_with(&format!("\\end{{{e}}}"))) {
                    let outer = indentation(&chars[le + 1..next_end]);
                    let insert = format!("{next}\n{outer}");
                    let at = ls + insert.chars().count();
                    return Edit { from: ls, to: next_end, insert, sel: (at, at) };
                }
            }
            let dedent: String = indent.chars().skip(INDENT.len()).collect();
            let at = ls + dedent.chars().count();
            return Edit { from: ls, to: le, insert: dedent, sel: (at, at) };
        }
        let insert = format!("\n{indent}\\item ");
        let at = a + insert.chars().count();
        return Edit { from: a, to: b, insert, sel: (at, at) };
    }

    // An opening brace right before the cursor with its closer right after:
    // open the block onto its own line.
    if before.ends_with('{') && after.starts_with('}') {
        let body = format!("\n{indent}{INDENT}");
        let insert = format!("{body}\n{indent}");
        let at = a + body.chars().count();
        return Edit { from: a, to: b, insert, sel: (at, at) };
    }

    let insert = format!("\n{indent}");
    let at = a + insert.chars().count();
    Edit { from: a, to: b, insert, sel: (at, at) }
}

/// The whole lines a selection touches, as `[start, end)` char indices (end
/// excludes the final newline). A selection ending at column 0 of a line
/// does not take that line.
fn line_block(chars: &[char], sel: (usize, usize)) -> (usize, usize) {
    let (a, b) = (sel.0.min(sel.1), sel.0.max(sel.1));
    let b = if b > a && b > 0 && chars.get(b - 1) == Some(&'\n') { b - 1 } else { b };
    (line_start(chars, a), line_end(chars, b))
}

/// Maps every line of the block through `f`, keeping the selection on the
/// same text.
fn map_lines(text: &str, sel: (usize, usize), f: impl Fn(&str) -> String) -> Edit {
    let chars: Vec<char> = text.chars().collect();
    let (start, end) = line_block(&chars, sel);
    let block: String = chars[start..end].iter().collect();
    let new: String = block.split('\n').map(f).collect::<Vec<_>>().join("\n");
    let delta = new.chars().count() as isize - block.chars().count() as isize;
    let (a, b) = (sel.0.min(sel.1), sel.0.max(sel.1));
    let first_line_delta = {
        let first_old = block.split('\n').next().unwrap_or("").chars().count() as isize;
        let first_new = new.split('\n').next().unwrap_or("").chars().count() as isize;
        first_new - first_old
    };
    let na = (a as isize + first_line_delta).max(start as isize) as usize;
    let nb = if a == b { na } else { (b as isize + delta).max(na as isize) as usize };
    Edit { from: start, to: end, insert: new, sel: (na, nb) }
}

/// Tab: two spaces at the cursor, or indents every selected line.
/// Shift+Tab: outdents the selected lines.
pub fn indent(text: &str, sel: (usize, usize), outdent: bool) -> Edit {
    let multi_line = {
        let chars: Vec<char> = text.chars().collect();
        let (a, b) = (sel.0.min(sel.1), sel.0.max(sel.1));
        chars[a..b].contains(&'\n')
    };
    if !outdent && !multi_line {
        let (a, b) = (sel.0.min(sel.1), sel.0.max(sel.1));
        let at = a + INDENT.len();
        return Edit { from: a, to: b, insert: INDENT.into(), sel: (at, at) };
    }
    map_lines(text, sel, |line| {
        if outdent {
            let n = line.chars().take(INDENT.len()).take_while(|c| *c == ' ').count();
            let n = if n == 0 && line.starts_with('\t') { 1 } else { n };
            line.chars().skip(n).collect()
        } else if line.trim().is_empty() {
            line.to_string()
        } else {
            format!("{INDENT}{line}")
        }
    })
}

/// Ctrl+/: comments the selected lines out with `mark` (`%` in LaTeX,
/// `//` elsewhere) and a space, or back in when every non-blank one already is.
pub fn toggle_comment(text: &str, sel: (usize, usize), mark: &str) -> Edit {
    let chars: Vec<char> = text.chars().collect();
    let (start, end) = line_block(&chars, sel);
    let block: String = chars[start..end].iter().collect();
    let all_commented = block.split('\n').filter(|l| !l.trim().is_empty()).all(|l| l.trim_start().starts_with(mark));
    let min_indent = block
        .split('\n')
        .filter(|l| !l.trim().is_empty())
        .map(|l| l.chars().take_while(|c| *c == ' ' || *c == '\t').count())
        .min()
        .unwrap_or(0);
    map_lines(text, sel, |line| {
        if line.trim().is_empty() {
            return line.to_string();
        }
        if all_commented {
            let lead: String = line.chars().take_while(|c| *c == ' ' || *c == '\t').collect();
            let rest = &line[lead.len()..];
            let rest = rest.strip_prefix(&format!("{mark} ")).or_else(|| rest.strip_prefix(mark)).unwrap_or(rest);
            format!("{lead}{rest}")
        } else {
            let lead: String = line.chars().take(min_indent).collect();
            let rest: String = line.chars().skip(min_indent).collect();
            format!("{lead}{mark} {rest}")
        }
    })
}

/// Alt+↑/↓: swaps the selected lines with the one above/below.
pub fn move_lines(text: &str, sel: (usize, usize), up: bool) -> Option<Edit> {
    let chars: Vec<char> = text.chars().collect();
    let (start, end) = line_block(&chars, sel);
    let block: String = chars[start..end].iter().collect();
    let (a, b) = (sel.0.min(sel.1), sel.0.max(sel.1));
    if up {
        if start == 0 {
            return None;
        }
        let prev_start = line_start(&chars, start - 1);
        let prev: String = chars[prev_start..start - 1].iter().collect();
        let shift = prev.chars().count() + 1;
        Some(Edit { from: prev_start, to: end, insert: format!("{block}\n{prev}"), sel: (a - shift, b - shift) })
    } else {
        if end >= chars.len() {
            return None;
        }
        let next_end = line_end(&chars, end + 1);
        let next: String = chars[end + 1..next_end].iter().collect();
        let shift = next.chars().count() + 1;
        Some(Edit { from: start, to: next_end, insert: format!("{next}\n{block}"), sel: (a + shift, b + shift) })
    }
}

/// Ctrl+Shift+D: duplicates the selected lines below themselves.
pub fn duplicate_lines(text: &str, sel: (usize, usize)) -> Edit {
    let chars: Vec<char> = text.chars().collect();
    let (start, end) = line_block(&chars, sel);
    let block: String = chars[start..end].iter().collect();
    let shift = block.chars().count() + 1;
    let (a, b) = (sel.0.min(sel.1), sel.0.max(sel.1));
    Edit { from: end, to: end, insert: format!("\n{block}"), sel: (a + shift, b + shift) }
}

/// How far the bracket search looks, so a huge document stays cheap.
const MATCH_REACH: usize = 20_000;

/// The bracket pair touching the cursor (the char before it, else the one
/// after), as the char indices of both brackets.
pub fn matching_bracket(chars: &[char], cursor: usize) -> Option<(usize, usize)> {
    let candidates = [cursor.checked_sub(1), Some(cursor)];
    for i in candidates.into_iter().flatten() {
        let Some(&c) = chars.get(i) else { continue };
        if escaped(chars, i) {
            continue;
        }
        let (open, close, forward) = match c {
            '{' => ('{', '}', true),
            '[' => ('[', ']', true),
            '(' => ('(', ')', true),
            '}' => ('{', '}', false),
            ']' => ('[', ']', false),
            ')' => ('(', ')', false),
            _ => continue,
        };
        let mut depth = 0i32;
        if forward {
            for (j, &d) in chars.iter().enumerate().skip(i).take(MATCH_REACH) {
                if escaped(chars, j) {
                    continue;
                }
                if d == open {
                    depth += 1;
                } else if d == close {
                    depth -= 1;
                    if depth == 0 {
                        return Some((i, j));
                    }
                }
            }
        } else {
            let lo = i.saturating_sub(MATCH_REACH);
            for j in (lo..=i).rev() {
                let d = chars[j];
                if escaped(chars, j) {
                    continue;
                }
                if d == close {
                    depth += 1;
                } else if d == open {
                    depth -= 1;
                    if depth == 0 {
                        return Some((j, i));
                    }
                }
            }
        }
        return None;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `|` marks the cursor (or `[`…`]`-free `«…»` a selection) in these
    /// fixtures.
    fn split(s: &str) -> (String, (usize, usize)) {
        if let (Some(a), Some(b)) = (s.find('«'), s.find('»')) {
            let text = s.replace(['«', '»'], "");
            let a = s[..a].chars().count();
            let b = s[..b].chars().count() - 1;
            return (text, (a, b));
        }
        let at = s[..s.find('|').unwrap()].chars().count();
        (s.replacen('|', "", 1), (at, at))
    }

    fn join(text: &str, sel: (usize, usize)) -> String {
        let mut chars: Vec<char> = text.chars().collect();
        if sel.0 == sel.1 {
            chars.insert(sel.0, '|');
        } else {
            chars.insert(sel.1, '»');
            chars.insert(sel.0, '«');
        }
        chars.into_iter().collect()
    }

    fn run(f: impl Fn(&str, (usize, usize)) -> Option<Edit>, fixture: &str) -> Option<String> {
        let (text, sel) = split(fixture);
        f(&text, sel).map(|e| join(&e.apply(&text), e.sel))
    }

    #[test]
    fn pairs_brackets() {
        assert_eq!(run(|t, s| on_char(t, s, '{'), "\\textbf|").as_deref(), Some("\\textbf{|}"));
        assert_eq!(run(|t, s| on_char(t, s, '('), "a | b").as_deref(), Some("a (|) b"));
        // Not in front of a word, not after a backslash.
        assert_eq!(run(|t, s| on_char(t, s, '{'), "|abc"), None);
        assert_eq!(run(|t, s| on_char(t, s, '{'), "\\|"), None);
    }

    #[test]
    fn steps_over_closers() {
        assert_eq!(run(|t, s| on_char(t, s, '}'), "\\emph{x|}").as_deref(), Some("\\emph{x}|"));
        assert_eq!(run(|t, s| on_char(t, s, '$'), "$x|$").as_deref(), Some("$x$|"));
        assert_eq!(run(|t, s| on_char(t, s, '}'), "x|"), None);
    }

    #[test]
    fn pairs_dollars_only_at_word_boundaries() {
        assert_eq!(run(|t, s| on_char(t, s, '$'), "seja | tal").as_deref(), Some("seja $|$ tal"));
        assert_eq!(run(|t, s| on_char(t, s, '$'), "R|"), None);
        assert_eq!(run(|t, s| on_char(t, s, '$'), "custa \\|"), None);
    }

    #[test]
    fn wraps_selection() {
        assert_eq!(run(|t, s| on_char(t, s, '$'), "a «x+1» b").as_deref(), Some("a $«x+1»$ b"));
        assert_eq!(run(|t, s| on_char(t, s, '{'), "«abc»").as_deref(), Some("{«abc»}"));
        assert_eq!(run(|t, s| on_char(t, s, 'x'), "«abc»"), None);
    }

    #[test]
    fn backspace_deletes_empty_pairs() {
        assert_eq!(run(on_backspace, "\\emph{|}").as_deref(), Some("\\emph|"));
        assert_eq!(run(on_backspace, "$|$").as_deref(), Some("|"));
        assert_eq!(run(on_backspace, "{a|}"), None);
        assert_eq!(run(on_backspace, "\\{|}"), None);
    }

    #[test]
    fn enter_closes_environments() {
        let e = |f: &str| run(|t, s| Some(on_enter(t, s)), f).unwrap();
        assert_eq!(e("\\begin{equation}|"), "\\begin{equation}\n  |\n\\end{equation}");
        assert_eq!(e("  \\begin{itemize}|"), "  \\begin{itemize}\n    \\item |\n  \\end{itemize}");
        // Already closed below at the same indentation: just a new line.
        assert_eq!(e("\\begin{center}|\n\\end{center}"), "\\begin{center}\n|\n\\end{center}");
        // A nested block is closed even though the outer \end follows.
        assert_eq!(
            e("\\begin{itemize}\n  \\begin{itemize}|\n\\end{itemize}"),
            "\\begin{itemize}\n  \\begin{itemize}\n    \\item |\n  \\end{itemize}\n\\end{itemize}"
        );
        // Closed on the same line already.
        assert_eq!(e("\\begin{x}a\\end{x}|"), "\\begin{x}a\\end{x}\n|");
    }

    #[test]
    fn enter_continues_and_ends_lists() {
        let e = |f: &str| run(|t, s| Some(on_enter(t, s)), f).unwrap();
        assert_eq!(e("  \\item um|"), "  \\item um\n  \\item |");
        assert_eq!(e("    \\item|"), "  |");
        // The empty last item leaves the list.
        assert_eq!(
            e("\\begin{itemize}\n  \\item a\n  \\item|\n\\end{itemize}\nfim"),
            "\\begin{itemize}\n  \\item a\n\\end{itemize}\n|\nfim"
        );
        assert_eq!(e("\tsem nada|"), "\tsem nada\n\t|");
        assert_eq!(e("\\section{|}"), "\\section{\n  |\n}");
    }

    #[test]
    fn indents_blocks() {
        assert_eq!(run(|t, s| Some(indent(t, s, false)), "a|b").as_deref(), Some("a  |b"));
        assert_eq!(run(|t, s| Some(indent(t, s, false)), "«a\nb»").as_deref(), Some("  «a\n  b»"));
        assert_eq!(run(|t, s| Some(indent(t, s, true)), "  a|\n  b").as_deref(), Some("a|\n  b"));
        assert_eq!(run(|t, s| Some(indent(t, s, true)), "«  a\n b»").as_deref(), Some("«a\nb»"));
    }

    #[test]
    fn toggles_comments() {
        assert_eq!(run(|t, s| Some(toggle_comment(t, s, "%")), "  «a\n  b»").as_deref(), Some("  % «a\n  % b»"));
        assert_eq!(run(|t, s| Some(toggle_comment(t, s, "%")), "% a|").as_deref(), Some("a|"));
        assert_eq!(run(|t, s| Some(toggle_comment(t, s, "%")), "«%a\n\n  % b»").as_deref(), Some("«a\n\n  b»"));
        assert_eq!(run(|t, s| Some(toggle_comment(t, s, "//")), "«a\nb»").as_deref(), Some("// «a\n// b»"));
        assert_eq!(run(|t, s| Some(toggle_comment(t, s, "//")), "//a|").as_deref(), Some("a|"));
    }

    #[test]
    fn moves_and_duplicates_lines() {
        assert_eq!(run(|t, s| move_lines(t, s, true), "um\ndo|is\ntrês").as_deref(), Some("do|is\num\ntrês"));
        assert_eq!(run(|t, s| move_lines(t, s, false), "um\ndo|is\ntrês").as_deref(), Some("um\ntrês\ndo|is"));
        assert_eq!(run(|t, s| move_lines(t, s, true), "u|m\ndois"), None);
        assert_eq!(run(|t, s| move_lines(t, s, false), "um\ndo|is"), None);
        assert_eq!(run(|t, s| Some(duplicate_lines(t, s)), "a|b\nc").as_deref(), Some("ab\na|b\nc"));
    }

    #[test]
    fn matches_brackets() {
        let chars: Vec<char> = "\\frac{a}{b{c}}".chars().collect();
        assert_eq!(matching_bracket(&chars, 6), Some((5, 7)));
        assert_eq!(matching_bracket(&chars, 14), Some((8, 13)));
        let chars: Vec<char> = "\\{ a }".chars().collect();
        assert_eq!(matching_bracket(&chars, 2), None);
    }
}
