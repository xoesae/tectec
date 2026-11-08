//! The interface language: English (the source every message is written in)
//! or Brazilian Portuguese (`assets/i18n/pt-BR.po`, compiled into the binary
//! by `build.rs`). One language for the whole process, switched from the
//! menu, so backend errors come out in the same language as the window.
//!
//! - `t("Open project…")` for a fixed phrase;
//! - `tf!("Delete {name}?", name)` or `tf!("{n} of {total}", n = i + 1, total)`
//!   for one with values, named so a translation can reorder them;
//! - `tn("{n} error", "{n} errors", count)` for one that depends on a count;
//! - `n("…")` marks a phrase kept in a table, translated where it is shown
//!   with `t`.

use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{LazyLock, Mutex};

use serde::{Deserialize, Serialize};

include!(concat!(env!("OUT_DIR"), "/catalogs.rs"));

/// The setting: follow the system, or a fixed language.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Language {
    #[default]
    System,
    English,
    Portuguese,
}

impl Language {
    /// The menu entries, each in its own language so anyone can find theirs.
    pub const CHOICES: [(Language, &'static str); 2] = [(Language::English, "English"), (Language::Portuguese, "Português")];
}

/// A language with its catalog.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Locale {
    English,
    Portuguese,
}

impl fastframe_i18n::Locale for Locale {
    fn catalog(self) -> Option<&'static dyn fastframe_i18n::Translator> {
        match self {
            Self::English => None,
            Self::Portuguese => Some(&pt_br::Translator),
        }
    }
}

impl Locale {
    fn from_tag(tag: &fastframe_i18n::LanguageTag) -> Option<Locale> {
        match tag.language.as_str() {
            "pt" => Some(Locale::Portuguese),
            "en" => Some(Locale::English),
            _ => None,
        }
    }
}

/// The current locale; English until `set` runs, which is what tests see.
static CURRENT: AtomicU8 = AtomicU8::new(0);

/// Switches the interface language. Takes effect on the next frame.
pub fn set(language: Language) {
    let locale = match language {
        Language::English => Locale::English,
        Language::Portuguese => Locale::Portuguese,
        Language::System => fastframe_i18n::detect(Locale::from_tag).unwrap_or(Locale::English),
    };
    CURRENT.store(locale as u8, Ordering::Relaxed);
}

pub fn locale() -> Locale {
    if CURRENT.load(Ordering::Relaxed) == Locale::Portuguese as u8 { Locale::Portuguese } else { Locale::English }
}

/// Catalog strings are static; a translation that ever comes back owned is
/// kept once, so `t` can hand out `&'static str` like the literal it replaces.
fn keep(text: Cow<'static, str>) -> &'static str {
    static KEPT: LazyLock<Mutex<HashMap<String, &'static str>>> = LazyLock::new(Default::default);
    match text {
        Cow::Borrowed(s) => s,
        Cow::Owned(s) => {
            let mut kept = KEPT.lock().unwrap_or_else(|e| e.into_inner());
            kept.entry(s.clone()).or_insert_with(|| Box::leak(s.into_boxed_str()))
        }
    }
}

/// Translates a fixed phrase.
pub fn t(source: &'static str) -> &'static str {
    keep(fastframe_i18n::gettext(locale(), source))
}

/// Translates a phrase for `count`, with `{n}` replaced by it.
pub fn tn(singular: &'static str, plural: &'static str, count: usize) -> String {
    let count32 = u32::try_from(count).unwrap_or(u32::MAX);
    fastframe_i18n::ngettext(locale(), singular, plural, count32).replace("{n}", &count.to_string())
}

/// A count with the language's thousands separator: 12,345 or 12.345.
pub fn number(n: usize) -> String {
    let separator = if locale() == Locale::Portuguese { '.' } else { ',' };
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(separator);
        }
        out.push(c);
    }
    out
}

/// Marks a phrase kept in a table for translation; `t` translates it where
/// it is shown.
pub const fn n(source: &'static str) -> &'static str {
    source
}

/// Translates a phrase and fills its `{name}` placeholders:
/// `tf!("Delete {name}?", name)` or `tf!("Page {page}", page = i + 1)`.
#[macro_export]
macro_rules! tf {
    ($source:literal $(, $key:ident $(= $value:expr)?)* $(,)?) => {{
        #[allow(unused_mut)]
        let mut text = ::std::string::String::from($crate::i18n::t($source));
        $( text = text.replace(concat!("{", stringify!($key), "}"), &$crate::tf!(@value $key $(= $value)?).to_string()); )*
        text
    }};
    (@value $key:ident = $value:expr) => { $value };
    (@value $key:ident) => { $key };
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{BTreeMap, BTreeSet};

    /// The messages of a PO file: msgid (as written, escapes kept) → whether
    /// every msgstr is filled in.
    fn catalog(text: &str) -> BTreeMap<String, bool> {
        let mut out = BTreeMap::new();
        let (mut id, mut strs, mut field): (Option<String>, Vec<String>, &str) = (None, vec![], "");
        let mut flush = |id: &mut Option<String>, strs: &mut Vec<String>| {
            if let Some(id) = id.take().filter(|id| !id.is_empty()) {
                out.insert(id, !strs.is_empty() && strs.iter().all(|s| !s.is_empty()));
            }
            strs.clear();
        };
        // Exactly one quote off each end: an escaped \" can end the text.
        let unquote = |s: &str| {
            let s = s.trim();
            s.strip_prefix('"').and_then(|s| s.strip_suffix('"')).unwrap_or(s).to_string()
        };
        for line in text.lines() {
            let line = line.trim();
            if line.starts_with("msgid_plural") {
                field = "plural";
            } else if let Some(rest) = line.strip_prefix("msgid ") {
                flush(&mut id, &mut strs);
                id = Some(unquote(rest));
                field = "id";
            } else if line.starts_with("msgstr") {
                let rest = line.split_once(' ').map(|(_, r)| r).unwrap_or("");
                strs.push(unquote(rest));
                field = "str";
            } else if line.starts_with('"') {
                match field {
                    "id" => id.get_or_insert_default().push_str(&unquote(line)),
                    "str" => strs.last_mut().unwrap().push_str(&unquote(line)),
                    _ => {}
                }
            }
        }
        flush(&mut id, &mut strs);
        out
    }

    /// Every message the sources hand to `t`, `tf!`, `tn` or `n`.
    fn messages() -> BTreeSet<String> {
        let call = regex::Regex::new(r#"\b(?:t|tf!|n)\(\s*"((?:[^"\\]|\\.)*)""#).unwrap();
        let plural = regex::Regex::new(r#"\btn\(\s*"((?:[^"\\]|\\.)*)",\s*"((?:[^"\\]|\\.)*)""#).unwrap();
        let mut out = BTreeSet::new();
        let mut stack = vec![std::path::PathBuf::from("src")];
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(dir).unwrap().flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                } else if path.extension().is_some_and(|e| e == "rs") && !path.ends_with("i18n.rs") {
                    let text = std::fs::read_to_string(&path).unwrap();
                    out.extend(call.captures_iter(&text).map(|c| c[1].to_string()));
                    out.extend(plural.captures_iter(&text).map(|c| c[1].to_string()));
                }
            }
        }
        out
    }

    #[test]
    fn every_message_is_translated_to_portuguese() {
        let po = catalog(include_str!("../assets/i18n/pt-BR.po"));
        let missing: Vec<_> = messages().into_iter().filter(|m| po.get(m) != Some(&true)).collect();
        assert!(missing.is_empty(), "{} messages lack a Portuguese translation:\n{}", missing.len(), missing.join("\n"));
    }

    #[test]
    fn the_catalog_has_no_stale_messages() {
        let used = messages();
        let stale: Vec<_> = catalog(include_str!("../assets/i18n/pt-BR.po")).into_keys().filter(|m| !used.contains(m)).collect();
        assert!(stale.is_empty(), "{} messages in pt-BR.po are no longer used:\n{}", stale.len(), stale.join("\n"));
    }

    #[test]
    fn portuguese_is_compiled_in() {
        assert_eq!(fastframe_i18n::gettext(Locale::Portuguese, "Save"), "Salvar");
        assert_eq!(fastframe_i18n::gettext(Locale::English, "Save"), "Save");
        assert_eq!(fastframe_i18n::ngettext(Locale::Portuguese, "{n} error", "{n} errors", 2), "{n} erros");
    }

    #[test]
    fn placeholders_are_filled() {
        let name = "a.tex";
        assert_eq!(crate::tf!("Delete {name}?", name), "Delete a.tex?");
        assert_eq!(tn("{n} error", "{n} errors", 3), "3 errors");
        assert_eq!(number(1234567), "1,234,567");
        assert_eq!(number(999), "999");
    }
}
