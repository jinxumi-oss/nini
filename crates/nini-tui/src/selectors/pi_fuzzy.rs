//! Pi's fuzzy filter, ported.
//!
//! Pi's `/model` selector ranks its matches with `fuzzyFilter`
//! (`fuzzy.js#fuzzyFilter`, shipped in `@earendil-works/pi-tui`), and the
//! ranking is the whole point of the model-search change: the search
//! *text* decides which model wins, not the catalogue order.
//!
//! The port is deliberately literal — same scoring constants, same
//! tokenisation, same tie-breaking — because a "close enough" fuzzy
//! filter is how the ordering difference came about in the first place.
//!
//! # The scoring (`fuzzy.js#fuzzyMatch`)
//!
//! Lower is better. For one query token against one text:
//!
//! * consecutive matched characters: `-5 * n` (the n-th consecutive
//!   character is worth 5, 10, 15, ... points),
//! * a match at a word boundary (`start`, whitespace, `-`, `_`, `.`, `/`,
//!   `:`): `-10`,
//! * a gap between two matched characters: `+2` per skipped character,
//! * position in the text: `+0.1 * index`,
//! * the whole text matching the token exactly: `-100`.
//!
//! A query of several tokens (split on whitespace **and** `/`) must match
//! every token; the token scores are summed, and the items are sorted by
//! the total, ascending. Rust's `sort_by` is stable, like JavaScript's
//! `Array.prototype.sort`, so equal scores keep catalogue order.

/// Result of matching one token against one text.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FuzzyMatch {
    pub matches: bool,
    /// Lower is better. Meaningless when `matches` is false.
    pub score: f64,
}

impl FuzzyMatch {
    fn no_match() -> Self {
        FuzzyMatch {
            matches: false,
            score: 0.0,
        }
    }
}

/// Is `c` one of the characters Pi treats as a word boundary?
///
/// Pi: `/[\s\-_./:]/` — the previous character being one of these makes
/// the current position a word boundary (and therefore a rewarded one).
fn is_word_boundary_char(c: char) -> bool {
    c.is_whitespace() || matches!(c, '-' | '_' | '.' | '/' | ':')
}

/// Match one (already lowercased) token against a lowercased text.
///
/// Port of the `matchQuery` closure in `fuzzy.js#fuzzyMatch`.
fn match_query(normalized_query: &str, text_lower: &[char]) -> FuzzyMatch {
    if normalized_query.is_empty() {
        return FuzzyMatch {
            matches: true,
            score: 0.0,
        };
    }
    let q: Vec<char> = normalized_query.chars().collect();
    if q.len() > text_lower.len() {
        return FuzzyMatch::no_match();
    }

    let mut query_index = 0usize;
    let mut score = 0.0f64;
    // Pi starts `lastMatchIndex` at **-1**, so the very first matched
    // character counts as *consecutive* (`lastMatchIndex === i - 1` is
    // true for i = 0) and earns the first -5. An earlier version of this
    // port used `Option<usize>` and `None` for "no match yet", which
    // silently dropped that -5 — and the score is what decides the
    // order, so the port was not Pi's filter, it was a near neighbour
    // of it.
    let mut last_match_index: i64 = -1;
    let mut consecutive_matches = 0i64;

    for i in 0..text_lower.len() {
        if query_index >= q.len() {
            break;
        }
        if text_lower[i] != q[query_index] {
            continue;
        }
        let is_word_boundary = i == 0 || is_word_boundary_char(text_lower[i - 1]);
        if last_match_index == i as i64 - 1 {
            consecutive_matches += 1;
            score -= consecutive_matches as f64 * 5.0;
        } else {
            consecutive_matches = 0;
            // Pi penalises the gap only between two matches.
            if last_match_index >= 0 {
                score += (i as i64 - last_match_index - 1) as f64 * 2.0;
            }
        }
        if is_word_boundary {
            score -= 10.0;
        }
        score += i as f64 * 0.1;
        last_match_index = i as i64;
        query_index += 1;
    }

    if query_index < q.len() {
        return FuzzyMatch::no_match();
    }
    if q == text_lower[..] {
        score -= 100.0;
    }
    FuzzyMatch {
        matches: true,
        score,
    }
}

/// The alphanumeric swap Pi tries when the primary match fails:
/// `gpt5` also matches `5gpt` (`fuzzy.js#fuzzyMatch`).
///
/// Pi's two patterns are `^([a-z]+)([0-9]+)$` and `^([0-9]+)([a-z]+)$`,
/// and **both** are swapped — `5gpt` finds `gpt5` just as `gpt5` finds
/// `5gpt`. An earlier version of this port only handled the first
/// pattern, so the query `5gpt` found nothing where Pi finds a match.
fn swapped_query(query_lower: &str) -> Option<String> {
    let letters: String = query_lower
        .chars()
        .filter(|c| c.is_ascii_alphabetic())
        .collect();
    let digits: String = query_lower.chars().filter(|c| c.is_ascii_digit()).collect();
    if letters.is_empty() || digits.is_empty() {
        return None;
    }
    let letters_then_digits = format!("{letters}{digits}");
    let digits_then_letters = format!("{digits}{letters}");
    if query_lower == letters_then_digits {
        Some(digits_then_letters)
    } else if query_lower == digits_then_letters {
        Some(letters_then_digits)
    } else {
        // Neither pattern: Pi's regexes do not match either, so there is
        // no swap to try.
        None
    }
}

/// Match one token against one text, with Pi's alphanumeric swap.
///
/// Port of `fuzzy.js#fuzzyMatch`.
pub fn fuzzy_match(query: &str, text: &str) -> FuzzyMatch {
    let query_lower = query.to_lowercase();
    let text_lower: Vec<char> = text.to_lowercase().chars().collect();
    let primary = match_query(&query_lower, &text_lower);
    if primary.matches {
        return primary;
    }
    let Some(swapped) = swapped_query(&query_lower) else {
        return primary;
    };
    let swapped_match = match_query(&swapped, &text_lower);
    if !swapped_match.matches {
        return primary;
    }
    FuzzyMatch {
        matches: true,
        score: swapped_match.score + 5.0,
    }
}

/// Split a query into tokens, the way Pi does.
///
/// `fuzzy.js#fuzzyFilter`: `query.trim().split(/[\s/]+/)`, empty tokens
/// dropped.
pub fn query_tokens(query: &str) -> Vec<String> {
    query
        .trim()
        .split(|c: char| c.is_whitespace() || c == '/')
        .filter(|t| !t.is_empty())
        .map(|t| t.to_string())
        .collect()
}

/// Rank `texts` against `query`, best first.
///
/// Port of `fuzzy.js#fuzzyFilter`: returns the indices of the texts that
/// match every token, ordered by total score (ascending). An empty query
/// matches everything, in the original order.
pub fn fuzzy_filter_indices(query: &str, texts: &[String]) -> Vec<usize> {
    let tokens = query_tokens(query);
    if tokens.is_empty() {
        return (0..texts.len()).collect();
    }
    let mut scored: Vec<(usize, f64)> = Vec::new();
    for (i, text) in texts.iter().enumerate() {
        let mut total = 0.0f64;
        let mut all_match = true;
        for token in &tokens {
            let m = fuzzy_match(token, text);
            if m.matches {
                total += m.score;
            } else {
                all_match = false;
                break;
            }
        }
        if all_match {
            scored.push((i, total));
        }
    }
    // Stable sort by ascending score — same as JavaScript's stable sort.
    scored.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
    scored.into_iter().map(|(i, _)| i).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texts(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn a_word_boundary_match_outscores_a_mid_word_one() {
        let boundary = fuzzy_match("gpt", "openai/gpt-5");
        let mid_word = fuzzy_match("gpt", "alongptxyz");
        assert!(boundary.matches && mid_word.matches);
        assert!(
            boundary.score < mid_word.score,
            "boundary {boundary:?} should beat mid-word {mid_word:?}"
        );
    }

    #[test]
    fn a_query_is_split_on_slash_and_whitespace() {
        assert_eq!(
            query_tokens("openai/gpt-5"),
            vec!["openai".to_string(), "gpt-5".to_string()]
        );
        assert_eq!(
            query_tokens("  claude  opus "),
            vec!["claude".to_string(), "opus".to_string()]
        );
    }

    #[test]
    fn every_token_has_to_match() {
        let t = texts(&["openai openai/gpt-5 openai gpt-5"]);
        assert_eq!(fuzzy_filter_indices("openai/gpt-5", &t), vec![0]);
        // A token that cannot match at all drops the item. Note that a
        // token like "nope" **does** match: Pi's filter is a subsequence
        // match, not a substring one, so "nope" is found inside
        // "openai/gpt-5". An earlier version of this test asserted the
        // opposite, and Pi's own output says the opposite.
        assert!(fuzzy_filter_indices("zzzz", &t).is_empty());
    }

    #[test]
    fn an_empty_query_matches_everything_in_order() {
        let t = texts(&["a", "b", "c"]);
        assert_eq!(fuzzy_filter_indices("", &t), vec![0, 1, 2]);
    }

    #[test]
    fn the_alphanumeric_swap_is_tried() {
        // "gpt5" should match "5gpt" through Pi's swap …
        let m = fuzzy_match("gpt5", "5gpt");
        assert!(m.matches, "the swapped form should match: {m:?}");
        // … and the other way round: Pi swaps both patterns.
        let m = fuzzy_match("5gpt", "gpt5");
        assert!(m.matches, "the swapped form should match: {m:?}");
    }
}
