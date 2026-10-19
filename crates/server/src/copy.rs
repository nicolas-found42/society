//! Copy detection for spoken lines.
//!
//! The voice model is a 350M model and it copies: given a card full of salient
//! strings it returns the nearest one as its line ("balance" out of "balance
//! the granary stores", `draw_water` out of the decided action id). Measured
//! on a 5-turn run: 5 of 45 lines were a verbatim action id or card string,
//! and the line was then stored in memory and fed back into the next card, so
//! the copy repeated. A copy is not a failure of the model's judgment, it is a
//! string the card taught it; so the check for it belongs in code.

/// Lowercase, every non-alphanumeric character becomes a space, whitespace
/// runs collapse to one space, ends trimmed.
#[must_use]
pub fn normalize(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut pending_space = false;
    for ch in text.chars() {
        if ch.is_alphanumeric() {
            if pending_space && !out.is_empty() {
                out.push(' ');
            }
            pending_space = false;
            out.extend(ch.to_lowercase());
        } else {
            pending_space = true;
        }
    }
    out
}

/// True when `line` reproduces text from one of `sources`.
///
/// A line of two or more words is a copy when it occurs as a contiguous run
/// of words inside a source, or a multi-word source occurs inside it. A
/// one-word line is only a copy of a one-word source, so a villager may still
/// say a name ("Mira") without being silenced.
#[must_use]
pub fn is_copy(line: &str, sources: &[String]) -> bool {
    let n = normalize(line);
    if n.is_empty() {
        return false;
    }
    let n_words = words(&n);
    for source in sources {
        let m = normalize(source);
        if m.is_empty() {
            continue;
        }
        let m_words = words(&m);
        if n == m {
            return true;
        }
        if n_words.len() >= 2 && contains_run(&m_words, &n_words) {
            return true;
        }
        if m_words.len() >= 2 && contains_run(&n_words, &m_words) {
            return true;
        }
    }
    false
}

/// True when `line` paraphrases a phrase source: its content words occur, in
/// order, inside the source's content words.
///
/// The measured repeats are paraphrases, not quotes: "keep stocked" out of
/// "keep the settlement stocked", "circulate tools, timber, grain" out of
/// "circulate tools, timber, and grain". `is_copy` misses them because it only
/// compares contiguous runs, so this second, looser test exists. It is only
/// safe on the short phrase sources (action phrase, memory line the card
/// printed): applied to the whole card, common words would match everything.
///
/// A line needs at least two content words. A one-word line ("Mira") must not
/// be a paraphrase of a source that mentions Mira, or a villager could never
/// say a name.
#[must_use]
pub fn is_paraphrase(line: &str, phrases: &[String]) -> bool {
    let normalized_line = normalize(line);
    let line_words = content_words(&normalized_line);
    if line_words.len() < 2 {
        return false;
    }
    for phrase in phrases {
        let normalized_phrase = normalize(phrase);
        let source_words = content_words(&normalized_phrase);
        if source_words.is_empty() {
            continue;
        }
        if is_subsequence(&line_words, &source_words) {
            return true;
        }
    }
    false
}

/// Function words that carry no meaning of their own.
///
/// Paraphrase detection compares content words, so it must ignore the
/// pronouns, auxiliaries, articles and prepositions that would otherwise let
/// any two English sentences match: "counsel them" is not a copy of
/// "counsel the anxious ..." because "them" is present, but the content word
/// "counsel" is. The list is explicit rather than a grammar library because
/// the card's vocabulary is small and fixed.
const STOP_WORDS: &[&str] = &[
    "a", "an", "the", "of", "and", "or", "to", "in", "on", "at", "for", "with", "from", "by", "is",
    "are", "was", "were", "be", "been", "do", "does", "did", "not", "no", "so", "but", "if",
    "then", "than", "as", "it", "its", "this", "that", "these", "those", "i", "me", "my", "we",
    "us", "our", "you", "your", "he", "him", "his", "she", "her", "they", "them", "their", "will",
    "would", "can", "could", "should", "shall", "may", "might", "must", "am",
];

/// The words of an already-normalized string.
fn words(text: &str) -> Vec<&str> {
    text.split_whitespace().collect()
}

/// The content words of an already-normalized string: its words minus
/// [`STOP_WORDS`], in order.
fn content_words(text: &str) -> Vec<&str> {
    words(text)
        .into_iter()
        .filter(|word| !STOP_WORDS.contains(word))
        .collect()
}

/// Whether every word of `needle` occurs in `haystack`, in order, each word of
/// `haystack` used at most once.
fn is_subsequence(needle: &[&str], haystack: &[&str]) -> bool {
    let mut matched = 0;
    for word in haystack {
        if matched == needle.len() {
            break;
        }
        if *word == needle[matched] {
            matched += 1;
        }
    }
    matched == needle.len()
}

/// Whether `shorter` occurs as a contiguous run of words inside `longer`.
fn contains_run(longer: &[&str], shorter: &[&str]) -> bool {
    !shorter.is_empty()
        && shorter.len() <= longer.len()
        && longer.windows(shorter.len()).any(|run| run == shorter)
}

#[cfg(test)]
mod tests {
    use super::{is_copy, is_paraphrase, normalize};

    #[test]
    fn normalize_folds_case_punctuation_and_space() {
        assert_eq!(normalize("Nine, can't  keep up!"), "nine can t keep up");
    }

    #[test]
    fn multi_word_line_is_a_copy_of_a_sentence_that_contains_it() {
        let sources = vec!["You have decided to draw water from the well.".to_owned()];
        assert!(is_copy("draw water", &sources));
    }

    #[test]
    fn one_word_line_copies_a_one_word_source_but_not_a_word_inside_a_sentence() {
        let action_id = vec!["help".to_owned()];
        let sentence = vec!["You have decided to help someone here.".to_owned()];
        assert!(is_copy("help", &action_id));
        assert!(!is_copy("help", &sentence));
    }

    #[test]
    fn name_is_not_a_copy_of_a_card_line_that_mentions_it() {
        let sources = vec![
            "People here: Mira (herbalist, carries 0 food, hunger 1/3, mood content)".to_owned(),
        ];
        assert!(!is_copy("Mira", &sources));
    }

    #[test]
    fn line_that_extends_a_source_phrase_is_not_a_copy() {
        let sources = vec!["You have decided to draw water from the well.".to_owned()];
        assert!(!is_copy(
            "I will draw water from the well tomorrow",
            &sources
        ));
    }

    #[test]
    fn empty_line_is_not_a_copy() {
        let sources = vec!["You have decided to draw water from the well.".to_owned()];
        assert!(!is_copy("", &sources));
        assert!(!is_copy("  ,.! ", &sources));
    }

    #[test]
    fn line_equal_to_a_source_is_a_copy() {
        let sources = vec!["You have decided to draw water from the well.".to_owned()];
        assert!(is_copy(
            "you have decided to draw water from the well",
            &sources
        ));
    }

    #[test]
    fn paraphrase_matches_content_words_in_order_across_a_gap() {
        let sources = vec!["keep the settlement stocked with timber and firewood".to_owned()];
        assert!(is_paraphrase("keep stocked", &sources));
    }

    #[test]
    fn paraphrase_ignores_the_conjunction_between_content_words() {
        let sources = vec!["circulate tools, timber, and grain at the market".to_owned()];
        assert!(is_paraphrase("circulate tools, timber, grain", &sources));
    }

    #[test]
    fn paraphrase_matches_two_content_words_from_a_longer_source() {
        let sources = vec!["gather medicinal herbs and heal the sick".to_owned()];
        assert!(is_paraphrase("herbs for the sick", &sources));
    }

    #[test]
    fn name_with_a_word_absent_from_the_source_is_not_a_paraphrase() {
        let sources = vec!["Mira said: bare to bare".to_owned()];
        assert!(!is_paraphrase("Mira here", &sources));
    }

    #[test]
    fn one_content_word_is_not_a_paraphrase() {
        let sources = vec!["Mira said: bare to bare".to_owned()];
        assert!(!is_paraphrase("Mira", &sources));
    }

    #[test]
    fn extra_content_word_that_is_not_in_the_source_breaks_a_paraphrase() {
        let sources = vec!["draw water from the well".to_owned()];
        assert!(!is_paraphrase(
            "I will draw water from the well tomorrow",
            &sources
        ));
    }

    #[test]
    fn empty_or_stop_word_only_line_is_not_a_paraphrase() {
        let sources = vec!["draw water from the well".to_owned()];
        assert!(!is_paraphrase("", &sources));
        assert!(!is_paraphrase("and the", &sources));
    }
}
