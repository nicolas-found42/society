//! The action metadata table (spec #5, ticket #8) and the act table
//! (ticket #10). One row per action id; the shortlist, the policy, the
//! speaker and the check read this table. A new action type is a row plus a
//! grammar rule, not an edit across files.
//!
//! Action ids stay atomic. `go:<place>` remains the only parameterised id and
//! is resolved through [`GO_META`].

use crate::world::Role;

use crate::action::SocialAct;

/// The kinds of act the shortlist covers. The category decides what the menu
/// represents, never how much a given action is worth.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Category {
    Need,
    Social,
    Work,
    Rest,
    Move,
}

/// The need an action serves, when it serves one. The survival tier reads
/// this, not a spelling convention.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Need {
    Food,
    Water,
    Health,
    Warmth,
    Rest,
    Faith,
    None,
}

/// One row of the action metadata table.
#[derive(Clone, Copy, Debug)]
pub struct ActionMeta {
    /// The atomic action id, or the `go` family for `go:<place>`.
    pub id: &'static str,
    pub category: Category,
    /// Prose rendering, used by the speaker and the feed.
    pub phrase: &'static str,
    /// The social act the action implies. `SocialAct::None` for the thirteen
    /// actions with no spoken counterpart; `SPEAK_MARKER` for `speak`, whose
    /// act the policy chooses from the speech menu.
    pub implied_act: SocialAct,
    pub need: Need,
    /// The role whose duty this action is, for the `role_duty` term.
    pub role_affinity: Option<Role>,
    /// Approximate energy cost, 0..=10, for the `energy_cost` term.
    pub energy_cost: u8,
}

/// `speak` is the one action whose social act is a decision, not a table
/// entry. The menu is the eight acts the policy scores.
pub const SPEECH_MENU: [SocialAct; 8] = [
    SocialAct::Plea,
    SocialAct::Gratitude,
    SocialAct::Gossip,
    SocialAct::Forgiveness,
    SocialAct::Blessing,
    SocialAct::Threat,
    SocialAct::Counsel,
    SocialAct::None,
];

/// The table, in id order. Keep the `id` spellings identical to
/// [`crate::action::Intent::parse`]; the ids are the wire format.
pub const ACTIONS: [ActionMeta; 19] = [
    ActionMeta {
        id: "accuse",
        category: Category::Social,
        phrase: "accuse someone here",
        implied_act: SocialAct::Accusation,
        need: Need::None,
        role_affinity: Some(Role::Watchman),
        energy_cost: 0,
    },
    ActionMeta {
        id: "chop",
        category: Category::Work,
        phrase: "chop wood",
        implied_act: SocialAct::None,
        need: Need::Warmth,
        role_affinity: Some(Role::Woodcutter),
        energy_cost: 2,
    },
    ActionMeta {
        id: "console",
        category: Category::Social,
        phrase: "console someone here",
        implied_act: SocialAct::Counsel,
        need: Need::None,
        role_affinity: Some(Role::Elder),
        energy_cost: 0,
    },
    ActionMeta {
        id: "craft",
        category: Category::Work,
        phrase: "craft a tool",
        implied_act: SocialAct::None,
        need: Need::None,
        role_affinity: Some(Role::Artisan),
        energy_cost: 2,
    },
    ActionMeta {
        id: "draw_water",
        category: Category::Need,
        phrase: "draw water from the well",
        implied_act: SocialAct::None,
        need: Need::Water,
        role_affinity: None,
        energy_cost: 1,
    },
    ActionMeta {
        id: "forage",
        category: Category::Need,
        phrase: "forage here for food",
        implied_act: SocialAct::None,
        need: Need::Food,
        role_affinity: Some(Role::Forager),
        energy_cost: 2,
    },
    ActionMeta {
        id: "gather_herbs",
        category: Category::Work,
        phrase: "gather herbs",
        implied_act: SocialAct::None,
        need: Need::Health,
        role_affinity: Some(Role::Herbalist),
        energy_cost: 2,
    },
    ActionMeta {
        id: "give",
        category: Category::Social,
        phrase: "give food to someone here",
        implied_act: SocialAct::Gift,
        need: Need::None,
        role_affinity: Some(Role::Steward),
        energy_cost: 0,
    },
    ActionMeta {
        id: "heal",
        category: Category::Work,
        phrase: "heal the sick",
        implied_act: SocialAct::Counsel,
        need: Need::Health,
        role_affinity: Some(Role::Herbalist),
        energy_cost: 1,
    },
    ActionMeta {
        id: "help",
        category: Category::Social,
        phrase: "help someone here",
        implied_act: SocialAct::None,
        need: Need::None,
        role_affinity: Some(Role::Steward),
        energy_cost: 1,
    },
    ActionMeta {
        id: "pray",
        category: Category::Work,
        phrase: "pray at the shrine",
        implied_act: SocialAct::None,
        need: Need::Faith,
        role_affinity: Some(Role::Elder),
        energy_cost: 0,
    },
    ActionMeta {
        id: "promise",
        category: Category::Social,
        phrase: "promise someone here",
        implied_act: SocialAct::Promise,
        need: Need::None,
        role_affinity: None,
        energy_cost: 0,
    },
    ActionMeta {
        id: "repair",
        category: Category::Work,
        phrase: "repair what is broken",
        implied_act: SocialAct::None,
        need: Need::Warmth,
        role_affinity: Some(Role::Artisan),
        energy_cost: 2,
    },
    ActionMeta {
        id: "rest",
        category: Category::Rest,
        phrase: "rest",
        implied_act: SocialAct::None,
        need: Need::Rest,
        role_affinity: None,
        energy_cost: 0,
    },
    ActionMeta {
        id: "speak",
        category: Category::Social,
        phrase: "speak aloud",
        implied_act: SocialAct::Promise,
        need: Need::None,
        role_affinity: None,
        energy_cost: 0,
    },
    ActionMeta {
        id: "steal",
        category: Category::Need,
        phrase: "take what belongs to someone else",
        implied_act: SocialAct::None,
        need: Need::Food,
        role_affinity: None,
        energy_cost: 1,
    },
    ActionMeta {
        id: "take",
        category: Category::Need,
        phrase: "take food from the store here",
        implied_act: SocialAct::None,
        need: Need::Food,
        role_affinity: None,
        energy_cost: 0,
    },
    ActionMeta {
        id: "tend",
        category: Category::Work,
        phrase: "tend the field",
        implied_act: SocialAct::None,
        need: Need::None,
        role_affinity: Some(Role::Farmer),
        energy_cost: 2,
    },
    ActionMeta {
        id: "trade",
        category: Category::Work,
        phrase: "trade at the market",
        implied_act: SocialAct::TradeOffer,
        need: Need::None,
        role_affinity: Some(Role::Trader),
        energy_cost: 1,
    },
];

/// The `go:<place>` row. Moves serve no need directly; they lead somewhere.
pub const GO_META: ActionMeta = ActionMeta {
    id: "go",
    category: Category::Move,
    phrase: "leave for elsewhere",
    implied_act: SocialAct::None,
    need: Need::None,
    role_affinity: Some(Role::Watchman),
    energy_cost: 1,
};

/// The metadata for an action id, parameterised ids included.
#[must_use]
pub fn meta_for(action: &str) -> ActionMeta {
    if action.starts_with("go:") {
        return GO_META;
    }
    ACTIONS
        .iter()
        .find(|m| m.id == action)
        .copied()
        .unwrap_or(ActionMeta {
            id: "unknown",
            category: Category::Work,
            phrase: "do something",
            implied_act: SocialAct::None,
            need: Need::None,
            role_affinity: None,
            energy_cost: 1,
        })
}

/// The act an action implies: the table's act, or the policy's choice for
/// `speak`. `chosen` carries that choice; it is ignored elsewhere.
#[must_use]
pub(crate) fn act_for(action: &str, chosen: SocialAct) -> SocialAct {
    if action == "speak" {
        return chosen;
    }
    if action.starts_with("go:") {
        return SocialAct::None;
    }
    match action {
        "give" => SocialAct::Gift,
        "accuse" => SocialAct::Accusation,
        "promise" => SocialAct::Promise,
        "trade" => SocialAct::TradeOffer,
        "console" | "heal" => SocialAct::Counsel,
        _ => SocialAct::None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_wire_action_has_a_row() {
        for id in [
            "speak",
            "rest",
            "take",
            "give",
            "forage",
            "help",
            "steal",
            "accuse",
            "promise",
            "draw_water",
            "chop",
            "gather_herbs",
            "craft",
            "tend",
            "heal",
            "trade",
            "pray",
            "repair",
            "console",
        ] {
            assert_eq!(meta_for(id).id, id, "missing table row for {id}");
        }
        assert_eq!(meta_for("go:market").id, "go");
    }

    #[test]
    fn the_act_table_matches_the_spec() {
        assert_eq!(act_for("give", SocialAct::None), SocialAct::Gift);
        assert_eq!(act_for("accuse", SocialAct::None), SocialAct::Accusation);
        assert_eq!(act_for("promise", SocialAct::None), SocialAct::Promise);
        assert_eq!(act_for("trade", SocialAct::None), SocialAct::TradeOffer);
        assert_eq!(act_for("console", SocialAct::None), SocialAct::Counsel);
        assert_eq!(act_for("heal", SocialAct::None), SocialAct::Counsel);
        assert_eq!(act_for("chop", SocialAct::None), SocialAct::None);
        assert_eq!(act_for("help", SocialAct::None), SocialAct::None);
        assert_eq!(act_for("steal", SocialAct::None), SocialAct::None);
        assert_eq!(act_for("pray", SocialAct::None), SocialAct::None);
        assert_eq!(act_for("go:well", SocialAct::None), SocialAct::None);
        // `speak` performs the act the policy chose.
        assert_eq!(act_for("speak", SocialAct::Threat), SocialAct::Threat);
    }

    #[test]
    fn the_speech_menu_has_eight_acts() {
        assert_eq!(SPEECH_MENU.len(), 8);
    }

    #[test]
    fn categories_cover_the_shortlist_partition() {
        for (id, want) in [
            ("take", Category::Need),
            ("forage", Category::Need),
            ("draw_water", Category::Need),
            ("steal", Category::Need),
            ("speak", Category::Social),
            ("help", Category::Social),
            ("console", Category::Social),
            ("accuse", Category::Social),
            ("promise", Category::Social),
            ("give", Category::Social),
            ("rest", Category::Rest),
            ("go:well", Category::Move),
            ("chop", Category::Work),
            ("tend", Category::Work),
        ] {
            assert_eq!(meta_for(id).category, want, "{id}");
        }
    }
}
