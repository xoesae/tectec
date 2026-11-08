//! Spell checking of the prose (pt-BR and English Hunspell dictionaries,
//! embedded in the binary). A word is accepted when either dictionary knows
//! it. Loading takes a moment, so it happens on a background thread; until
//! then nothing is underlined.

use crate::tf;
use spellbook::Dictionary;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, OnceLock};

const PT_AFF: &str = include_str!("../assets/dictionaries/pt/index.aff");
const PT_DIC: &str = include_str!("../assets/dictionaries/pt/index.dic");
const EN_AFF: &str = include_str!("../assets/dictionaries/en/index.aff");
const EN_DIC: &str = include_str!("../assets/dictionaries/en/index.dic");

use crate::backend::document::Language;

/// pt-BR first, English second.
type Dicts = Result<Vec<Dictionary>, String>;

/// `declared`, unless most of `sample` is known only in the other language.
fn written_in(dicts: &[Dictionary], declared: Language, sample: &[String]) -> Option<Language> {
    let other = match declared {
        Language::Portuguese => Language::English,
        Language::English => Language::Portuguese,
    };
    let knows = |language: Language, word: &str| SpellCache::dicts_for(dicts, Some(language)).iter().any(|d| d.check(word));
    let (mut declared_only, mut other_only) = (0, 0);
    for word in sample {
        match (knows(declared, word), knows(other, word)) {
            (true, false) => declared_only += 1,
            (false, true) => other_only += 1,
            _ => {}
        }
    }
    Some(if other_only >= 20 && other_only > 3 * declared_only { other } else { declared })
}

pub struct SpellCache {
    dicts: Arc<OnceLock<Dicts>>,
    loading: bool,
    /// Answers per (word, language checked against).
    cache: HashMap<(String, Option<Language>), bool>,
    /// The language words are checked in: the document's, unless its text
    /// says otherwise. `None` accepts a word either dictionary knows.
    language: Option<Language>,
    /// What the preamble declares, and a sample of the document's words to
    /// check that against once the dictionaries are loaded.
    declared: Option<Language>,
    sample: Vec<String>,
    /// Words the user chose to ignore.
    pub ignored: HashSet<String>,
    /// Bumped whenever the answer for some word may have changed, so cached
    /// layouts get rebuilt.
    pub version: u64,
    reported_error: bool,
    announced_ready: bool,
}

impl SpellCache {
    pub fn new(ignored: HashSet<String>) -> Self {
        Self {
            dicts: Arc::new(OnceLock::new()),
            loading: false,
            cache: HashMap::new(),
            language: None,
            declared: None,
            sample: Vec::new(),
            ignored,
            version: 0,
            reported_error: false,
            announced_ready: false,
        }
    }

    /// Starts loading the dictionaries (once).
    pub fn ensure_loading(&mut self, ctx: &egui::Context) {
        if self.loading {
            return;
        }
        self.loading = true;
        let cell = self.dicts.clone();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let load = || -> Dicts {
                let pt = Dictionary::new(PT_AFF, PT_DIC).map_err(|e| format!("pt dictionary: {e}"))?;
                let en = Dictionary::new(EN_AFF, EN_DIC).map_err(|e| format!("en dictionary: {e}"))?;
                Ok(vec![pt, en])
            };
            let _ = cell.set(load());
            ctx.request_repaint();
        });
    }

    pub fn ready(&self) -> bool {
        matches!(self.dicts.get(), Some(Ok(_)))
    }

    /// Returns the load error once, for the status bar.
    pub fn take_error(&mut self) -> Option<String> {
        if self.reported_error {
            return None;
        }
        if let Some(Err(e)) = self.dicts.get() {
            self.reported_error = true;
            return Some(tf!("Spell check unavailable: {e}", e));
        }
        None
    }

    /// Called once per frame: when the dictionaries just finished loading,
    /// the layouts have to be rebuilt to show the underlines.
    pub fn poll(&mut self) {
        if !self.announced_ready && self.ready() {
            self.announced_ready = true;
            self.version += 1;
            self.settle_language();
        }
    }

    /// Checks against the document's language from now on: `declared` by its
    /// preamble (`None` when it declares none), unless `sample`, words of its
    /// text, is plainly in the other language (a Portuguese thesis loading
    /// `babel[english]` because `brazil` is not installed).
    pub fn set_language(&mut self, declared: Option<Language>, sample: Vec<String>) {
        self.declared = declared;
        self.sample = sample;
        self.settle_language();
    }

    fn settle_language(&mut self) {
        let language = match (self.declared, self.dicts.get()) {
            (Some(declared), Some(Ok(dicts))) => written_in(dicts, declared, &self.sample),
            (declared, _) => declared,
        };
        if self.language != language {
            self.language = language;
            self.version += 1;
        }
    }

    /// The dictionaries a word in `language` is checked against.
    fn dicts_for<'a>(dicts: &'a [Dictionary], language: Option<Language>) -> &'a [Dictionary] {
        match language {
            Some(Language::Portuguese) => &dicts[..1],
            Some(Language::English) => &dicts[1..],
            None => dicts,
        }
    }

    pub fn is_misspelled(&mut self, word: &str) -> bool {
        self.is_misspelled_in(word, None)
    }

    /// `region`: the language of the passage the word is in (an English
    /// abstract in a Portuguese thesis), over the document's.
    pub fn is_misspelled_in(&mut self, word: &str, region: Option<Language>) -> bool {
        let Some(Ok(dicts)) = self.dicts.get() else { return false };
        if word.chars().count() < 2 || self.ignored.contains(word) || is_name(word) {
            return false;
        }
        let language = region.or(self.language);
        let key = (word.to_string(), language);
        if let Some(bad) = self.cache.get(&key) {
            return *bad;
        }
        let bad = !Self::dicts_for(dicts, language).iter().any(|d| d.check(word));
        self.cache.insert(key, bad);
        bad
    }

    /// Corrections for `word`, from the dictionary of `region` (the passage's
    /// language) or else of the document.
    pub fn suggestions_in(&self, word: &str, region: Option<Language>) -> Vec<String> {
        let Some(Ok(dicts)) = self.dicts.get() else { return vec![] };
        let mut out: Vec<String> = Vec::new();
        for d in Self::dicts_for(dicts, region.or(self.language)) {
            let mut found = Vec::new();
            d.suggest(word, &mut found);
            for s in found {
                if !out.contains(&s) {
                    out.push(s);
                }
                if out.len() >= 5 {
                    return out;
                }
            }
        }
        out
    }

    pub fn ignore(&mut self, word: &str) {
        self.ignored.insert(word.to_string());
        self.cache.retain(|(w, _), _| w != word);
        self.version += 1;
    }
}

/// A capital past the first letter: names and acronyms (LaTeX, BibTeX,
/// GitHub, PDF, UFRJ) that no dictionary has, and that are written that
/// way on purpose.
fn is_name(word: &str) -> bool {
    word.chars().skip(1).any(char::is_uppercase)
}

#[cfg(test)]
impl SpellCache {
    /// A cache with both dictionaries loaded, no UI needed.
    pub fn loaded_for_tests() -> Self {
        let spell = Self::new(HashSet::new());
        let pt = Dictionary::new(PT_AFF, PT_DIC).unwrap();
        let en = Dictionary::new(EN_AFF, EN_DIC).unwrap();
        let _ = spell.dicts.set(Ok(vec![pt, en]));
        spell
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn loaded() -> SpellCache {
        SpellCache::loaded_for_tests()
    }

    #[test]
    fn names_with_inner_capitals_pass() {
        let mut spell = loaded();
        spell.set_language(Some(Language::Portuguese), Vec::new());
        assert!(!spell.is_misspelled("LaTeX") && !spell.is_misspelled("BibTeX") && !spell.is_misspelled("UFRJ"));
        assert!(spell.is_misspelled("relatorrio"));
    }

    #[test]
    fn checks_in_the_document_language() {
        let mut spell = loaded();
        // No language declared: either dictionary will do.
        assert!(!spell.is_misspelled("the") && !spell.is_misspelled("difusão"));
        spell.set_language(Some(Language::Portuguese), Vec::new());
        assert!(spell.is_misspelled("the"), "an English word in a Portuguese text");
        assert!(!spell.is_misspelled("difusão"));
        // …unless the passage is marked as English.
        assert!(!spell.is_misspelled_in("the", Some(Language::English)));
        spell.set_language(Some(Language::English), Vec::new());
        assert!(spell.is_misspelled("difusão") && !spell.is_misspelled("the"));
    }

    #[test]
    fn the_text_outweighs_a_babel_workaround() {
        let mut spell = loaded();
        let words = |text: &str| text.split_whitespace().map(String::from).collect::<Vec<_>>();
        let portuguese = words("esta seção apresenta os conceitos fundamentais necessários para a compreensão deste trabalho inicialmente são discutidos os dispositivos detalhando os blocos lógicos que os compõem e depois a arquitetura proposta com suas vantagens");
        spell.set_language(Some(Language::English), portuguese.clone());
        assert!(!spell.is_misspelled("seção"), "the text is in Portuguese");
        assert!(spell.is_misspelled("the"));
        // A few Portuguese words in an English text change nothing.
        let mut english = words("this section presents the fundamental concepts needed to understand this work first the devices are discussed then the logic blocks that compose them and later the proposed architecture with its advantages");
        english.extend(portuguese.into_iter().take(5));
        spell.set_language(Some(Language::English), english);
        assert!(spell.is_misspelled("seção") && !spell.is_misspelled("the"));
    }
}

