//! The grammar's rule tables (spec #5, ticket #12).
//!
//! Rules are keyed (action, register); `speak` is keyed (act, register)
//! because its act is the thing the line has to perform. A cell key is
//! `"action_id"` or `"act:<act_id>"`; the register keys are
//! [`REGISTERS`]. Templates carry typed slot placeholders; the speaker fills
//! them from what the villager can observe or remember, and every filled
//! slot contributes its claim to the check.
//!
//! Coverage floor (spec constants): two first-clause rules per cell over 95
//! non-speech cells (19 actions x 5 registers) and 40 speech cells (8 acts x
//! 5 registers) is **270 first-clause rules**; three second-clause forms per
//! action family over 27 families (19 actions + 8 speech acts) is **81
//! second-clause forms**. [`assert_floors`] counts the tables against the
//! registry.

use crate::action::SocialAct;
use crate::check::Claim;
pub(crate) use crate::grammar_first::FIRST;
pub(crate) use crate::grammar_second::LieKind;
pub(crate) use crate::grammar_second::{LIE_CLAUSES, SECOND};
use crate::world::{Mood, Place};

/// The five registers, index order fixed. Keyed by the short words here; the
/// feed renders the full labels ("cold and clipped", ...).
pub(crate) const REGISTERS: [&str; 5] = ["cold", "wary", "plain", "warm", "urgent"];

/// The register keys, exported under a distinct name from the feed labels.
pub const REGISTER_KEYS: [&str; 5] = REGISTERS;

/// The register key for a mood. Same mapping the old engine derived from the
/// judge's dropped tone question.
#[must_use]
pub(crate) const fn register_for_mood(mood: Mood) -> &'static str {
    match mood {
        Mood::Desperate => "urgent",
        Mood::Anxious | Mood::Feverish | Mood::Grieving => "wary",
        Mood::Bitter | Mood::Weary => "cold",
        Mood::Hopeful => "warm",
        Mood::Determined | Mood::Content => "plain",
    }
}

/// The full register label for the feed, from the short key.
#[must_use]
pub(crate) fn register_label(key: &str) -> &'static str {
    match key {
        "cold" => "cold and clipped",
        "wary" => "wary and guarded",
        "warm" => "warm and open",
        "urgent" => "urgent and raw",
        _ => "plain and neutral",
    }
}

/// The cell key for a speaking act of `speak`.
#[must_use]
pub(crate) fn act_key(act: SocialAct) -> String {
    format!("act:{}", act.id())
}

/// The slot vocabulary the templates may use: `{count}` `{store}` `{granary}`
/// `{water}` `{wood}` `{herbs}` `{tools}` `{coin}` `{person}` `{other}`
/// `{place}` `{obligation}` `{quality}`. One sentence; the second clause, if
/// any, is joined by the renderer within the 20-word budget.
/// Second-clause forms per action family: `(family, [form; 3])`. The
/// connective is matched to the family, not the register.
/// An action family key is an action id or `act:<act_id>` for the speech acts.
/// The lie kinds (ticket #14), each with its clause templates and the claim
/// the clause carries.
/// The claim each lie kind carries once its clause is filled.
#[must_use]
pub(crate) fn lie_claim(kind: LieKind, person: &str, count: u32) -> Claim {
    match kind {
        LieKind::InflatedCount => Claim::Count {
            resource: "food",
            value: count,
        },
        LieKind::PastGift | LieKind::ClaimedVisit => Claim::PastEvent {
            what: "claimed past".into(),
        },
        LieKind::HiddenCache => Claim::OtherCount,
        LieKind::FakePromise => Claim::Obligation {
            to: person.to_owned(),
            made: true,
        },
        LieKind::OthersIntent => Claim::Intent {
            who: person.to_owned(),
            what: "meant to cheat you".into(),
        },
    }
}

/// Count the tables against the coverage floor. Called by the registry test;
/// returns the counts so the report can name them.
#[must_use]
pub const fn count_rules() -> (usize, usize, usize) {
    let mut first = 0;
    let mut i = 0;
    while i < FIRST.len() {
        first += FIRST[i].1.len();
        i += 1;
    }
    let mut second = 0;
    let mut j = 0;
    while j < SECOND.len() {
        second += SECOND[j].1.len();
        j += 1;
    }
    let mut lies = 0;
    let mut k = 0;
    while k < LIE_CLAUSES.len() {
        lies += LIE_CLAUSES[k].1.len();
        k += 1;
    }
    (first, second, lies)
}

/// The floors from the spec constants.
pub const FIRST_FLOOR: usize = 270;
pub const SECOND_FLOOR: usize = 81;

/// Every cell that can occur needs its rules: 19 action keys and 8 act keys,
/// each across the five registers.
#[must_use]
pub fn missing_cells() -> Vec<(String, &'static str)> {
    let mut missing = Vec::new();
    // `speak`'s own cells are the act keys, not the action key.
    let mut keys: Vec<String> = crate::catalog::ACTIONS
        .iter()
        .map(|m| m.id.to_owned())
        .chain(["go".to_owned()])
        .collect();
    keys.retain(|k| k != "speak");
    for act in [
        SocialAct::Plea,
        SocialAct::Gratitude,
        SocialAct::Gossip,
        SocialAct::Forgiveness,
        SocialAct::Blessing,
        SocialAct::Threat,
        SocialAct::Counsel,
        SocialAct::None,
    ] {
        keys.push(act_key(act));
    }
    for key in &keys {
        for register in REGISTERS {
            let has = FIRST
                .iter()
                .any(|((k, r), rules)| *k == key.as_str() && *r == register && !rules.is_empty());
            if !has {
                missing.push((key.clone(), register));
            }
        }
    }
    missing
}

/// The second-clause family for a cell key; the connective is matched to the
/// family (ticket #12).
#[must_use]
pub(crate) fn second_for(family: &str) -> Option<(&'static str, [&'static str; 3])> {
    // The connective is matched to the action family.
    let connective: &'static str = if family.starts_with("act:") {
        match family {
            "act:threat" | "act:gossip" | "act:plea" => "or",
            "act:counsel" => "for",
            _ => "and",
        }
    } else if matches!(family, "accuse" | "steal") {
        "or"
    } else {
        "and"
    };
    SECOND
        .iter()
        .find(|(f, _)| *f == family)
        .map(|(_, forms)| (connective, *forms))
}

/// The place names a `{place}` slot may render.
#[must_use]
pub(crate) const fn place_word(p: Place) -> &'static str {
    p.id()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::SPEECH_MENU;

    /// The rule count meets the floor, asserted against the registry rather
    /// than by inspection (ticket #12).
    #[test]
    fn the_registry_meets_the_coverage_floor() {
        let (first, second, lies) = count_rules();
        assert!(
            first >= FIRST_FLOOR,
            "first-clause rules: {first} < {FIRST_FLOOR}"
        );
        assert!(
            second >= SECOND_FLOOR,
            "second-clause forms: {second} < {SECOND_FLOOR}"
        );
        assert!(lies >= 12, "lie clauses: {lies} < 12");
    }

    /// Every (key, register) cell that can occur carries rules, and every
    /// family carries three second-clause forms.
    #[test]
    fn every_cell_is_populated() {
        let missing = missing_cells();
        assert!(missing.is_empty(), "empty cells: {missing:?}");
        let mut keys: Vec<String> = crate::catalog::ACTIONS
            .iter()
            .map(|m| m.id.to_owned())
            .chain(["go".to_owned()])
            .collect();
        keys.retain(|k| k != "speak");
        for act in SPEECH_MENU {
            keys.push(act_key(act));
        }
        for key in &keys {
            assert!(
                second_for(key).is_some(),
                "family {key} has no second-clause forms"
            );
        }
    }
}
