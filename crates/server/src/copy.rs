//! The copy filter: did the voice model reproduce its own prompt?
//!
//! Measured on the real stack: the 350M voice model copies the nearest
//! salient string in the card (the role goal, a raw action id, its own
//! previous line, "Recent speech at this place"). The line is then stored in
//! memory and reappears in the next card, so the village talks in circles.
//! The gate cannot catch it — fidelity measures alignment with the decided
//! action, not wording, and it scored the identical string "draw_water" 0.96
//! when the decision was `draw_water` — so the leak is caught at the text,
//! before the gate sees it.

/// Lowercase, punctuation to space, whitespace collapsed.
#[must_use]
pub fn normalize(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut pending_space = false;
    for c in text.chars() {
        if c.is_alphanumeric() {
            if pending_space && !out.is_empty() {
                out.push(' ');
            }
            pending_space = false;
            out.extend(c.to_lowercase());
        } else {
            // Punctuation, underscores and whitespace are all word breaks, so
            // `draw_water` and "draw water" compare equal.
            pending_space = true;
        }
    }
    out
}

/// True when `line` reproduces text from one of `sources`.
///
/// Equality counts, and so does a run of two or more whole words in either
/// direction: "draw water" is copied from a card that says "You have decided
/// to draw water from the well." A one-word line is only a copy of a one-word
/// source, so a villager may say "Mira" (a name) without being silenced, while
/// the one-word action id "help" is still caught by its own source.
#[must_use]
pub fn is_copy(line: &str, sources: &[String]) -> bool {
    let n = normalize(line);
    if n.is_empty() {
        return false;
    }
    let words: Vec<&str> = n.split(' ').collect();
    for source in sources {
        let m = normalize(source);
        if m.is_empty() {
            continue;
        }
        if n == m {
            return true;
        }
        let source_words: Vec<&str> = m.split(' ').collect();
        if words.len() >= 2 && contains_run(&source_words, &words) {
            return true;
        }
        if source_words.len() >= 2 && contains_run(&words, &source_words) {
            return true;
        }
    }
    false
}

/// True when `needle` appears as a contiguous run of whole words in `hay`.
fn contains_run(hay: &[&str], needle: &[&str]) -> bool {
    needle.len() <= hay.len() && hay.windows(needle.len()).any(|window| window == needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_lowercases_breaks_punctuation_and_collapses_space() {
        assert_eq!(normalize("  Hello,   World!  "), "hello world");
        assert_eq!(normalize("Draw_water: now!"), "draw water now");
        assert_eq!(normalize("A — B"), "a b");
        assert_eq!(normalize("!!!"), "");
        assert_eq!(normalize("draw_water"), normalize("draw water"));
    }

    #[test]
    fn a_two_word_fragment_of_the_card_is_a_copy() {
        let card = "You have decided to draw water from the well.".to_owned();
        assert!(is_copy("draw water", &[card]));
    }

    #[test]
    fn a_one_word_action_id_is_a_copy_of_its_own_source_only() {
        let source = "help".to_owned();
        assert!(is_copy("help", &[source]));
        let card = "You have decided to help someone here.".to_owned();
        assert!(!is_copy("help", &[card]));
    }

    #[test]
    fn a_name_is_not_a_copy_of_a_card_that_names_it() {
        let card = "You are Mira, the village farmer. People here: Mira.".to_owned();
        assert!(!is_copy("Mira", &[card]));
    }

    #[test]
    fn a_longer_line_that_contains_the_card_is_not_a_copy() {
        let card = "You have decided to draw water from the well.".to_owned();
        let line = "I will draw water from the well tomorrow";
        assert!(!is_copy(line, &[card]));
    }

    #[test]
    fn an_empty_line_is_never_a_copy() {
        assert!(!is_copy("", &["help".to_owned()]));
        assert!(!is_copy("   ", &["help".to_owned()]));
    }
}
