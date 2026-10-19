//! Second-clause forms per action family, and the lie clauses. See
//! `grammar.rs` for the slot vocabulary.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LieKind {
    InflatedCount,
    PastGift,
    HiddenCache,
    FakePromise,
    ClaimedVisit,
    OthersIntent,
}

pub(crate) static SECOND: &[(&str, [&str; 3])] = &[
    (
        "take",
        [
            "the pot is not bare tonight",
            "with {count} under my cloak",
            "lest the children wake hungry",
        ],
    ),
    (
        "give",
        [
            "for your hearth is cold tonight",
            "from my store of {count}",
            "that you may eat till dawn",
        ],
    ),
    (
        "forage",
        [
            "while the light holds over the field",
            "with {herbs} bound in my cloth",
            "lest the frost take them first",
        ],
    ),
    (
        "help",
        [
            "with hands glad for the work",
            "beside {person} till the task is done",
            "that the load feels half as heavy",
        ],
    ),
    (
        "steal",
        [
            "while no eyes mark my hands",
            "for my store holds {store} and naught else",
            "lest hunger drive me harder still",
        ],
    ),
    (
        "accuse",
        [
            "for {person} took from the common store",
            "while the measure runs short each night",
            "that all here may judge the truth",
        ],
    ),
    (
        "promise",
        [
            "kept true as the dawn returns",
            "to pay {person} {count} in kind",
            "bound by my word before all",
        ],
    ),
    (
        "draw_water",
        [
            "for the pot boils dry by dusk",
            "from the well with {water} brimming cold",
            "lest thirst wake us in the night",
        ],
    ),
    (
        "chop",
        [
            "for the frost creeps near the door",
            "with {wood} stacked high by the hut",
            "till the pile stands shoulder high",
        ],
    ),
    (
        "gather_herbs",
        [
            "for fever walks the low huts",
            "with {herbs} fresh from the damp edge",
            "while the leaves still hold their strength",
        ],
    ),
    (
        "craft",
        [
            "shaped sound for the winter keeping",
            "of {wood} and {tools} worn smooth",
            "that it may hold till spring thaw",
        ],
    ),
    (
        "tend",
        [
            "till the field breathes green again",
            "for the {place} soil runs thin and pale",
            "with care through frost and thaw",
        ],
    ),
    (
        "heal",
        [
            "that breath comes soft again",
            "with {herbs} pressed warm to the wound",
            "till fever loosens its cold grip",
        ],
    ),
    (
        "trade",
        [
            "for {coin} weighed fair in hand",
            "with {quality} goods laid open plain",
            "that neither walks home cheated",
        ],
    ),
    (
        "pray",
        [
            "for mercy on the thin store",
            "by the shrine past the {place} in silence",
            "lest the dark take our hope",
        ],
    ),
    (
        "repair",
        [
            "that the roof holds through rain",
            "with {tools} mended for the task",
            "lest the mud wash down at night",
        ],
    ),
    (
        "console",
        [
            "for grief sits heavy on {person}",
            "till tears soften into sleep",
            "with my hand upon your shoulder",
        ],
    ),
    (
        "rest",
        [
            "for my limbs are heavy as stone",
            "by the hearth till the fire burns low",
            "with {place} quiet round my dreams",
        ],
    ),
    (
        "go",
        [
            "to the {place} by the low path",
            "while the mud still holds my step",
            "for need calls me past the gate",
        ],
    ),
    (
        "act:plea",
        [
            "for my children have not eaten",
            "spare {count} from your store",
            "lest we last not till dawn",
        ],
    ),
    (
        "act:gratitude",
        [
            "for your kindness warms the hut",
            "with {count} owed to your grace",
            "kept in my heart past winter",
        ],
    ),
    (
        "act:gossip",
        [
            "for {other} met {person} past dusk",
            "by the well where all tongues wag",
            "though none here speak it plain",
        ],
    ),
    (
        "act:forgiveness",
        [
            "for the debt is washed away",
            "with no claim left on {person}",
            "that we may share bread again",
        ],
    ),
    (
        "act:blessing",
        [
            "for the hearth held through frost",
            "upon {person} and all close kin",
            "with grain enough till spring",
        ],
    ),
    (
        "act:threat",
        [
            "that you will rue this theft",
            "with {person} marked still for reckoning",
            "lest steel answer for your slight",
        ],
    ),
    (
        "act:counsel",
        [
            "for patience mends more than haste",
            "to weigh {count} twice ere you share",
            "lest haste spill the winter store",
        ],
    ),
    (
        "act:none",
        [
            "while the fire burns steady still",
            "with {person} close by the hearth",
            "as the night wears soft and slow",
        ],
    ),
];

pub(crate) static LIE_CLAUSES: &[(LieKind, [&str; 2])] = &[
    (
        LieKind::InflatedCount,
        [
            "I carry {count}, count them yourself",
            "I bear {count} full in my sack",
        ],
    ),
    (
        LieKind::PastGift,
        [
            "I gave {person} {count} last thaw",
            "I fed {person} from my store ere frost",
        ],
    ),
    (
        LieKind::HiddenCache,
        [
            "I keep {count} hidden past the {place}",
            "I hold {count} sacks hidden past the {place}",
        ],
    ),
    (
        LieKind::FakePromise,
        [
            "I promised {person} my oath ere harvest",
            "I swore to {person} {obligation} in truth",
        ],
    ),
    (
        LieKind::ClaimedVisit,
        [
            "I stood at the {place} ere first light",
            "I walked to the {place} at dusk yesterday",
        ],
    ),
    (
        LieKind::OthersIntent,
        [
            "I heard {person} meant to cheat you",
            "I know {person} plans to steal your share",
        ],
    ),
];
