//! Matching and ranking of completion candidates, as IntelliJ does it: the
//! typed text matches a name exactly, as its prefix, as the initials of its
//! words (split at `_`, `-`, spaces and camel-case humps), or anywhere inside
//! it, in that order; within an order, shorter names come first. Case matters
//! for the first letter only (names are stored in any case, and people type
//! the rest in whatever case their hands do).
//!
//! Pure: text in, a verdict out.

use std::collections::HashSet;

/// How well a name matches, best first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Tier {
    Exact,
    Prefix,
    Initials,
    Anywhere,
}

/// A name that matched: how well, and which of its characters (by index) the
/// typed text covers, for the popup to emphasise.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Match {
    pub tier: Tier,
    pub positions: Vec<usize>,
}

/// Matches `pattern` against `name`. With `strict_first` the first letter must
/// have the same case in both (names); without it case never matters
/// (keywords, which people type in either case). The empty pattern matches
/// everything, as a prefix.
pub fn match_name(pattern: &str, name: &str, strict_first: bool) -> Option<Match> {
    let p: Vec<char> = pattern.chars().collect();
    let n: Vec<char> = name.chars().collect();
    if p.is_empty() {
        return Some(Match { tier: Tier::Prefix, positions: Vec::new() });
    }
    if p.len() > n.len() {
        return None;
    }
    // Whether pattern character `i` matches name character `j`: any case,
    // except that the first letter of the pattern is exact when asked.
    let same = |i: usize, j: usize| {
        if i == 0 && strict_first {
            p[0] == n[j]
        } else {
            p[i] == n[j] || p[i].to_lowercase().eq(n[j].to_lowercase())
        }
    };
    let prefix = (0..p.len()).all(|i| same(i, i));
    if prefix {
        let tier = if p.len() == n.len() { Tier::Exact } else { Tier::Prefix };
        return Some(Match { tier, positions: (0..p.len()).collect() });
    }
    if let Some(positions) = initials(&p, &n, &same, &mut 0) {
        return Some(Match { tier: Tier::Initials, positions });
    }
    (1..=n.len() - p.len())
        .find(|&at| (0..p.len()).all(|i| same(i, at + i)))
        .map(|at| Match { tier: Tier::Anywhere, positions: (at..at + p.len()).collect() })
}

/// Where the words of `name` start: its first character, the one after a
/// separator, and the capitals that begin a camel-case hump.
fn word_starts(n: &[char]) -> Vec<usize> {
    let separator = |c: char| matches!(c, '_' | '-' | ' ' | '.' | '$');
    let mut starts = Vec::new();
    for i in 0..n.len() {
        if separator(n[i]) {
            continue;
        }
        let hump = i > 0
            && n[i].is_uppercase()
            && (n[i - 1].is_lowercase() || n[i - 1].is_ascii_digit() || (n[i - 1].is_uppercase() && n.get(i + 1).is_some_and(|c| c.is_lowercase())));
        if i == 0 || separator(n[i - 1]) || hump {
            starts.push(i);
        }
    }
    starts
}

/// The pattern as the leading letters of words: the first segment is the start
/// of the name's first word, each next one starts a later word (words may be
/// skipped), and each takes as many letters of its word as it can use.
fn initials(p: &[char], n: &[char], same: &impl Fn(usize, usize) -> bool, steps: &mut usize) -> Option<Vec<usize>> {
    let starts = word_starts(n);
    // The end of word `w`: where the next begins, or the name's end, less the
    // separators that trail it (they are not typed as part of a word).
    let word_end = |w: usize| starts.get(w + 1).copied().unwrap_or(n.len());
    fn go(pi: usize, from: usize, p: &[char], starts: &[usize], word_end: &dyn Fn(usize) -> usize, same: &dyn Fn(usize, usize) -> bool, dead: &mut HashSet<(usize, usize)>, steps: &mut usize) -> Option<Vec<usize>> {
        *steps += 1;
        if pi == p.len() {
            return Some(Vec::new());
        }
        // Where the search from here (the rest of the pattern, the words from
        // `from` on) failed once, it fails again: not tried twice.
        if dead.contains(&(pi, from)) {
            return None;
        }
        // The first segment must be the name's first word.
        let words = if pi == 0 { 0..1.min(starts.len()) } else { from..starts.len() };
        for w in words {
            let (s, e) = (starts[w], word_end(w));
            let mut k = 0;
            while pi + k < p.len() && s + k < e && same(pi + k, s + k) {
                k += 1;
            }
            // Longest first, so `fiac` takes `fi` of `film` before `f`.
            for take in (1..=k).rev() {
                if let Some(mut rest) = go(pi + take, w + 1, p, starts, word_end, same, dead, steps) {
                    let mut out: Vec<usize> = (s..s + take).collect();
                    out.append(&mut rest);
                    return Some(out);
                }
            }
        }
        dead.insert((pi, from));
        None
    }
    go(0, 0, p, &starts, &word_end, same, &mut HashSet::new(), steps)
}

/// What a list of matches is ordered by: the tier, then the length, then the
/// name without regard to case, then the name.
pub fn rank_key(m: &Match, name: &str) -> (Tier, usize, String, String) {
    (m.tier, name.chars().count(), name.to_lowercase(), name.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tier(pattern: &str, name: &str) -> Option<Tier> {
        match_name(pattern, name, true).map(|m| m.tier)
    }

    fn ranked(pattern: &str, names: &[&str]) -> Vec<String> {
        let mut found: Vec<(Match, &str)> =
            names.iter().filter_map(|n| match_name(pattern, n, true).map(|m| (m, *n))).collect();
        found.sort_by_key(|(m, n)| rank_key(m, n));
        found.into_iter().map(|(_, n)| n.to_string()).collect()
    }

    #[test]
    fn the_four_tiers_are_told_apart() {
        assert_eq!(tier("film", "film"), Some(Tier::Exact));
        assert_eq!(tier("fil", "film"), Some(Tier::Prefix));
        assert_eq!(tier("fa", "film_actor"), Some(Tier::Initials));
        assert_eq!(tier("ctor", "film_actor"), Some(Tier::Anywhere));
        assert_eq!(tier("xyz", "film_actor"), None);
    }

    #[test]
    fn the_tiers_order_exact_then_prefix_then_initials_then_anywhere() {
        assert!(Tier::Exact < Tier::Prefix && Tier::Prefix < Tier::Initials && Tier::Initials < Tier::Anywhere);
        // A name starting `fa` ranks above the one `fa` is the initials of,
        // although it is longer.
        assert_eq!(ranked("fa", &["film_actor", "fabrication_records", "staff_fax"]), ["fabrication_records", "film_actor", "staff_fax"]);
    }

    #[test]
    fn within_a_tier_the_shorter_name_comes_first_then_alphabetical() {
        assert_eq!(ranked("fil", &["film_text", "film", "film_list", "file"]), ["file", "film", "film_list", "film_text"]);
    }

    #[test]
    fn only_the_first_letters_case_must_match() {
        assert_eq!(tier("Fi", "film"), None, "an uppercase first letter does not find a lowercase name");
        assert_eq!(tier("fI", "film"), Some(Tier::Prefix), "the rest need not match in case");
        assert_eq!(tier("FILM", "Film"), Some(Tier::Exact));
        assert_eq!(tier("Film", "film"), None);
        assert_eq!(tier("FILm", "Film"), Some(Tier::Exact));
        assert_eq!(tier("OR", "Order Items"), Some(Tier::Prefix));
    }

    #[test]
    fn without_a_strict_first_letter_case_never_matters() {
        assert_eq!(match_name("SEL", "select", false).map(|m| m.tier), Some(Tier::Prefix));
        assert_eq!(match_name("SELECT", "select", false).map(|m| m.tier), Some(Tier::Exact));
        assert_eq!(match_name("SEL", "select", true), None);
    }

    #[test]
    fn initials_follow_underscores_spaces_and_camel_case_humps() {
        assert_eq!(match_name("fa", "film_actor", true).unwrap().positions, [0, 5]);
        assert_eq!(match_name("gu", "getUser", true).unwrap().positions, [0, 3]);
        assert_eq!(match_name("oi", "Order Items", true), None, "the first letter's case differs");
        assert_eq!(match_name("OI", "Order Items", true).unwrap().tier, Tier::Initials);
        assert_eq!(match_name("xp", "XMLParser", true).map(|m| m.tier), None);
        assert_eq!(match_name("XP", "XMLParser", true).map(|m| m.tier), Some(Tier::Initials));
    }

    #[test]
    fn initials_may_take_several_letters_of_a_word_and_skip_words() {
        assert_eq!(match_name("fiac", "film_actor", true).unwrap().positions, [0, 1, 5, 6]);
        assert_eq!(match_name("fa", "film_category_actor", true).map(|m| m.tier), Some(Tier::Initials));
        // The first initial is the name's own first letter: `ac` is found
        // inside the name, not by initials.
        assert_eq!(tier("ac", "film_actor"), Some(Tier::Anywhere));
    }

    #[test]
    fn matched_positions_cover_the_typed_characters() {
        assert_eq!(match_name("fil", "film", true).unwrap().positions, [0, 1, 2]);
        assert_eq!(match_name("ctor", "film_actor", true).unwrap().positions, [6, 7, 8, 9]);
        assert_eq!(match_name("film", "film", true).unwrap().positions, [0, 1, 2, 3]);
    }

    /// How many steps the initials search takes for `pattern` against `name`.
    fn initials_steps(pattern: &str, name: &str) -> usize {
        let (p, n): (Vec<char>, Vec<char>) = (pattern.chars().collect(), name.chars().collect());
        let same = |i: usize, j: usize| p[i] == n[j];
        let mut steps = 0;
        initials(&p, &n, &same, &mut steps);
        steps
    }

    #[test]
    fn the_initials_search_does_polynomial_work_on_a_pattern_that_almost_matches() {
        // `aaa...ab` against `a_a_a_...`: every `a` can be taken as a letter of
        // many words, and the `b` never comes.
        for words in [8usize, 16, 32, 64] {
            let name = "a_".repeat(words);
            let pattern = format!("{}b", "a".repeat(words));
            let steps = initials_steps(&pattern, &name);
            let bound = (words + 1) * (words + 1) * 16;
            assert!(steps <= bound, "{words} words took {steps} steps (more than {bound})");
        }
        assert_eq!(match_name(&format!("{}b", "a".repeat(40)), &"a_".repeat(40), true), None);
    }

    #[test]
    fn the_empty_pattern_matches_everything_as_a_prefix() {
        let m = match_name("", "film", true).unwrap();
        assert_eq!((m.tier, m.positions.len()), (Tier::Prefix, 0));
    }

    #[test]
    fn a_pattern_longer_than_the_name_does_not_match_and_unicode_is_counted_in_characters() {
        assert_eq!(tier("films", "film"), None);
        assert_eq!(match_name("é", "électricité", true).unwrap().positions, [0]);
        assert_eq!(match_name("lec", "électricité", true).unwrap().positions, [1, 2, 3]);
    }
}
