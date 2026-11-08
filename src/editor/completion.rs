//! What to complete, decided from the text to the left of the cursor on the
//! current line, and the ranked options for it: LaTeX's own commands and
//! environments (`latex_db`), plus everything the project index knows:
//! labels, citations, figures, packages and the project's own macros.

use super::latex_db;
use crate::backend::index::Index;
use crate::i18n::t;
use crate::settings::Snippet;
use crate::tf;
use regex::Regex;
use std::sync::LazyLock;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Ctx {
    Label,
    Cite,
    Graphic,
    Package,
    Begin,
    End,
    Command,
}

pub struct Target {
    pub kind: Ctx,
    /// What was already typed (the part the completion replaces).
    pub typed: String,
    /// The command whose argument is being completed (`eqref`, `cite`…).
    pub command: String,
}

// The last capture group is always what was typed; `cmd` names the command
// where it matters.
static CONTEXTS: LazyLock<Vec<(Ctx, Regex, bool)>> = LazyLock::new(|| {
    vec![
        (Ctx::Label, Regex::new(r"\\(?P<cmd>(?:auto|page|name|eq|c|C)?refs?)\*?\s*\{([^}]*)$").unwrap(), true),
        (
            Ctx::Cite,
            Regex::new(r"\\(?P<cmd>(?:no|paren|text|foot|super|auto)?cite[a-zA-Z]*)\*?\s*(?:\[[^\]]*\]\s*)*\{([^}]*)$").unwrap(),
            true,
        ),
        (Ctx::Graphic, Regex::new(r"\\include(?:graphics|pdf)\s*(?:\[[^\]]*\]\s*)?\{([^}]*)$").unwrap(), false),
        (Ctx::Package, Regex::new(r"\\(?:usepackage|RequirePackage)\s*(?:\[[^\]]*\]\s*)?\{([^}]*)$").unwrap(), true),
        (Ctx::Begin, Regex::new(r"\\begin\s*\{([^}]*)$").unwrap(), false),
        (Ctx::End, Regex::new(r"\\end\s*\{([^}]*)$").unwrap(), false),
        (Ctx::Command, Regex::new(r"(\\[a-zA-Z]*)$").unwrap(), false),
    ]
});

pub fn context_at(before: &str) -> Option<Target> {
    for (kind, re, multiple) in CONTEXTS.iter() {
        let Some(m) = re.captures(before) else { continue };
        let mut typed = m.get(m.len() - 1).map(|g| g.as_str()).unwrap_or("").to_string();
        if *multiple {
            if let Some(comma) = typed.rfind(',') {
                typed = typed[comma + 1..].to_string();
            }
        }
        let command = m.name("cmd").map(|c| c.as_str().to_string()).unwrap_or_default();
        return Some(Target { kind: *kind, typed: typed.trim_start().to_string(), command });
    }
    None
}

#[derive(Clone, Debug)]
pub enum Apply {
    /// Replace the typed text with this.
    Text(String),
    /// Insert the text and put the cursor this many chars into it.
    Template(String, usize),
    /// `\begin{name}` … `\end{name}` around a body template (see
    /// `latex_db::ENVIRONMENTS`).
    Environment(String, String),
    /// `\end{name}`, eating the `}` already there.
    EndName(String),
}

/// What an option is, for its icon.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Command,
    Math,
    Environment,
    Label,
    Citation,
    File,
    Package,
    Snippet,
    /// Defined by the project itself.
    Project,
}

#[derive(Clone, Debug)]
pub struct Item {
    pub label: String,
    pub detail: String,
    /// Longer text shown for the selected option.
    pub info: String,
    pub apply: Apply,
    pub kind: Kind,
}

/// `\section{|}` → (`\section{}`, cursor offset in chars).
pub fn split_template(template: &str) -> (String, usize) {
    match template.find('|') {
        Some(b) => (template.replacen('|', "", 1), template[..b].chars().count()),
        None => (template.to_string(), template.chars().count()),
    }
}

/// The trigger a template shows up as: `\section{}` is listed as `\section`.
fn trigger_of(text: &str) -> String {
    let end = text[1..].find(|c: char| !c.is_ascii_alphabetic() && c != '*').map(|i| i + 1).unwrap_or(text.len());
    text[..end].to_string()
}

pub fn builtin_triggers() -> Vec<String> {
    latex_db::COMMANDS.iter().map(|(t, _, _)| trigger_of(&split_template(t).0)).collect()
}

/// Whether a command belongs in math mode: for the icon only.
fn is_math(description: &str, package: &str, template: &str) -> bool {
    // A description that is just the glyph (α, ≤, ∑…): short, no ASCII letters.
    let symbol = description.chars().count() <= 3 && !description.chars().any(|c| c.is_ascii_alphabetic());
    symbol || package == "amssymb" || template.starts_with("\\math") || template.contains("_{") || template.starts_with("\\frac")
}

/// Every option the autocomplete knows, refreshed from the project index,
/// the installed packages and the user's snippets.
#[derive(Default)]
pub struct Completions {
    labels: Vec<Item>,
    citations: Vec<Item>,
    graphics: Vec<Item>,
    packages: Vec<Item>,
    commands: Vec<Item>,
    project_commands: Vec<Item>,
    environments: Vec<(String, String, String, bool)>,
    project_environments: Vec<(String, String, String, bool)>,
}

impl Completions {
    pub fn new() -> Self {
        let mut c = Self::default();
        c.set_snippets(&[]);
        c.retranslate();
        c
    }

    /// The built-in environments' descriptions, in the current language.
    pub fn retranslate(&mut self) {
        self.environments = latex_db::ENVIRONMENTS.iter().map(|(n, b, d)| (n.to_string(), b.to_string(), t(d).to_string(), false)).collect();
    }

    pub fn set_index(&mut self, index: &Index) {
        let file_name = |p: &std::path::Path| crate::backend::project::file_name(p);
        self.labels = index
            .labels
            .iter()
            .map(|l| Item {
                label: l.name.clone(),
                detail: l.context.clone(),
                info: format!("{}:{}", file_name(&l.file), l.line),
                apply: Apply::Text(l.name.clone()),
                kind: Kind::Label,
            })
            .collect();
        self.citations = index
            .citations
            .iter()
            .map(|c| Item {
                label: c.key.clone(),
                detail: [c.author.as_str(), c.year.as_str()].iter().filter(|s| !s.is_empty()).cloned().collect::<Vec<_>>().join(", "),
                info: if c.title.is_empty() { c.kind.clone() } else { c.title.clone() },
                apply: Apply::Text(c.key.clone()),
                kind: Kind::Citation,
            })
            .collect();
        self.graphics = index
            .graphics
            .iter()
            .map(|g| Item {
                label: g.path.clone(),
                detail: if g.kind == crate::backend::project::Kind::Pdf { "pdf".into() } else { t("image").into() },
                info: String::new(),
                apply: Apply::Text(g.path.clone()),
                kind: Kind::File,
            })
            .collect();
        self.project_commands = index
            .macros
            .iter()
            .filter(|m| !m.environment)
            .map(|m| {
                let args = "{}".repeat(m.args.saturating_sub(1));
                let (text, cursor) = if m.args > 0 {
                    (format!("{}{{}}{args}", m.name), m.name.chars().count() + 1)
                } else {
                    (m.name.clone(), m.name.chars().count())
                };
                Item {
                    label: m.name.clone(),
                    detail: if m.args > 0 { tf!("from the project · {n} arg.", n = m.args) } else { t("from the project").into() },
                    info: tf!("defined in {place}", place = format!("{}:{}", file_name(&m.file), m.line)),
                    apply: Apply::Template(text, cursor),
                    kind: Kind::Project,
                }
            })
            .collect();
        self.project_environments = index
            .macros
            .iter()
            .filter(|m| m.environment)
            .map(|m| {
                let body = if m.args > 0 { format!("{}\n", "{}".repeat(m.args)).replacen("{}", "{|}", 1) } else { "|".into() };
                (m.name.clone(), body, format!("{} · {}:{}", t("from the project"), file_name(&m.file), m.line), true)
            })
            .collect();
    }

    pub fn set_packages(&mut self, list: &[String]) {
        self.packages = list
            .iter()
            .map(|p| Item { label: p.clone(), detail: String::new(), info: String::new(), apply: Apply::Text(p.clone()), kind: Kind::Package })
            .collect();
    }

    pub fn set_snippets(&mut self, snippets: &[Snippet]) {
        let mut commands: Vec<Item> = latex_db::COMMANDS
            .iter()
            .map(|(template, detail, package)| {
                let (text, cursor) = split_template(template);
                let info = if package.is_empty() { text.clone() } else { format!("{text} ({})", tf!("package {package}", package)) };
                Item {
                    label: trigger_of(&text),
                    detail: t(detail).to_string(),
                    info,
                    apply: Apply::Template(text.clone(), cursor),
                    kind: if is_math(detail, package, template) { Kind::Math } else { Kind::Command },
                }
            })
            .collect();
        // One entry per trigger: the first template wins (`\caption{}` over
        // `\caption[]{}`), the others stay reachable by typing more.
        let mut seen = std::collections::HashSet::new();
        commands.retain(|c| seen.insert((c.label.clone(), c.detail.clone())));
        for s in snippets {
            let (text, cursor) = split_template(&s.body);
            let detail = if s.description.is_empty() { t("custom snippet").to_string() } else { s.description.clone() };
            commands.insert(0, Item { label: s.trigger.clone(), detail, info: text.clone(), apply: Apply::Template(text, cursor), kind: Kind::Snippet });
        }
        self.commands = commands;
    }

    fn environment_items(&self, end: bool) -> Vec<Item> {
        self.project_environments
            .iter()
            .chain(self.environments.iter())
            .map(|(name, body, desc, project)| Item {
                label: name.clone(),
                detail: desc.clone(),
                info: String::new(),
                apply: if end { Apply::EndName(name.clone()) } else { Apply::Environment(name.clone(), body.clone()) },
                kind: if *project { Kind::Project } else { Kind::Environment },
            })
            .collect()
    }

    /// Options for a context, ranked by how well they match what was typed.
    /// `None` when there is nothing to offer.
    pub fn options(&self, target: &Target, explicit: bool) -> Option<Vec<Item>> {
        let typed = target.typed.to_lowercase();
        if typed.is_empty() && !explicit && !matches!(target.kind, Ctx::Begin | Ctx::End) {
            return None;
        }
        // A bare backslash (or `#`) is not worth a popup unless asked for.
        if matches!(target.kind, Ctx::Command) && typed == "\\" && !explicit {
            return None;
        }
        let list: Vec<Item> = match target.kind {
            Ctx::Label => {
                // \eqref wants equations first, and so on.
                let prefix = match target.command.as_str() {
                    "eqref" => "eq:",
                    _ => "",
                };
                let mut l = self.labels.clone();
                if !prefix.is_empty() {
                    l.sort_by_key(|i| !i.label.starts_with(prefix));
                }
                l
            }
            Ctx::Cite => self.citations.clone(),
            Ctx::Graphic => self.graphics.clone(),
            Ctx::Package => self.packages.clone(),
            Ctx::Command => self.project_commands.iter().chain(self.commands.iter()).cloned().collect(),
            Ctx::Begin => self.environment_items(false),
            Ctx::End => self.environment_items(true),
        };
        let mut scored: Vec<(i32, usize, Item)> = list
            .into_iter()
            .enumerate()
            .filter_map(|(order, item)| {
                let mut s = score(&item.label.to_lowercase(), &typed);
                if matches!(target.kind, Ctx::Cite) {
                    // Inserting a citation is a search: key, author/year or title.
                    let hay = format!("{} {}", item.detail, item.info).to_lowercase();
                    if s.is_none() && !typed.is_empty() && hay.contains(&typed) {
                        s = Some(50);
                    }
                }
                s.map(|s| (s, order, item))
            })
            .collect();
        scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
        let matched: Vec<Item> = scored.into_iter().map(|(_, _, i)| i).collect();
        // An exact, complete match leaves nothing to complete.
        if matched.len() == 1 && matched[0].label.to_lowercase() == typed && matches!(target.kind, Ctx::Command) {
            return None;
        }
        (!matched.is_empty()).then_some(matched)
    }
}

/// How well `label` matches `typed` (both lowercase): a prefix beats a
/// substring, which beats a scattered subsequence (`\sse` → `\subsection`).
/// Shorter labels win ties so `\sec` offers `\section` before
/// `\sectionmark`.
pub fn score(label: &str, typed: &str) -> Option<i32> {
    if typed.is_empty() {
        return Some(0);
    }
    let len_penalty = label.chars().count() as i32;
    if label.starts_with(typed) {
        return Some(3000 - len_penalty);
    }
    let bare = typed.trim_start_matches('\\');
    if bare.chars().count() < 2 {
        return None;
    }
    if let Some(pos) = label.find(bare) {
        return Some(2000 - pos as i32 * 4 - len_penalty);
    }
    // Subsequence, anchored at the first letter after the backslash.
    let label_bare = label.trim_start_matches('\\');
    let mut chars = label_bare.chars().enumerate();
    let mut first = true;
    let mut last = 0usize;
    let mut gaps = 0i32;
    for t in bare.chars() {
        let found = chars.by_ref().find(|(_, c)| *c == t)?;
        if first && found.0 != 0 {
            return None;
        }
        if !first && found.0 > last + 1 {
            gaps += 1;
        }
        first = false;
        last = found.0;
    }
    Some(1000 - gaps * 20 - len_penalty)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn labels(opts: &[Item]) -> Vec<&str> {
        opts.iter().map(|o| o.label.as_str()).collect()
    }

    #[test]
    fn contexts() {
        let t = context_at("see \\eqref{eq:").unwrap();
        assert_eq!((t.kind, t.typed.as_str(), t.command.as_str()), (Ctx::Label, "eq:", "eqref"));
        let t = context_at("\\cite[p. 2]{a, kn").unwrap();
        assert_eq!((t.kind, t.typed.as_str()), (Ctx::Cite, "kn"));
        assert_eq!(context_at("\\begin{ite").unwrap().kind, Ctx::Begin);
        assert_eq!(context_at("text \\sec").unwrap().typed, "\\sec");
        assert!(context_at("plain text").is_none());
    }

    #[test]
    fn templates() {
        assert_eq!(split_template("\\frac{|}{}"), ("\\frac{}{}".into(), 6));
        assert_eq!(trigger_of("\\section*{}"), "\\section*");
        assert_eq!(trigger_of("\\includegraphics[width=\\linewidth]{}"), "\\includegraphics");
    }

    #[test]
    fn ranks_prefix_then_substring_then_subsequence() {
        let c = Completions::new();
        let opts = c.options(&Target { kind: Ctx::Command, typed: "\\sec".into(), command: String::new() }, false).unwrap();
        assert_eq!(opts[0].label, "\\section");
        let opts = c.options(&Target { kind: Ctx::Command, typed: "\\ssec".into(), command: String::new() }, false).unwrap();
        assert!(labels(&opts).contains(&"\\subsection"), "{:?}", labels(&opts));
        let opts = c.options(&Target { kind: Ctx::Command, typed: "\\arrow".into(), command: String::new() }, false).unwrap();
        assert!(labels(&opts).contains(&"\\rightarrow"));
        assert_eq!(score("\\alpha", "\\zz"), None);
    }

    #[test]
    fn project_macros_and_environments() {
        use crate::backend::index::MacroEntry;
        let mut c = Completions::new();
        let entry = |name: &str, args, environment| MacroEntry { name: name.into(), args, environment, file: "p.tex".into(), line: 1 };
        let index = Index { macros: vec![entry("\\norm", 1, false), entry("teorema", 0, true)], ..Default::default() };
        c.set_index(&index);
        let opts = c.options(&Target { kind: Ctx::Command, typed: "\\nor".into(), command: String::new() }, false).unwrap();
        assert_eq!(opts[0].label, "\\norm");
        assert!(matches!(&opts[0].apply, Apply::Template(t, 6) if t == "\\norm{}"));
        let opts = c.options(&Target { kind: Ctx::Begin, typed: "teo".into(), command: String::new() }, false).unwrap();
        assert_eq!((opts[0].label.as_str(), opts[0].kind), ("teorema", Kind::Project));
    }

    #[test]
    fn eqref_prefers_equations() {
        use crate::backend::index::LabelEntry;
        let mut c = Completions::new();
        let label = |n: &str| LabelEntry { name: n.into(), file: "a.tex".into(), line: 1, context: String::new() };
        c.set_index(&Index { labels: vec![label("fig:a"), label("eq:b")], ..Default::default() });
        let opts = c.options(&Target { kind: Ctx::Label, typed: String::new(), command: "eqref".into() }, true).unwrap();
        assert_eq!(opts[0].label, "eq:b");
    }
}
