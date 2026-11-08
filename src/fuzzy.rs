//! The command palette's search: every word typed must be found in the
//! entry, whole or as scattered letters ("nref" finds "Nova referência"),
//! accents and case ignored. Entries are ranked by how well the words
//! match: at the start of the name, at the start of one of its words,
//! inside a word, as initials, or spread out. Also says which characters
//! of the name matched, to show them.

use crate::textmatch::folded;

/// One char of the text searched, folded, with the index of the char of the
/// original text it came from and whether a word starts there.
struct Hay {
    chars: Vec<char>,
    origin: Vec<usize>,
    starts: Vec<bool>,
}

impl Hay {
    fn new(text: &str) -> Self {
        let (mut chars, mut origin, mut starts) = (Vec::new(), Vec::new(), Vec::new());
        let mut previous: Option<char> = None;
        for (i, c) in text.chars().enumerate() {
            let start = match previous {
                None => true,
                Some(p) => !p.is_alphanumeric() && c.is_alphanumeric() || p.is_lowercase() && c.is_uppercase(),
            };
            for (k, f) in folded(&c.to_string()).chars().enumerate() {
                chars.push(f);
                origin.push(i);
                starts.push(start && k == 0);
            }
            previous = Some(c);
        }
        Hay { chars, origin, starts }
    }
}

/// How well `word` (folded) matches in `hay`, and the positions it took.
fn word_score(hay: &Hay, word: &[char]) -> Option<(i32, Vec<usize>)> {
    let n = word.len();
    if n == 0 || n > hay.chars.len() {
        return None;
    }
    // Whole: the best of its occurrences.
    let mut best: Option<(i32, usize)> = None;
    for at in 0..=hay.chars.len() - n {
        if hay.chars[at..at + n] == *word {
            let score = if at == 0 {
                120
            } else if hay.starts[at] {
                100
            } else {
                60
            } - (at as i32).min(30) / 3;
            if best.is_none_or(|(b, _)| score > b) {
                best = Some((score, at));
            }
        }
    }
    if let Some((score, at)) = best {
        return Some((score + n as i32 * 4, (at..at + n).collect()));
    }
    // As initials: each letter starts a word ("nr", "nova ref").
    let mut taken = Vec::new();
    let mut from = 0;
    for &c in word {
        let at = (from..hay.chars.len()).find(|&i| hay.starts[i] && hay.chars[i] == c)?;
        taken.push(at);
        from = at + 1;
    }
    // Not every letter needs a word of its own: "nref" is "n" + "ref".
    if taken.len() == n {
        return Some((50 + n as i32 * 3, taken));
    }
    None
}

/// Pieces of words, in order: each letter right after the one before, or
/// at the start of a word ("nref": Nova REFerência, "abpro": ABrir
/// PROjeto). A letter anywhere else would be noise ("auto" in "AUmenTar
/// fOnte").
fn spread_score(hay: &Hay, word: &[char]) -> Option<(i32, Vec<usize>)> {
    let mut taken: Vec<usize> = Vec::new();
    let mut score = 0;
    for &c in word {
        let at = match taken.last() {
            Some(&prev) if hay.chars.get(prev + 1) == Some(&c) => {
                score += 4;
                prev + 1
            }
            last => {
                let from = last.map(|p| p + 1).unwrap_or(0);
                score += 6;
                (from..hay.chars.len()).find(|&i| hay.starts[i] && hay.chars[i] == c)?
            }
        };
        taken.push(at);
    }
    // Spread over a long stretch counts less.
    let span = taken.last().unwrap() - taken[0];
    Some((score - (span as i32) / 4, taken))
}

/// The score of `query` for an entry named `name`, with `extra` (where it
/// sits, its shortcut) searched too but less; `None` when a word is
/// missing. The second value is the chars of `name` that matched.
pub fn score(query: &str, name: &str, extra: &str) -> Option<(i32, Vec<usize>)> {
    let words: Vec<Vec<char>> = folded(query).split_whitespace().map(|w| w.chars().collect()).collect();
    if words.is_empty() {
        return Some((0, vec![]));
    }
    let main = Hay::new(name);
    let other = Hay::new(extra);
    let mut total = 0;
    let mut hits = Vec::new();
    for w in &words {
        if let Some((s, at)) = word_score(&main, w).or_else(|| spread_score(&main, w).filter(|(s, _)| *s > 0)) {
            total += s;
            hits.extend(at.into_iter().map(|i| main.origin[i]));
        } else if let Some((s, _)) = word_score(&other, w) {
            total += s / 3;
        } else {
            return None;
        }
    }
    hits.sort_unstable();
    hits.dedup();
    // Of two equal matches, the shorter name.
    Some((total * 4 - name.chars().count() as i32 / 4, hits))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rank<'a>(query: &str, names: &[&'a str]) -> Vec<&'a str> {
        let mut found: Vec<(i32, &str)> = names.iter().filter_map(|n| score(query, n, "").map(|(s, _)| (s, *n))).collect();
        found.sort_by(|a, b| b.0.cmp(&a.0));
        found.into_iter().map(|(_, n)| n).collect()
    }

    #[test]
    fn accents_and_case_are_ignored() {
        assert!(score("referencia", "Nova referência…", "").is_some());
        assert!(score("SEÇÃO", "Ir para a seção", "").is_some());
    }

    #[test]
    fn every_word_must_be_found() {
        assert!(score("nova nota", "Nova nota…", "").is_some());
        assert!(score("nova tabela", "Nova nota…", "").is_none());
    }

    #[test]
    fn starts_of_words_win() {
        assert_eq!(rank("tab", &["Fechar todas as abas", "Inserir tabela…", "Próxima aba"])[0], "Inserir tabela…");
        assert_eq!(rank("ab", &["Tabela", "Abrir projeto…"])[0], "Abrir projeto…");
    }

    #[test]
    fn initials_and_scattered_letters() {
        assert_eq!(rank("nr", &["Inserir lista…", "Nova referência…"]), ["Nova referência…"]);
        assert_eq!(rank("nref", &["Nova referência…", "Nova nota…"]), ["Nova referência…"]);
        assert_eq!(rank("abpro", &["Abrir projeto…", "Abrir pasta"]), ["Abrir projeto…"]);
        assert!(score("auto", "Aumentar fonte do editor", "").is_none());
    }

    #[test]
    fn matched_chars_point_into_the_name() {
        let (_, hits) = score("ref", "Nova referência…", "").unwrap();
        assert_eq!(hits, [5, 6, 7]);
        // Accented chars keep their place.
        let (_, hits) = score("ca", "Citação", "").unwrap();
        assert_eq!(hits, [4, 5]);
    }

    #[test]
    fn the_extra_text_counts_less() {
        let ranked = rank("arquivo", &["Arquivo recente"]);
        assert_eq!(ranked.len(), 1);
        let (in_name, _) = score("ctrl", "Ctrl de versões", "").unwrap();
        let (in_extra, hits) = score("ctrl", "Salvar", "Ctrl+S").unwrap();
        assert!(in_name > in_extra);
        assert!(hits.is_empty());
    }
}
