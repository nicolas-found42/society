//! Copy detection for spoken lines.
//!
//! The voice model is a 350M model and it copies: given a card full of salient
//! strings it returns the nearest one as its line ("balance" out of "balance
//! the granary stores", "draw_water" out of the decided action id). Measured
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

/// The words of an already-normalized string.
fn words(text: &str) -> Vec<&str> {
    text.split_whitespace().collect()
}

/// Whether `shorter` occurs as a contiguous run of words inside `longer`.
fn contains_run(longer: &[&str], shorter: &[&str]) -> bool {
    !shorter.is_empty()
        && shorter.len() <= longer.len()
        && longer.windows(shorter.len()).any(|run| run == shorter)
}

#[cfg(test)]
mod tests {
    use super::{is_copy, normalize};

    #[test]
    fn normalize_folds_case_punctuation_and_space() {
        assert_eq!(normalize("Nine, can't  keep up!"), "nine can t keep up");
    }

    #[test]
    fn multi_word_line_is_a_copy_of_a_sentence_that_contains_it() {
        let sources = vec!["You have decided to draw water from the well.".to_string()];
        assert!(is_copy("draw water", &sources));
    }

    #[test]
    fn one_word_line_copies_a_one_word_source_but_not_a_word_inside_a_sentence() {
        let action_id = vec!["help".to_string()];
        let sentence = vec!["You have decided to help someone here.".to_string()];
        assert!(is_copy("help", &action_id));
        assert!(!is_copy("help", &sentence));
    }

    #[test]
    fn name_is_not_a_copy_of_a_card_line_that_mentions_it() {
        let sources = vec![
            "People here: Mira (herbalist, carries 0 food, hunger 1/3, mood content)".to_string(),
        ];
        assert!(!is_copy("Mira", &sources));
    }

    #[test]
    fn line_that_extends_a_source_phrase_is_not_a_copy() {
        let sources = vec!["You have decided to draw water from the well.".to_string()];
        assert!(!is_copy(
            "I will draw water from the well tomorrow",
            &sources
        ));
    }

    #[test]
    fn empty_line_is_not_a_copy() {
        let sources = vec!["You have decided to draw water from the well.".to_string()];
        assert!(!is_copy("", &sources));
        assert!(!is_copy("  ,.! ", &sources));
    }

    #[test]
    fn line_equal_to_a_source_is_a_copy() {
        let sources = vec!["You have decided to draw water from the well.".to_string()];
        assert!(is_copy(
            "you have decided to draw water from the well",
            &sources
        ));
    }
}
