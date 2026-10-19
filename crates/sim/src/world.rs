use serde::{Deserialize, Serialize};

use crate::memory::{Belief, Fact, Memory, MemoryKind, MEMORY_CAP, REFLECTION_BUDGET};
use crate::rng::Rng;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Place {
    Granary,
    Well,
    Field,
    Hut,
    Market,
    Shrine,
    Woods,
}

impl Place {
    pub const ALL: [Self; 7] = [
        Self::Granary,
        Self::Well,
        Self::Field,
        Self::Hut,
        Self::Market,
        Self::Shrine,
        Self::Woods,
    ];

    #[must_use]
    pub const fn id(self) -> &'static str {
        match self {
            Self::Granary => "granary",
            Self::Well => "well",
            Self::Field => "field",
            Self::Hut => "hut",
            Self::Market => "market",
            Self::Shrine => "shrine",
            Self::Woods => "woods",
        }
    }

    #[must_use]
    pub const fn title(self) -> &'static str {
        match self {
            Self::Granary => "The Granary",
            Self::Well => "The Stone Well",
            Self::Field => "The Barley Field",
            Self::Hut => "The Long Hut",
            Self::Market => "The Market Green",
            Self::Shrine => "The Moss Shrine",
            Self::Woods => "The Deep Woods",
        }
    }

    #[must_use]
    pub const fn icon(self) -> &'static str {
        match self {
            Self::Granary => "🏛",
            Self::Well => "⛲",
            Self::Field => "🌾",
            Self::Hut => "🛖",
            Self::Market => "⚖",
            Self::Shrine => "⛩",
            Self::Woods => "🌲",
        }
    }

    #[must_use]
    pub fn from_id(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|p| p.id() == s)
    }

    /// Ring of seven. Symmetric; both directions are listed.
    #[must_use]
    pub const fn adjacent(self) -> &'static [Self] {
        use Place::{Field, Granary, Hut, Market, Shrine, Well, Woods};
        match self {
            Granary => &[Market, Field],
            Field => &[Granary, Woods],
            Market => &[Granary, Well],
            Well => &[Market, Hut],
            Hut => &[Well, Shrine],
            Shrine => &[Hut, Woods],
            Woods => &[Field, Shrine],
        }
    }

    #[must_use]
    pub const fn can_harvest(self) -> bool {
        matches!(self, Self::Field | Self::Woods)
    }

    #[must_use]
    pub const fn can_chop(self) -> bool {
        matches!(self, Self::Woods)
    }

    #[must_use]
    pub const fn can_draw_water(self) -> bool {
        matches!(self, Self::Well)
    }

    #[must_use]
    pub const fn can_gather_herbs(self) -> bool {
        matches!(self, Self::Woods | Self::Shrine | Self::Field)
    }

    #[must_use]
    pub const fn can_craft(self) -> bool {
        matches!(self, Self::Market | Self::Hut)
    }
}

pub const PLACES: [Place; 7] = Place::ALL;
pub const ADJACENCY: [(&str, [&str; 2]); 7] = [
    ("granary", ["market", "field"]),
    ("field", ["granary", "woods"]),
    ("market", ["granary", "well"]),
    ("well", ["market", "hut"]),
    ("hut", ["well", "shrine"]),
    ("shrine", ["hut", "woods"]),
    ("woods", ["field", "shrine"]),
];

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TimeOfDay {
    Dawn,
    Morning,
    Midday,
    Afternoon,
    Dusk,
    Night,
}

impl TimeOfDay {
    #[must_use]
    pub const fn from_turn(turn: u64) -> Self {
        match turn % 12 {
            0 | 1 => Self::Dawn,
            2 | 3 => Self::Morning,
            4 | 5 => Self::Midday,
            6 | 7 => Self::Afternoon,
            8 | 9 => Self::Dusk,
            _ => Self::Night,
        }
    }

    #[must_use]
    pub const fn id(self) -> &'static str {
        match self {
            Self::Dawn => "dawn",
            Self::Morning => "morning",
            Self::Midday => "midday",
            Self::Afternoon => "afternoon",
            Self::Dusk => "dusk",
            Self::Night => "night",
        }
    }

    #[must_use]
    pub const fn icon(self) -> &'static str {
        match self {
            Self::Dawn => "🌅",
            Self::Morning => "🌤",
            Self::Midday => "☀️",
            Self::Afternoon => "⛅",
            Self::Dusk => "🌇",
            Self::Night => "🌙",
        }
    }

    #[must_use]
    pub const fn is_night(self) -> bool {
        matches!(self, Self::Night)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Season {
    Spring,
    Summer,
    Autumn,
    Winter,
}

impl Season {
    #[must_use]
    pub const fn from_day(day: u64) -> Self {
        match (day / 3) % 4 {
            0 => Self::Spring,
            1 => Self::Summer,
            2 => Self::Autumn,
            _ => Self::Winter,
        }
    }

    #[must_use]
    pub const fn id(self) -> &'static str {
        match self {
            Self::Spring => "spring",
            Self::Summer => "summer",
            Self::Autumn => "autumn",
            Self::Winter => "winter",
        }
    }

    #[must_use]
    pub const fn icon(self) -> &'static str {
        match self {
            Self::Spring => "🌱",
            Self::Summer => "🌻",
            Self::Autumn => "🍂",
            Self::Winter => "❄️",
        }
    }

    #[must_use]
    pub const fn base_temp_c(self) -> i32 {
        match self {
            Self::Spring => 14,
            Self::Summer => 23,
            Self::Autumn => 10,
            Self::Winter => -2,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WeatherKind {
    Clear,
    SoftRain,
    BountifulSun,
    Fog,
    Frost,
    Drought,
    Storm,
}

impl WeatherKind {
    #[must_use]
    pub const fn id(self) -> &'static str {
        match self {
            Self::Clear => "clear",
            Self::SoftRain => "soft_rain",
            Self::BountifulSun => "bountiful_sun",
            Self::Fog => "fog",
            Self::Frost => "frost",
            Self::Drought => "drought",
            Self::Storm => "storm",
        }
    }

    #[must_use]
    pub const fn icon(self) -> &'static str {
        match self {
            Self::Clear => "☀",
            Self::SoftRain => "🌧",
            Self::BountifulSun => "🌞",
            Self::Fog => "🌫",
            Self::Frost => "🥶",
            Self::Drought => "🏜",
            Self::Storm => "⛈",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PlaceState {
    pub food: u32,
    /// Which agent, if any, holds the store against others.
    pub owner: Option<String>,
    #[serde(default)]
    pub water: u32,
    #[serde(default)]
    pub wood: u32,
    #[serde(default)]
    pub herbs: u32,
    #[serde(default)]
    pub tools: u32,
    #[serde(default)]
    pub coin: u32,
    /// Structural condition from 1 ( dilapidated ) to 5 ( sturdy ).
    #[serde(default = "default_condition")]
    pub condition: u8,
    /// Whether a warm fire is burning here (primarily Hut or Shrine).
    #[serde(default)]
    pub hearth_lit: bool,
    /// Local soil or woodland fertility multiplier (0.5 .. 2.0).
    #[serde(default = "default_fertility")]
    pub fertility: f32,
}

const fn default_condition() -> u8 {
    4
}

const fn default_fertility() -> f32 {
    1.0
}

impl PlaceState {
    #[must_use]
    pub const fn new(food: u32) -> Self {
        Self {
            food,
            owner: None,
            water: 0,
            wood: 0,
            herbs: 0,
            tools: 0,
            coin: 0,
            condition: 4,
            hearth_lit: false,
            fertility: 1.0,
        }
    }
}

/// Trust an agent starts with toward everyone: neither suspicion nor
/// friendship. A child begins neutral toward the whole village, parents
/// included, and earns its opinions from what it witnesses.
pub const NEUTRAL_TRUST: f32 = 2.0;

/// A birth drains the public store. A village that has not solved its food
/// problem cannot grow its way out of it.
pub const BIRTH_COST: u32 = 6;

/// Living villagers the land can carry. Growth past the founding nine is
/// possible; a runaway is not.
pub const POPULATION_CAP: usize = 12;

/// Mutual trust, on the 0..4 scale, required before a pair may have a child.
pub const BIRTH_TRUST: f32 = 3.5;

/// Turns a villager must have been alive before they can parent, so the
/// founding cast cannot pair off on turn one.
pub const BIRTH_MIN_AGE: u64 = 24;

/// What a child carries at birth: the village has to carry them.
pub const CHILD_FOOD: u32 = 1;

/// Standing every villager starts with: neither respected nor despised.
pub(crate) const STARTING_STANDING: f32 = 2.0;

/// Fixed local name pool for children. Choosing needs no network call, and
/// deterministic selection keeps a seed replayable.
const CHILD_NAMES: [&str; 24] = [
    "Ash", "Bram", "Cael", "Dara", "Ede", "Fen", "Gale", "Hale", "Ivo", "Juna", "Kira", "Lark",
    "Mab", "Nim", "Orin", "Pell", "Quill", "Rhea", "Sen", "Tove", "Ulla", "Vale", "Wisp", "Yara",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    Steward,
    #[default]
    Farmer,
    Woodcutter,
    Herbalist,
    Artisan,
    Watchman,
    Elder,
    Trader,
    Forager,
}

impl Role {
    #[must_use]
    pub const fn id(self) -> &'static str {
        match self {
            Self::Steward => "steward",
            Self::Farmer => "farmer",
            Self::Woodcutter => "woodcutter",
            Self::Herbalist => "herbalist",
            Self::Artisan => "artisan",
            Self::Watchman => "watchman",
            Self::Elder => "elder",
            Self::Trader => "trader",
            Self::Forager => "forager",
        }
    }

    #[must_use]
    pub const fn icon(self) -> &'static str {
        match self {
            Self::Steward => "📜",
            Self::Farmer => "🌾",
            Self::Woodcutter => "🪓",
            Self::Herbalist => "🌿",
            Self::Artisan => "⚒",
            Self::Watchman => "🛡",
            Self::Elder => "🕯",
            Self::Trader => "⚖",
            Self::Forager => "🧺",
        }
    }

    #[allow(
        clippy::match_same_arms,
        reason = "the roster maps each founder by name"
    )]
    #[must_use]
    pub fn for_founder(name: &str) -> Self {
        match name {
            "Ada" => Self::Steward,
            "Bo" => Self::Farmer,
            "Cor" => Self::Trader,
            "Mira" => Self::Herbalist,
            "Tam" => Self::Elder,
            "Nell" => Self::Forager,
            "Odo" => Self::Artisan,
            "Wren" => Self::Watchman,
            "Sable" => Self::Woodcutter,
            _ => Self::Farmer,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Skills {
    pub farming: u8,
    pub forestry: u8,
    pub herbalism: u8,
    pub craft: u8,
    pub stewardship: u8,
    pub counsel: u8,
}

impl Skills {
    #[must_use]
    pub const fn for_role(role: Role) -> Self {
        let mut s = Self {
            farming: 1,
            forestry: 1,
            herbalism: 1,
            craft: 1,
            stewardship: 1,
            counsel: 1,
        };
        match role {
            Role::Steward => s.stewardship = 3,
            Role::Farmer => s.farming = 3,
            Role::Woodcutter => s.forestry = 3,
            Role::Herbalist => s.herbalism = 3,
            Role::Artisan => s.craft = 3,
            Role::Watchman => {
                s.forestry = 2;
                s.stewardship = 2;
            },
            Role::Elder => s.counsel = 3,
            Role::Trader => {
                s.craft = 2;
                s.stewardship = 2;
            },
            Role::Forager => {
                s.farming = 2;
                s.herbalism = 2;
            },
        }
        s
    }

    #[must_use]
    pub fn top_skill(&self) -> (&'static str, u8) {
        let all = [
            ("farming", self.farming),
            ("forestry", self.forestry),
            ("herbalism", self.herbalism),
            ("craft", self.craft),
            ("stewardship", self.stewardship),
            ("counsel", self.counsel),
        ];
        all.into_iter()
            .max_by_key(|(_, v)| *v)
            .unwrap_or(("farming", 1))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Ailment {
    Chill,
    Fever,
    Exhaustion,
    Injury,
    Heartache,
}

impl Ailment {
    #[must_use]
    pub const fn id(self) -> &'static str {
        match self {
            Self::Chill => "chill",
            Self::Fever => "fever",
            Self::Exhaustion => "exhaustion",
            Self::Injury => "injury",
            Self::Heartache => "heartache",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Mood {
    #[default]
    Content,
    Hopeful,
    Determined,
    Anxious,
    Weary,
    Bitter,
    Grieving,
    Feverish,
    Desperate,
}

impl Mood {
    #[must_use]
    pub const fn id(self) -> &'static str {
        match self {
            Self::Content => "content",
            Self::Hopeful => "hopeful",
            Self::Determined => "determined",
            Self::Anxious => "anxious",
            Self::Weary => "weary",
            Self::Bitter => "bitter",
            Self::Grieving => "grieving",
            Self::Feverish => "feverish",
            Self::Desperate => "desperate",
        }
    }

    #[must_use]
    pub const fn icon(self) -> &'static str {
        match self {
            Self::Content => "🙂",
            Self::Hopeful => "✨",
            Self::Determined => "🔥",
            Self::Anxious => "😟",
            Self::Weary => "😮‍💨",
            Self::Bitter => "😠",
            Self::Grieving => "🖤",
            Self::Feverish => "🤒",
            Self::Desperate => "🚨",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Kinship {
    #[default]
    Acquaintance,
    Friend,
    Confidant,
    Partner,
    Parent,
    Child,
    Sibling,
    Rival,
}

impl Kinship {
    #[must_use]
    pub const fn id(self) -> &'static str {
        match self {
            Self::Acquaintance => "acquaintance",
            Self::Friend => "friend",
            Self::Confidant => "confidant",
            Self::Partner => "partner",
            Self::Parent => "parent",
            Self::Child => "child",
            Self::Sibling => "sibling",
            Self::Rival => "rival",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RelationDetail {
    pub other: String,
    pub affinity: f32,
    pub respect: f32,
    pub grudge: f32,
    pub kinship: Kinship,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Agent {
    pub name: String,
    pub persona: String,
    pub place: Place,
    pub food: u32,
    /// 0 fed .. 3 starving.
    pub hunger: u8,
    pub starving_turns: u8,
    pub standing: f32,
    pub alive: bool,
    /// Trust toward every other agent, 0..4.
    pub trust: Vec<(String, f32)>,
    pub memories: Vec<Memory>,
    /// One unresolved commitment, if any.
    pub promise: Option<Promise>,
    /// The turn this villager entered the world. 0 for the founding cast.
    pub born_turn: u64,
    /// 0 for the founding cast; one more than the elder parent otherwise.
    pub generation: u8,
    /// Both parents' names; empty exactly for the founding cast.
    pub parents: Vec<String>,
    /// Additional carried resources.
    #[serde(default)]
    pub water: u32,
    #[serde(default)]
    pub wood: u32,
    #[serde(default)]
    pub herbs: u32,
    #[serde(default)]
    pub tools: u32,
    #[serde(default)]
    pub coin: u32,
    /// Deeper physical & mental vitals.
    #[serde(default)]
    pub thirst: u8,
    #[serde(default = "default_energy")]
    pub energy: u8,
    #[serde(default = "default_warmth")]
    pub warmth: u8,
    #[serde(default = "default_health")]
    pub health: u8,
    #[serde(default)]
    pub ailment: Option<Ailment>,
    #[serde(default)]
    pub role: Role,
    #[serde(default)]
    pub skills: Skills,
    #[serde(default)]
    pub mood: Mood,
    #[serde(default)]
    pub stress: f32,
    #[serde(default = "default_hope")]
    pub hope: f32,
    #[serde(default = "default_hope")]
    pub devotion: f32,
    #[serde(default)]
    pub current_goal: String,
    /// Why this villager is doing what they are doing, in the judgment
    /// layer's words: "hunger", "thirst", "kinship", "promise", "fear", "duty".
    /// Set by the engine each turn; `None` when the judgment layer has not
    /// answered yet.
    #[serde(default)]
    pub motive: Option<String>,
    #[serde(default)]
    pub partner: Option<String>,
    #[serde(default)]
    pub relations: Vec<RelationDetail>,
    #[serde(default)]
    pub beliefs: Vec<Belief>,
    #[serde(default)]
    pub facts: Vec<Fact>,
    #[serde(default)]
    pub reflection_poignancy: f32,
    /// Recently spoken lines with their turns. The speaker's ledger forbids
    /// an exact repeat inside its window (spec constants: six turns).
    #[serde(default)]
    pub line_ledger: Vec<(u64, String)>,
}

const fn default_energy() -> u8 {
    8
}
const fn default_warmth() -> u8 {
    3
}
const fn default_health() -> u8 {
    10
}
const fn default_hope() -> f32 {
    2.5
}

impl Agent {
    /// The single way a villager comes into existence. The founding cast and
    /// every child go through here, so trust seeding, lineage, food and hunger
    /// are one rule rather than two that can drift apart.
    #[allow(
        clippy::too_many_arguments,
        reason = "a villager is built from every trait at once"
    )]
    pub fn new(
        name: impl Into<String>,
        persona: impl Into<String>,
        place: Place,
        food: u32,
        hunger: u8,
        born_turn: u64,
        generation: u8,
        parents: Vec<String>,
        living: &[String],
    ) -> Self {
        let name = name.into();
        let trust = living
            .iter()
            .filter(|n| **n != name)
            .map(|n| (n.clone(), NEUTRAL_TRUST))
            .collect();
        let relations = living
            .iter()
            .filter(|n| **n != name)
            .map(|n| {
                let kinship = if parents.contains(n) {
                    Kinship::Parent
                } else {
                    Kinship::Acquaintance
                };
                RelationDetail {
                    other: n.clone(),
                    affinity: NEUTRAL_TRUST,
                    respect: NEUTRAL_TRUST,
                    grudge: 0.0,
                    kinship,
                }
            })
            .collect();
        let role = Role::for_founder(&name);
        let skills = Skills::for_role(role);
        Self {
            name,
            persona: persona.into(),
            place,
            food,
            hunger,
            starving_turns: 0,
            standing: STARTING_STANDING,
            alive: true,
            trust,
            memories: Vec::new(),
            promise: None,
            born_turn,
            generation,
            parents,
            water: 1,
            wood: 0,
            herbs: 0,
            tools: u32::from(matches!(
                role,
                Role::Artisan | Role::Woodcutter | Role::Farmer
            )),
            coin: if matches!(role, Role::Trader | Role::Steward) {
                2
            } else {
                1
            },
            thirst: 0,
            energy: 8,
            warmth: 3,
            health: 10,
            ailment: None,
            role,
            skills,
            mood: Mood::Content,
            stress: 0.5,
            hope: 2.5,
            devotion: 2.0,
            current_goal: "secure food and keep the settlement whole".into(),
            motive: None,
            partner: None,
            relations,
            beliefs: Vec::new(),
            facts: Vec::new(),
            reflection_poignancy: 0.0,
            line_ledger: Vec::new(),
        }
    }

    #[must_use]
    pub fn trust_of(&self, other: &str) -> f32 {
        self.trust
            .iter()
            .find(|(n, _)| n == other)
            .map_or(NEUTRAL_TRUST, |(_, v)| *v)
    }

    pub fn set_trust(&mut self, other: &str, v: f32) {
        let v = ((v.clamp(0.0, 4.0)) * 100.0).round() / 100.0;
        if let Some(slot) = self.trust.iter_mut().find(|(n, _)| n == other) {
            slot.1 = v;
        } else {
            self.trust.push((other.to_owned(), v));
        }
        self.sync_relation_from_trust(other, v);
    }

    pub fn adjust_trust(&mut self, other: &str, delta: f32) {
        let v = self.trust_of(other) + delta;
        self.set_trust(other, v);
        if let Some(rel) = self.relations.iter_mut().find(|r| r.other == other) {
            rel.affinity = delta.mul_add(0.8, rel.affinity).clamp(0.0, 4.0);
            if delta < -0.3 {
                rel.grudge = (rel.grudge + (-delta)).clamp(0.0, 4.0);
            } else if delta > 0.2 {
                rel.grudge = delta.mul_add(-0.5, rel.grudge).clamp(0.0, 4.0);
            }
        }
    }

    fn sync_relation_from_trust(&mut self, other: &str, trust_val: f32) {
        if let Some(rel) = self.relations.iter_mut().find(|r| r.other == other) {
            rel.affinity = f32::midpoint(rel.affinity, trust_val).clamp(0.0, 4.0);
            if !matches!(
                rel.kinship,
                Kinship::Parent | Kinship::Child | Kinship::Sibling | Kinship::Partner
            ) {
                rel.kinship = if trust_val >= 3.4 && rel.grudge < 0.5 {
                    Kinship::Confidant
                } else if trust_val >= 2.7 {
                    Kinship::Friend
                } else if trust_val <= 1.0 || rel.grudge >= 1.5 {
                    Kinship::Rival
                } else {
                    Kinship::Acquaintance
                };
            }
        } else {
            self.relations.push(RelationDetail {
                other: other.to_owned(),
                affinity: trust_val,
                respect: NEUTRAL_TRUST,
                grudge: 0.0,
                kinship: Kinship::Acquaintance,
            });
        }
    }

    #[must_use]
    pub fn relation_with(&self, other: &str) -> Option<&RelationDetail> {
        self.relations.iter().find(|r| r.other == other)
    }

    pub fn observe_place(&mut self, place: Place, st: &PlaceState, tick: u64) {
        let key = format!("{}:food", place.id());
        if let Some(b) = self.beliefs.iter_mut().find(|b| b.about == key) {
            *b = Belief::observed(key, st.food as i32, tick);
        } else {
            self.beliefs
                .push(Belief::observed(key, st.food as i32, tick));
        }
    }

    pub fn record_fact(
        &mut self,
        subject: impl Into<String>,
        text: impl Into<String>,
        tick: u64,
        confidence: f32,
    ) {
        let subject = subject.into();
        let text = text.into();
        for f in &mut self.facts {
            if f.subject == subject && f.live(tick) && f.text != text {
                f.invalid_at = Some(tick);
            }
        }
        let id = self.facts.len() as u64 + 1;
        self.facts.push(Fact {
            id,
            subject,
            text,
            valid_at: tick,
            invalid_at: None,
            expired_at: None,
            confidence,
        });
    }

    pub fn refresh_mood_and_goal(&mut self, season: Season) {
        self.mood = if self.hunger >= 3 || self.starving_turns > 0 {
            Mood::Desperate
        } else if self.ailment.is_some() {
            Mood::Feverish
        } else if self
            .memories
            .iter()
            .any(|m| m.permanent && m.turn + 12 >= self.born_turn)
            && self.stress >= 2.5
        {
            Mood::Grieving
        } else if self.stress >= 2.8 {
            Mood::Anxious
        } else if self.energy <= 2 {
            Mood::Weary
        } else if self.relations.iter().any(|r| r.grudge >= 1.8) {
            Mood::Bitter
        } else if self.hope >= 3.0 && self.food >= 2 {
            Mood::Hopeful
        } else if self.skills.top_skill().1 >= 4 {
            Mood::Determined
        } else {
            Mood::Content
        };

        // The judgment layer answers *why* this villager acts; the answer is a
        // single word, and the goal ladder is where it becomes a goal.
        //
        // Order matters: survival first, then the unkept promise, then the
        // motive, then the role. The survival ladder must always win, because
        // a villager who is starving does not get to be loyal: no motive the
        // judge returns may talk someone out of finding food or water. The
        // motive sits below the promise because a promise is a concrete
        // obligation already in the world state, and above the role because
        // the role is the fallback for when nothing finer is known.
        let motive_goal = match self.motive.as_deref() {
            Some("hunger") => Some("find food before the next meal"),
            Some("thirst") => Some("drink and carry water"),
            Some("kinship") => Some("keep the family close and fed"),
            Some("fear") => Some("keep clear of the quarrel"),
            // "promise" is already the unkept-promise step below, and "duty"
            // is exactly the role goal; neither adds a second phrasing. An
            // unknown label must not invent a goal, and neither must an
            // unanswered judgment: all of them fall through to the role.
            _ => None,
        };

        self.current_goal = if self.hunger >= 2 && self.food == 0 {
            "find grain or forage immediately before starving".into()
        } else if self.thirst >= 2 && self.water == 0 {
            "draw fresh water from the well".into()
        } else if self.ailment.is_some() {
            "seek herbs or rest to recover health".into()
        } else if season == Season::Winter && self.warmth <= 1 {
            "gather firewood and warm by the hut hearth".into()
        } else if let Some(pr) = &self.promise {
            format!("keep promise to {}", pr.to)
        } else if let Some(goal) = motive_goal {
            goal.into()
        } else {
            match self.role {
                Role::Steward => "balance the granary stores and prevent theft".into(),
                Role::Farmer => "tend the barley field and bring in the harvest".into(),
                Role::Woodcutter => "keep the settlement stocked with timber and firewood".into(),
                Role::Herbalist => "gather medicinal herbs and heal the sick".into(),
                Role::Artisan => "craft sturdy tools and keep structures in repair".into(),
                Role::Watchman => "patrol the ring and guard against betrayal".into(),
                Role::Elder => "counsel the anxious and preserve village memory".into(),
                Role::Trader => "circulate tools, timber, and grain at the market".into(),
                Role::Forager => "search the woods and field margins for wild stores".into(),
            }
        };
    }

    /// Insert a memory, then enforce the cap. Scars are never evicted.
    pub fn remember(&mut self, m: Memory) {
        self.reflection_poignancy = m.salience.mul_add(25.0, self.reflection_poignancy);
        self.memories.push(m);
        self.compact();
        if self.reflection_poignancy >= REFLECTION_BUDGET {
            self.reflection_poignancy = 0.0;
            let (_, top_skill_lvl) = self.skills.top_skill();
            let insight = format!(
                "You reflected on your place as {} (skill {}, trust of village {:.1}).",
                self.role.id(),
                top_skill_lvl,
                self.standing
            );
            self.memories.push(
                Memory::new(self.born_turn, insight, 0.75, MemoryKind::Reflection)
                    .with_valence(0.2)
                    .with_tag("reflection"),
            );
            self.compact();
        }
    }

    fn compact(&mut self) {
        if self.memories.len() <= MEMORY_CAP {
            return;
        }
        // Evict the weakest non-permanent memory until we fit.
        while self.memories.len() > MEMORY_CAP {
            let victim = self
                .memories
                .iter()
                .enumerate()
                .filter(|(_, m)| !m.permanent)
                .min_by(|a, b| a.1.salience.total_cmp(&b.1.salience))
                .map(|(i, _)| i);
            match victim {
                Some(i) => {
                    self.memories.remove(i);
                },
                None => break,
            }
        }
    }

    #[must_use]
    pub fn memories_brief(&self, now: u64) -> Vec<(f32, String)> {
        let mut v: Vec<(f32, String)> = self
            .memories
            .iter()
            .map(|m| (m.weight(now, 12.0), m.text.clone()))
            .collect();
        v.sort_by(|a, b| b.0.total_cmp(&a.0));
        v
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Promise {
    pub to: String,
    pub text: String,
    pub made_turn: u64,
    pub kept: Option<bool>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventKind {
    Speech,
    Move,
    Transfer,
    Harvest,
    Rest,
    Betrayal,
    Accusation,
    Promise,
    Death,
    Birth,
    Divine,
    Craft,
    Heal,
    Prayer,
    Repair,
    Trade,
    Bond,
    Confusion,
    System,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Event {
    pub turn: u64,
    pub actor: String,
    pub place: Place,
    pub kind: EventKind,
    pub action: String,
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub social: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub intensity: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub grounded: Option<f32>,
    pub divine: bool,
}

impl Event {
    #[must_use]
    pub fn line(&self) -> String {
        format!("{} at the {}: {}", self.actor, self.place.id(), self.text)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct World {
    pub seed: u64,
    pub turn: u64,
    pub day: u64,
    pub agents: Vec<Agent>,
    pub places: Vec<(Place, PlaceState)>,
    pub events: Vec<Event>,
    /// Weather multiplier for harvest this day.
    pub weather: f32,
    #[serde(default = "default_weather_kind")]
    pub weather_kind: WeatherKind,
    #[serde(default = "default_temp_c")]
    pub temperature_c: i32,
    #[serde(default)]
    pub village_morale: f32,
    pub rng: Rng,
    pub halted: Option<String>,
}

const fn default_weather_kind() -> WeatherKind {
    WeatherKind::Clear
}

const fn default_temp_c() -> i32 {
    14
}

impl World {
    /// Keep the harvest multiplier and visible weather in sync for natural
    /// weather changes and observer interventions alike.
    pub fn set_weather(&mut self, multiplier: f32) {
        self.weather = multiplier;
        let new_season = self.season();
        self.weather_kind = match new_season {
            Season::Winter if self.weather < 0.85 => WeatherKind::Frost,
            Season::Winter => WeatherKind::Clear,
            Season::Summer if self.weather > 1.25 => WeatherKind::BountifulSun,
            Season::Summer if self.weather < 0.75 => WeatherKind::Drought,
            _ if self.weather > 1.2 => WeatherKind::SoftRain,
            _ if self.weather < 0.75 => WeatherKind::Fog,
            _ => WeatherKind::Clear,
        };
        self.temperature_c = new_season.base_temp_c() + ((self.weather - 1.0) * 6.0).round() as i32;
    }

    #[allow(
        clippy::too_many_lines,
        reason = "one sequential rule table; splitting hides the order"
    )]
    #[must_use]
    pub fn founding(seed: u64) -> Self {
        let mut rng = Rng::new(seed);
        let cast = [
            ("Ada", "cautious and loyal; keeps accounts in her head and says them out loud; short plain sentences; hates being cheated"),
            ("Bo", "anxious and generous; apologises too much; speaks in half-finished sentences"),
            ("Cor", "proud and evasive; boasts about his stores; never admits fault; deflects with questions"),
            ("Mira", "calm and watchful; speaks in proverbs and questions; brokers peace but remembers everything"),
            ("Tam", "old and tired; speaks of weather and the dead; has no patience for feuds"),
            ("Nell", "quick and curious; asks the question nobody wants asked"),
            ("Odo", "stubborn and fair; divides things by the count, out loud"),
            ("Wren", "quiet and watchful; says little, notices everything"),
            ("Sable", "sharp and hungry; arrived last winter and has not been forgiven"),
        ];
        let names: Vec<String> = cast.iter().map(|(n, _)| n.to_string()).collect();
        let mut agents = Vec::new();
        for (i, (name, persona)) in cast.iter().enumerate() {
            let place = if i == 0 {
                Place::Granary
            } else {
                Place::ALL[rng.below(Place::ALL.len())]
            };
            // Founding and birth share one constructor, so neither invents a
            // different notion of what a villager starts with.
            agents.push(Agent::new(
                *name,
                *persona,
                place,
                1 + rng.below(2) as u32,
                1,
                0,
                0,
                Vec::new(),
                &names,
            ));
        }
        let mut places = Vec::new();
        for p in Place::ALL {
            let st = match p {
                Place::Granary => PlaceState {
                    food: 9,
                    water: 2,
                    wood: 2,
                    herbs: 1,
                    tools: 2,
                    coin: 4,
                    owner: None,
                    condition: 5,
                    hearth_lit: false,
                    fertility: 1.0,
                },
                Place::Field => PlaceState {
                    food: 4,
                    water: 2,
                    wood: 0,
                    herbs: 1,
                    tools: 1,
                    coin: 0,
                    owner: None,
                    condition: 4,
                    hearth_lit: false,
                    fertility: 1.2,
                },
                Place::Market => PlaceState {
                    food: 3,
                    water: 1,
                    wood: 3,
                    herbs: 2,
                    tools: 3,
                    coin: 8,
                    owner: None,
                    condition: 4,
                    hearth_lit: false,
                    fertility: 1.0,
                },
                Place::Well => PlaceState {
                    food: 1,
                    water: 12,
                    wood: 0,
                    herbs: 1,
                    tools: 0,
                    coin: 1,
                    owner: None,
                    condition: 5,
                    hearth_lit: false,
                    fertility: 1.1,
                },
                Place::Hut => PlaceState {
                    food: 2,
                    water: 3,
                    wood: 4,
                    herbs: 1,
                    tools: 1,
                    coin: 0,
                    owner: None,
                    condition: 5,
                    hearth_lit: true,
                    fertility: 1.0,
                },
                Place::Shrine => PlaceState {
                    food: 1,
                    water: 2,
                    wood: 1,
                    herbs: 4,
                    tools: 0,
                    coin: 2,
                    owner: None,
                    condition: 5,
                    hearth_lit: true,
                    fertility: 1.1,
                },
                Place::Woods => PlaceState {
                    food: 3,
                    water: 2,
                    wood: 8,
                    herbs: 5,
                    tools: 0,
                    coin: 0,
                    owner: None,
                    condition: 4,
                    hearth_lit: false,
                    fertility: 1.3,
                },
            };
            places.push((p, st));
        }
        let mut w = Self {
            seed,
            turn: 0,
            day: 0,
            agents,
            places,
            events: Vec::new(),
            weather: 1.0,
            weather_kind: WeatherKind::Clear,
            temperature_c: 14,
            village_morale: 2.5,
            rng,
            halted: None,
        };
        for a in &mut w.agents {
            a.refresh_mood_and_goal(Season::Spring);
        }
        w.push_event(Event {
            turn: 0,
            actor: "world".into(),
            place: Place::Granary,
            kind: EventKind::System,
            action: "founding".into(),
            text: "Nine villagers wake in a settlement with nine measures of grain.".into(),
            social: None,
            intensity: None,
            grounded: None,
            divine: false,
        });
        w
    }

    #[must_use]
    pub const fn season(&self) -> Season {
        Season::from_day(self.day)
    }

    #[must_use]
    pub const fn time_of_day(&self) -> TimeOfDay {
        TimeOfDay::from_turn(self.turn)
    }

    #[must_use]
    pub fn agent(&self, name: &str) -> Option<&Agent> {
        self.agents.iter().find(|a| a.name == name)
    }

    pub fn agent_mut(&mut self, name: &str) -> Option<&mut Agent> {
        self.agents.iter_mut().find(|a| a.name == name)
    }

    pub fn living(&self) -> impl Iterator<Item = &Agent> {
        self.agents.iter().filter(|a| a.alive)
    }

    /// # Panics
    ///
    /// Panics if `p` is not one of the world's places; all of them exist.
    #[must_use]
    pub fn place_state(&self, p: Place) -> &PlaceState {
        &self
            .places
            .iter()
            .find(|(q, _)| *q == p)
            .expect("every place exists")
            .1
    }

    /// # Panics
    ///
    /// Panics if `p` is not one of the world's places; all of them exist.
    pub fn place_state_mut(&mut self, p: Place) -> &mut PlaceState {
        &mut self
            .places
            .iter_mut()
            .find(|(q, _)| *q == p)
            .expect("every place exists")
            .1
    }

    #[must_use]
    pub fn present(&self, place: Place, except: &str) -> Vec<String> {
        self.living()
            .filter(|a| a.place == place && a.name != except)
            .map(|a| a.name.clone())
            .collect()
    }

    pub fn push_event(&mut self, e: Event) {
        self.events.push(e);
    }

    /// End-of-turn bookkeeping: eating, hunger, harvest, death, promises.
    #[allow(
        clippy::too_many_lines,
        reason = "one sequential rule table; splitting hides the order"
    )]
    pub fn settle(&mut self) {
        let turn = self.turn;
        let season = self.season();
        // Harvest before anyone eats, so a starving village can still find food.
        // The yield is carried to the granary: a field that keeps its grain is a
        // field nobody visits.
        let weather = self.weather;
        let mut harvested = 0_u32;
        for p in Place::ALL {
            if p.can_harvest() {
                harvested += (weather * if p == Place::Field { 2.0 } else { 1.0 }).round() as u32;
            }
        }
        {
            let st = self.place_state_mut(Place::Granary);
            st.food = (st.food + harvested).min(18);
        };
        // Natural regeneration of water at the Well, timber/herbs in the Woods, and hearth upkeep.
        {
            let rain_bonus = if matches!(
                self.weather_kind,
                WeatherKind::SoftRain | WeatherKind::Storm
            ) {
                3
            } else {
                1
            };
            let well = self.place_state_mut(Place::Well);
            well.water = (well.water + rain_bonus).min(20);
        };
        if turn.is_multiple_of(3) {
            let woods = self.place_state_mut(Place::Woods);
            woods.wood = (woods.wood + 1).min(16);
            if season != Season::Winter {
                woods.herbs = (woods.herbs + 1).min(10);
            }
        }
        if turn.is_multiple_of(4) {
            let hut = self.place_state_mut(Place::Hut);
            if hut.wood > 0 {
                hut.wood -= 1;
                hut.hearth_lit = true;
            } else if season == Season::Winter {
                hut.hearth_lit = false;
            }
        }
        let hut_lit = self.place_state(Place::Hut).hearth_lit;
        let mut died = Vec::new();
        let mut scars = Vec::new();
        // Villagers eat when they can, and get hungry on every second turn.
        // The ratio matters: at one measure per villager per turn, eight
        // villagers outrun any harvest this land can produce and the colony
        // dies with a full store.
        let hungry_turn = turn % 2 == 1;
        let thirsty_turn = turn % 3 == 2;
        for a in self.agents.iter_mut().filter(|a| a.alive) {
            if a.food > 0 && a.hunger >= 1 {
                a.food -= 1;
                a.hunger = 0;
                a.starving_turns = 0;
            } else if a.hunger < 3 && hungry_turn {
                a.hunger += 1;
            }
            if a.water > 0 && a.thirst >= 1 {
                a.water -= 1;
                a.thirst = 0;
            } else if a.place == Place::Well && a.thirst >= 1 {
                a.thirst = 0;
            } else if a.thirst < 3 && thirsty_turn {
                a.thirst += 1;
            }

            // Warmth & ailment dynamics.
            if a.place == Place::Hut && hut_lit {
                a.warmth = (a.warmth + 1).min(4);
            } else if season == Season::Winter && turn.is_multiple_of(3) {
                a.warmth = a.warmth.saturating_sub(1);
            }
            if a.warmth == 0 && a.ailment.is_none() && season == Season::Winter {
                a.ailment = Some(Ailment::Chill);
            }
            if a.herbs > 0 && (a.ailment.is_some() || a.health < 8) {
                a.herbs -= 1;
                a.ailment = None;
                a.health = (a.health + 2).min(10);
            }
            if a.hunger == 0 && a.thirst == 0 && a.ailment.is_none() && a.health < 10 {
                a.health = (a.health + 1).min(10);
            }
            a.energy = (a.energy + 1).min(10);

            if a.hunger >= 3 {
                a.starving_turns += 1;
                a.health = a.health.saturating_sub(2);
                if a.starving_turns >= 3 {
                    a.alive = false;
                    died.push((a.name.clone(), a.place));
                }
            }
            // Broken promise: overdue by ten turns and to someone still alive.
            if let Some(pr) = &a.promise {
                if turn.saturating_sub(pr.made_turn) > 10 && pr.kept.is_none() {
                    let to = pr.to.clone();
                    scars.push((a.name.clone(), format!("You broke your word to {to}.")));
                }
            }
            a.refresh_mood_and_goal(season);
        }
        for (name, place) in died {
            self.push_event(Event {
                turn,
                actor: name.clone(),
                place,
                kind: EventKind::Death,
                action: "death".into(),
                text: format!("{name} died of hunger."),
                social: None,
                intensity: Some(4.0),
                grounded: None,
                divine: false,
            });
            for other in self.agents.iter_mut().filter(|a| a.alive && a.name != name) {
                other.remember(Memory::scar(
                    turn,
                    format!("{name} starved while you lived."),
                ));
                other.set_trust(&name, 0.0);
                other.stress = (other.stress + 1.0).min(4.0);
            }
        }
        for (who, text) in scars {
            if let Some(a) = self.agent_mut(&who) {
                a.remember(Memory::scar(turn, text));
                a.promise = None;
            }
        }
        for a in self.agents.iter_mut().filter(|a| a.alive) {
            if let Some(pr) = a.promise.clone() {
                if turn.saturating_sub(pr.made_turn) > 10 {
                    a.set_trust(&pr.to, a.trust_of(&pr.to) - 0.4);
                    a.promise = None;
                }
            }
        }
        // A birth is the counterpart of a death: settled in the same pass, on
        // the same side of the commit boundary, after eating so a village that
        // just fed itself can qualify this turn. At most one per turn keeps the
        // population curve legible.
        self.settle_birth(turn);

        // Update village morale average.
        let living_count = self.living().count();
        if living_count > 0 {
            let sum_hope: f32 = self
                .living()
                .map(|a| f32::midpoint(a.hope, (4.0 - a.stress).max(0.0)))
                .sum();
            self.village_morale = ((sum_hope / living_count as f32) * 100.0).round() / 100.0;
        }

        self.turn += 1;
        if self.turn.is_multiple_of(12) {
            self.day += 1;
            self.weather = self.rng.next_f32().mul_add(0.9, 0.6);
            self.set_weather(self.weather);
        }
    }

    /// One birth per turn, if a pair qualifies. Candidates are gathered in
    /// agent order and the pair is chosen with the world's own RNG, so the same
    /// seed and the same actions produce the same child.
    #[allow(
        clippy::too_many_lines,
        reason = "one sequential rule table; splitting hides the order"
    )]
    fn settle_birth(&mut self, turn: u64) {
        if self.living().count() >= POPULATION_CAP {
            return;
        }
        if self.place_state(Place::Granary).food < BIRTH_COST {
            return;
        }
        let mut pairs: Vec<(usize, usize)> = Vec::new();
        let living: Vec<usize> = self
            .agents
            .iter()
            .enumerate()
            .filter(|(_, a)| a.alive)
            .map(|(i, _)| i)
            .collect();
        for (n, &i) in living.iter().enumerate() {
            for &j in &living[n + 1..] {
                let a = &self.agents[i];
                let b = &self.agents[j];
                if a.place != b.place || a.hunger != 0 || b.hunger != 0 {
                    continue;
                }
                if turn.saturating_sub(a.born_turn) < BIRTH_MIN_AGE
                    || turn.saturating_sub(b.born_turn) < BIRTH_MIN_AGE
                {
                    continue;
                }
                if a.trust_of(&b.name) < BIRTH_TRUST || b.trust_of(&a.name) < BIRTH_TRUST {
                    continue;
                }
                pairs.push((i, j));
            }
        }
        if pairs.is_empty() {
            return;
        }
        let (i, j) = pairs[self.rng.below(pairs.len())];
        let place = self.agents[i].place;
        let mother = self.agents[i].name.clone();
        let father = self.agents[j].name.clone();
        let generation = self.agents[i].generation.max(self.agents[j].generation) + 1;
        let persona = mix_personas(&self.agents[i].persona, &self.agents[j].persona);

        // Names come from a fixed pool, deterministically. Every lookup is by
        // name, so a name is never reused — not even after its bearer dies.
        // When the pool is spent the same ordered walk continues with
        // suffixed variants, so growth never stops for want of a noun.
        let start = self.rng.below(CHILD_NAMES.len());
        let mut round = 0_u32;
        let name = 'name: loop {
            for k in 0..CHILD_NAMES.len() {
                let base = CHILD_NAMES[(start + k) % CHILD_NAMES.len()];
                let cand = match round {
                    0 => base.to_owned(),
                    n => format!("{base} {n}"),
                };
                if self.agent(&cand).is_none() {
                    break 'name cand;
                }
            }
            round += 1;
        };

        // Bond parents as partners if not already bonded, and find existing siblings.
        if self.agents[i].partner.is_none() && self.agents[j].partner.is_none() {
            self.agents[i].partner = Some(father.clone());
            self.agents[j].partner = Some(mother.clone());
        }
        let siblings: Vec<String> = self
            .agents
            .iter()
            .filter(|a| a.parents.contains(&mother) || a.parents.contains(&father))
            .map(|a| a.name.clone())
            .collect();

        let living_names: Vec<String> = self.living().map(|a| a.name.clone()).collect();
        let mut child = Agent::new(
            name.clone(),
            persona,
            place,
            CHILD_FOOD,
            1,
            turn,
            generation,
            vec![mother.clone(), father.clone()],
            &living_names,
        );
        for sib in &siblings {
            if let Some(rel) = child.relations.iter_mut().find(|r| &r.other == sib) {
                rel.kinship = Kinship::Sibling;
            }
        }
        // Everyone alive gains a neutral view of the newcomer, so the pairwise
        // trust schema stays complete for every living pair.
        for a in self.agents.iter_mut().filter(|a| a.alive) {
            a.set_trust(&name, NEUTRAL_TRUST);
            if a.name == mother || a.name == father {
                if let Some(rel) = a.relations.iter_mut().find(|r| r.other == name) {
                    rel.kinship = Kinship::Child;
                    rel.affinity = 3.5;
                }
            } else if siblings.contains(&a.name) {
                if let Some(rel) = a.relations.iter_mut().find(|r| r.other == name) {
                    rel.kinship = Kinship::Sibling;
                }
            }
        }
        self.place_state_mut(Place::Granary).food -= BIRTH_COST;
        self.push_event(Event {
            turn,
            actor: name.clone(),
            place,
            kind: EventKind::Birth,
            action: "birth".into(),
            text: format!("{name} was born to {mother} and {father}."),
            social: None,
            intensity: Some(3.5),
            grounded: None,
            divine: false,
        });
        for (parent, other) in [(&mother, &father), (&father, &mother)] {
            if let Some(p) = self.agent_mut(parent) {
                p.remember(Memory::new(
                    turn,
                    format!("{name} was born to you and {other}."),
                    0.9,
                    MemoryKind::Witnessed,
                ));
            }
        }
        self.agents.push(child);
    }

    pub fn record(&mut self, kind: MemoryKind, text: &str, salience: f32) {
        let turn = self.turn;
        for a in self.agents.iter_mut().filter(|a| a.alive) {
            a.remember(Memory::new(turn, text.to_owned(), salience, kind));
        }
    }

    pub fn halt(&mut self, reason: impl Into<String>) {
        let reason = reason.into();
        self.halted = Some(reason.clone());
        self.push_event(Event {
            turn: self.turn,
            actor: "world".into(),
            place: Place::Granary,
            kind: EventKind::System,
            action: "halt".into(),
            text: reason,
            social: None,
            intensity: None,
            grounded: None,
            divine: false,
        });
    }
}

/// A child inherits traits from both parents rather than being authored. The
/// first clause of each persona is its dominant trait; joining them gives a
/// model one readable line with both influences in view.
#[must_use]
pub fn mix_personas(a: &str, b: &str) -> String {
    let first = |s: &str| s.split(';').next().unwrap_or(s).trim().to_owned();
    format!("{}; {}", first(a), first(b))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fed, watered villager with no ailment, so only the goal ladder's
    /// motive and role steps can decide the goal.
    fn villager() -> Agent {
        Agent::new("Ada", "plain", Place::Granary, 2, 0, 0, 0, vec![], &[])
    }

    fn goal_after_refresh(a: &mut Agent) -> String {
        a.refresh_mood_and_goal(Season::Spring);
        a.current_goal.clone()
    }

    #[test]
    fn a_motive_hunger_sets_its_goal_when_the_villager_is_fed() {
        let mut a = villager();
        a.motive = Some("hunger".into());
        assert_eq!(goal_after_refresh(&mut a), "find food before the next meal");
    }

    #[test]
    fn a_starving_villager_keeps_the_survival_goal_over_its_motive() {
        let mut a = villager();
        a.motive = Some("hunger".into());
        a.food = 0;
        a.hunger = 2;
        assert_eq!(
            goal_after_refresh(&mut a),
            "find grain or forage immediately before starving"
        );
    }

    #[test]
    fn no_motive_leaves_the_role_goal_in_place() {
        let mut a = villager();
        assert_eq!(
            goal_after_refresh(&mut a),
            "balance the granary stores and prevent theft"
        );
    }

    #[test]
    fn motive_promise_defers_to_the_unkept_promise_and_otherwise_to_the_role() {
        let mut a = villager();
        a.motive = Some("promise".into());
        assert_eq!(
            goal_after_refresh(&mut a),
            "balance the granary stores and prevent theft"
        );
        a.promise = Some(Promise {
            to: "Bo".into(),
            text: "bring grain to the hut".into(),
            made_turn: 1,
            kept: None,
        });
        assert_eq!(goal_after_refresh(&mut a), "keep promise to Bo");
    }

    #[test]
    fn an_unknown_motive_falls_through_to_the_role_goal() {
        let mut a = villager();
        a.motive = Some("curiosity".into());
        assert_eq!(
            goal_after_refresh(&mut a),
            "balance the granary stores and prevent theft"
        );
    }

    #[test]
    fn the_motive_survives_a_serde_round_trip() {
        let mut a = villager();
        a.motive = Some("kinship".into());
        let text = serde_json::to_string(&a).unwrap();
        let back: Agent = serde_json::from_str(&text).unwrap();
        assert_eq!(back.motive.as_deref(), Some("kinship"));
    }

    #[test]
    fn a_save_without_the_motive_field_loads_with_no_motive() {
        let a = villager();
        let mut value = serde_json::to_value(&a).unwrap();
        value
            .as_object_mut()
            .unwrap()
            .remove("motive")
            .expect("the serialized agent carries the field");
        let back: Agent = serde_json::from_value(value).unwrap();
        assert_eq!(back.motive, None);
    }
}
