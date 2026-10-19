//! The window. Deliberately read-mostly: the society runs in the engine and is
//! driven through the dev server, so the UI is an observer, not the owner of
//! state.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::{Duration, Instant};

use eframe::egui;
use egui::{Color32, RichText};
use society_server::legacy::AgentTurn;
use society_server::{AppState, TurnRecord};
use society_sim::{Agent, Event, EventKind, Place, PlaceState, World};

const MAP_W: f32 = 420.0;
const MAP_H: f32 = 360.0;

/// Everything the command line decides about how the window opens.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Launch<'a> {
    pub port: u16,
    pub view: &'a str,
    pub select: Option<&'a str>,
    pub expand_state: bool,
    pub select_none: bool,
    pub size: Option<(f32, f32)>,
    pub autostart: bool,
    pub focus: bool,
}

pub(crate) fn run(
    rt: tokio::runtime::Handle,
    state: AppState,
    launch: Launch<'_>,
) -> Result<(), eframe::Error> {
    let Launch {
        port, size, focus, ..
    } = launch;
    // winit activates the whole app on launch by default, which would pull the
    // user out of whatever they are doing. Stay passive unless --focus was
    // explicitly requested.
    let options = eframe::NativeOptions {
        // On macOS winit activates the app on launch, which pulls the user out
        // of whatever they are doing. Only activate when --focus is requested.
        #[cfg(target_os = "macos")]
        event_loop_builder: Some(Box::new(move |builder| {
            use winit::platform::macos::EventLoopBuilderExtMacOS as _;
            builder.with_activate_ignoring_other_apps(focus);
        })),
        viewport: egui::ViewportBuilder::default()
            .with_inner_size(size.unwrap_or((1120.0, 760.0)))
            .with_min_inner_size([900.0, 600.0])
            // Remain unfocused by default; --focus explicitly opens in front.
            .with_active(focus)
            .with_title(format!("society — :{port}")),
        ..Default::default()
    };
    eframe::run_native(
        "society",
        options,
        Box::new(move |_cc| Ok(Box::new(App::new(rt, state, launch)))),
    )
}

struct App {
    rt: tokio::runtime::Handle,
    state: AppState,
    port: u16,
    snapshot: Option<Snapshot>,
    turns: Vec<TurnRecord>,
    selected: Option<String>,
    selected_place: Option<String>,
    inspector: Option<Inspector>,
    tab: Tab,
    roster_filter: RosterFilter,
    roster_search: String,
    feed_filter: FeedFilter,
    feed_search: String,
    custom_event_text: String,
    theme_applied: bool,
    /// Open the judgment-state disclosure on first draw. Screenshot aid only.
    expand_state: bool,
    auto: bool,
    speed: f32,
    last_turn: Instant,
    status: String,
    busy: bool,
    pending: Option<PendingTurn>,
}

/// A turn running on the runtime: where its result arrives, and when it began.
type PendingTurn = (
    std::sync::mpsc::Receiver<(Vec<TurnRecord>, Option<String>)>,
    Instant,
);

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    World,
    Council,
    Feed,
    Inspector,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum RosterFilter {
    All,
    Living,
    Distressed,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum FeedFilter {
    All,
    Social,
    Economy,
    Crisis,
    Sacred,
}

#[derive(Clone, Copy)]
enum VillageIntervention {
    RingBell,
    HarvestFeast,
    MerchantCaravan,
    ShrineBlessing,
    HerbalRemedy,
}

#[derive(Clone)]
struct Snapshot {
    turn: u64,
    day: u64,
    season: String,
    time_of_day: String,
    weather_kind: String,
    weather: f32,
    temperature_c: i32,
    village_morale: f32,
    halted: Option<String>,
    agents: Vec<AgentView>,
    places: Vec<PlaceView>,
    events: Vec<Event>,
    report: VillageReport,
}

/// Keep the full simulation records so new fields cannot silently disappear
/// between the engine and the observer again.
#[derive(Clone)]
struct AgentView {
    agent: Agent,
    death: Option<DeathView>,
    legal_actions: Vec<String>,
}

impl std::ops::Deref for AgentView {
    type Target = Agent;
    fn deref(&self) -> &Agent {
        &self.agent
    }
}

#[derive(Clone)]
struct DeathView {
    turn: u64,
    cause: String,
}

#[derive(Clone)]
struct PlaceView {
    place: Place,
    state: PlaceState,
    present: Vec<String>,
}

#[derive(Clone)]
struct VillageReport {
    era_name: String,
    daily_phase: String,
    daily_detail: String,
    leader: Option<LeaderView>,
    decrees: Vec<DecreeView>,
    commodities: Vec<CommodityQuote>,
    prosperity_score: f32,
    gini_coin: f32,
    households: Vec<HouseholdView>,
    factions: Vec<FactionView>,
    feuds: Vec<FeudView>,
    rumors: Vec<RumorView>,
    memorials: Vec<MemorialView>,
}

#[derive(Clone)]
struct LeaderView {
    name: String,
    role: String,
    title: String,
    support: f32,
}

#[derive(Clone)]
struct DecreeView {
    title: String,
    domain: String,
    summary: String,
    urgent: bool,
}

#[derive(Clone)]
struct CommodityQuote {
    name: &'static str,
    stockpile: u32,
    carried: u32,
    total: u32,
    price_coin: f32,
    status: &'static str,
}

#[derive(Clone)]
struct HouseholdView {
    title: String,
    hearth_place: String,
    members: Vec<String>,
    wealth: u32,
    food: u32,
    tradition: String,
}

#[derive(Clone)]
struct FactionView {
    name: String,
    ethos: String,
    members: Vec<String>,
    cohesion: f32,
}

#[derive(Clone)]
struct FeudView {
    a: String,
    b: String,
    grudge: f32,
    reason: String,
}

#[derive(Clone)]
struct RumorView {
    speaker: String,
    subject: String,
    text: String,
    turn: u64,
    kind: &'static str,
}

#[derive(Clone)]
struct MemorialView {
    name: String,
    role: String,
    generation: u8,
    turn: u64,
    cause: String,
}

/// Format a concise multi-line text summary of the village's civic, economic,
/// and social state for CLI inspection.
pub(crate) fn format_village_digest(w: &World) -> String {
    let rep = VillageReport::analyze(w);
    let leader = rep.leader.as_ref().map_or_else(
        || "None elected".into(),
        |l| format!("{} ({}, support {:.1})", l.name, l.title, l.support),
    );
    let decrees = rep
        .decrees
        .iter()
        .map(|d| d.title.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "  era: {} · schedule: {}\n  reeve: {} · prosperity: {:.1}/100 · decrees: [{}]",
        rep.era_name, rep.daily_phase, leader, rep.prosperity_score, decrees
    )
}

/// Execute emergent village-level systemic mechanics after a simulation turn:
/// emergency parish rationing, herbalist triage, guild apprenticeships,
/// shrine counsel, and seasonal festivals.
pub(crate) fn apply_village_systems(w: &mut World) -> usize {
    if w.turn == 0 {
        return 0;
    }
    let mut emitted = Vec::new();
    let turn = w.turn;
    let season_id = w.season().id().to_owned();
    let temp = w.temperature_c;

    // 1. Seasonal Festival & Parish Gathering every 12 turns.
    if turn.is_multiple_of(12) {
        let (festival_name, festival_place, text) = match season_id.as_str() {
            "spring" => (
                "festival:sowing",
                "shrine",
                "The village gathered at the Shrine for the Spring Sowing Blessing, sharing seed-cake and renewing vows.",
            ),
            "summer" => (
                "festival:fair",
                "market",
                "Lanterns hung above the Market for the High Summer Fair as villagers traded crafts and sang old ballads.",
            ),
            "autumn" => (
                "festival:harvest_feast",
                "granary",
                "The long trestle tables by the Granary groaned under the Autumn Harvest Feast, easing old grievances.",
            ),
            _ => (
                "festival:hearth_vigil",
                "hut",
                "Villagers kept the Midwinter Hearth Vigil together, banking embers against the deep frost.",
            ),
        };
        let target_place = find_place(w, festival_place);
        let mut actor_name = "Village Council".to_owned();
        for a in w.agents.iter_mut().filter(|a| a.alive) {
            if actor_name == "Village Council" {
                actor_name.clone_from(&a.name);
            }
            a.hope = (a.hope + 0.35).min(4.0);
            a.stress = (a.stress - 0.30).max(0.0);
            if season_id == "autumn" && a.hunger > 0 {
                a.hunger = a.hunger.saturating_sub(1);
            }
            if season_id == "winter" {
                a.warmth = (a.warmth + 1).min(4);
            }
        }
        w.village_morale = (w.village_morale + 0.25).min(4.0);
        emitted.push(Event {
            turn,
            actor: actor_name,
            place: target_place,
            kind: EventKind::System,
            action: festival_name.into(),
            text: text.into(),
            social: Some("communal".into()),
            intensity: Some(0.85),
            grounded: Some(1.0),
            divine: false,
        });
    }

    // 2. Parish Mutual Aid & Emergency Rationing for critically starving/parched villagers.
    let needy: Vec<(String, Place, bool, bool)> = w
        .agents
        .iter()
        .filter(|a| a.alive && ((a.hunger >= 2 && a.food == 0) || (a.thirst >= 2 && a.water == 0)))
        .map(|a| {
            (
                a.name.clone(),
                a.place,
                a.hunger >= 2 && a.food == 0,
                a.thirst >= 2 && a.water == 0,
            )
        })
        .collect();

    for (recipient_name, place, needs_food, needs_water) in needy {
        if needs_food {
            if let Some(donor_idx) = w.agents.iter().position(|d| {
                d.alive
                    && d.name != recipient_name
                    && d.food >= 3
                    && (d.place == place || d.role.id() == "steward")
            }) {
                let donor_name = w.agents[donor_idx].name.clone();
                w.agents[donor_idx].food -= 1;
                w.agents[donor_idx].standing = (w.agents[donor_idx].standing + 0.12).min(4.0);
                if let Some(rec) = w.agent_mut(&recipient_name) {
                    rec.food += 1;
                    rec.hope = (rec.hope + 0.2).min(4.0);
                }
                emitted.push(Event {
                    turn,
                    actor: donor_name.clone(),
                    place,
                    kind: EventKind::Transfer,
                    action: "parish_relief".into(),
                    text: format!(
                        "{donor_name} distributed emergency parish grain to {recipient_name}."
                    ),
                    social: Some("altruistic".into()),
                    intensity: Some(0.65),
                    grounded: Some(1.0),
                    divine: false,
                });
            }
        }
        if needs_water {
            if let Some(donor_idx) = w.agents.iter().position(|d| {
                d.alive
                    && d.name != recipient_name
                    && d.water >= 3
                    && (d.place == place || d.role.id() == "steward")
            }) {
                let donor_name = w.agents[donor_idx].name.clone();
                w.agents[donor_idx].water -= 1;
                if let Some(rec) = w.agent_mut(&recipient_name) {
                    rec.water += 1;
                }
                emitted.push(Event {
                    turn,
                    actor: donor_name.clone(),
                    place,
                    kind: EventKind::Transfer,
                    action: "water_relief".into(),
                    text: format!(
                        "{donor_name} shared a skin of well water with parched {recipient_name}."
                    ),
                    social: Some("supportive".into()),
                    intensity: Some(0.60),
                    grounded: Some(1.0),
                    divine: false,
                });
            }
        }
    }

    // 3. Herbalist Apothecary Triage for co-located sick villagers.
    if turn.is_multiple_of(2) {
        let healers: Vec<(String, Place)> = w
            .agents
            .iter()
            .filter(|a| {
                a.alive && a.herbs >= 1 && (a.skills.herbalism >= 3 || a.role.id() == "herbalist")
            })
            .map(|a| (a.name.clone(), a.place))
            .collect();
        for (healer_name, place) in healers {
            if let Some(patient_name) = w
                .agents
                .iter()
                .find(|p| {
                    p.alive
                        && p.name != healer_name
                        && p.place == place
                        && (p.ailment.is_some() || p.health <= 6)
                })
                .map(|p| p.name.clone())
            {
                if let Some(h) = w.agent_mut(&healer_name) {
                    if h.herbs == 0 {
                        continue;
                    }
                    h.herbs -= 1;
                    h.standing = (h.standing + 0.1).min(4.0);
                }
                if let Some(p) = w.agent_mut(&patient_name) {
                    p.health = (p.health + 2).min(10);
                    p.stress = (p.stress - 0.25).max(0.0);
                    if p.health >= 8 {
                        p.ailment = None;
                    }
                }
                emitted.push(Event {
                    turn,
                    actor: healer_name.clone(),
                    place,
                    kind: EventKind::Heal,
                    action: "poultice_triage".into(),
                    text: format!(
                        "{healer_name} steeped bitter herbs to tend {patient_name}'s constitution."
                    ),
                    social: Some("caring".into()),
                    intensity: Some(0.70),
                    grounded: Some(1.0),
                    divine: false,
                });
                break;
            }
        }
    }

    // 4. Guild Apprenticeship & Skill Transmission every 5 turns.
    if turn.is_multiple_of(5) {
        let mut mentored: Option<(String, String, Place, &'static str)> = None;
        for i in 0..w.agents.len() {
            for j in 0..w.agents.len() {
                if i == j || !w.agents[i].alive || !w.agents[j].alive {
                    continue;
                }
                if w.agents[i].place != w.agents[j].place {
                    continue;
                }
                let mentor = &w.agents[i];
                let pupil = &w.agents[j];
                let place = mentor.place;
                let skill_match = match place.id() {
                    "field" if mentor.skills.farming >= 4 && pupil.skills.farming < 3 => {
                        Some("farming")
                    },
                    "woods" if mentor.skills.forestry >= 4 && pupil.skills.forestry < 3 => {
                        Some("forestry")
                    },
                    "market" | "hut" if mentor.skills.craft >= 4 && pupil.skills.craft < 3 => {
                        Some("craft")
                    },
                    "shrine" if mentor.skills.counsel >= 4 && pupil.skills.counsel < 3 => {
                        Some("counsel")
                    },
                    "granary" if mentor.skills.stewardship >= 4 && pupil.skills.stewardship < 3 => {
                        Some("stewardship")
                    },
                    _ if mentor.skills.herbalism >= 4 && pupil.skills.herbalism < 3 => {
                        Some("herbalism")
                    },
                    _ => None,
                };
                if let Some(skill) = skill_match {
                    mentored = Some((mentor.name.clone(), pupil.name.clone(), place, skill));
                    break;
                }
            }
            if mentored.is_some() {
                break;
            }
        }
        if let Some((mentor_name, pupil_name, place, skill)) = mentored {
            if let Some(pupil) = w.agent_mut(&pupil_name) {
                match skill {
                    "farming" => pupil.skills.farming = (pupil.skills.farming + 1).min(5),
                    "forestry" => pupil.skills.forestry = (pupil.skills.forestry + 1).min(5),
                    "craft" => pupil.skills.craft = (pupil.skills.craft + 1).min(5),
                    "counsel" => pupil.skills.counsel = (pupil.skills.counsel + 1).min(5),
                    "stewardship" => {
                        pupil.skills.stewardship = (pupil.skills.stewardship + 1).min(5);
                    },
                    _ => pupil.skills.herbalism = (pupil.skills.herbalism + 1).min(5),
                }
            }
            emitted.push(Event {
                turn,
                actor: mentor_name.clone(),
                place,
                kind: EventKind::Bond,
                action: "guild_apprenticeship".into(),
                text: format!(
                    "{mentor_name} instructed {pupil_name} in the finer points of {skill}."
                ),
                social: Some("mentorship".into()),
                intensity: Some(0.55),
                grounded: Some(1.0),
                divine: false,
            });
        }
    }

    // 5. Cold Snap Hearth Huddle when temperature drops near freezing.
    if temp <= 2 && turn.is_multiple_of(4) {
        let hut = find_place(w, "hut");
        for a in w.agents.iter_mut().filter(|a| a.alive && a.place == hut) {
            a.warmth = (a.warmth + 1).min(4);
        }
    }

    let count = emitted.len();
    for ev in emitted {
        w.push_event(ev);
    }
    count
}

fn find_place(w: &World, id: &str) -> Place {
    w.places
        .iter()
        .find(|(p, _)| p.id() == id)
        .map_or(Place::Market, |(p, _)| *p)
}

impl VillageReport {
    fn analyze(w: &World) -> Self {
        let living: Vec<&Agent> = w.living().collect();
        let pop = living.len().max(1) as f32;

        // 1. Elect / recognize Reeve (Village Leader)
        let leader = living
            .iter()
            .max_by(|a, b| {
                let score = |x: &Agent| {
                    let incoming_trust: f32 = living
                        .iter()
                        .filter(|o| o.name != x.name)
                        .map(|o| o.trust_of(&x.name))
                        .sum::<f32>()
                        / pop;
                    f32::mul_add(
                        f32::from(x.skills.counsel),
                        0.35,
                        f32::mul_add(f32::from(x.skills.stewardship), 0.45, x.standing * 1.4),
                    ) + incoming_trust
                };
                score(a)
                    .partial_cmp(&score(b))
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .map(|a| {
                let incoming_trust: f32 = living
                    .iter()
                    .filter(|o| o.name != a.name)
                    .map(|o| o.trust_of(&a.name))
                    .sum::<f32>()
                    / (living.len().saturating_sub(1).max(1) as f32);
                let title = match a.role.id() {
                    "steward" => "High Steward",
                    "elder" => "Parish Elder",
                    "herbalist" => "Herb-Warden",
                    "smith" | "builder" => "Guildmaster",
                    _ => "Village Reeve",
                };
                LeaderView {
                    name: a.name.clone(),
                    role: humanize(a.role.id()),
                    title: title.into(),
                    support: f32::midpoint(a.standing, incoming_trust).clamp(0.0, 4.0),
                }
            });

        // 2. Commodity totals & dynamic market pricing
        let mut sp_food = 0_u32;
        let mut sp_water = 0_u32;
        let mut sp_timber = 0_u32;
        let mut sp_herbs = 0_u32;
        let mut sp_tools = 0_u32;
        for (_, st) in &w.places {
            sp_food += st.food;
            sp_water += st.water;
            sp_timber += st.wood;
            sp_herbs += st.herbs;
            sp_tools += st.tools;
        }
        let mut c_food = 0_u32;
        let mut c_water = 0_u32;
        let mut c_timber = 0_u32;
        let mut c_herbs = 0_u32;
        let mut c_tools = 0_u32;
        for a in &living {
            c_food += a.food;
            c_water += a.water;
            c_timber += a.wood;
            c_herbs += a.herbs;
            c_tools += a.tools;
        }

        let season_id = w.season().id();
        let quote = |name: &'static str, sp: u32, car: u32, base: f32, target_per_cap: f32| {
            let total = sp + car;
            let demand = (pop * target_per_cap).max(1.0);
            let ratio = (demand / (total as f32 + 1.0)).clamp(0.4, 3.5);
            let price = (base * ratio).clamp(0.5, 18.0);
            let status = if (total as f32) < demand * 0.55 {
                "Scarcity"
            } else if (total as f32) > demand * 1.45 {
                "Surplus"
            } else {
                "Balanced"
            };
            CommodityQuote {
                name,
                stockpile: sp,
                carried: car,
                total,
                price_coin: price,
                status,
            }
        };

        let wood_demand = if season_id == "winter" || w.temperature_c <= 5 {
            3.5
        } else {
            2.0
        };
        let commodities = vec![
            quote("Food", sp_food, c_food, 2.0, 3.5),
            quote("Water", sp_water, c_water, 1.5, 3.0),
            quote("Wood", sp_timber, c_timber, 2.2, wood_demand),
            quote("Herbs", sp_herbs, c_herbs, 3.5, 1.4),
            quote("Tools", sp_tools, c_tools, 5.0, 1.2),
        ];

        // 3. Active Civic Decrees derived from live conditions
        let mut decrees = Vec::new();
        let hungry_count = living.iter().filter(|a| a.hunger >= 2).count();
        let sick_count = living.iter().filter(|a| a.ailment.is_some()).count();
        let cold_count = living.iter().filter(|a| a.warmth <= 1).count();
        let damaged_places = w.places.iter().filter(|(_, s)| s.condition <= 3).count();

        if hungry_count > 0 || sp_food + c_food < (living.len() as u32 * 2) {
            decrees.push(DecreeView {
                title: "Statute of the Parish Granary".into(),
                domain: "Rationing".into(),
                summary: format!(
                    "Priority grain distribution to {} hungry villager(s); hoarding discouraged.",
                    hungry_count.max(1)
                ),
                urgent: true,
            });
        }
        if sick_count > 0 {
            decrees.push(DecreeView {
                title: "Apothecary & Well Quarantine".into(),
                domain: "Public Health".into(),
                summary: format!(
                    "Herbalists mobilized to treat {sick_count} afflicted villager(s) with poultices."
                ),
                urgent: true,
            });
        }
        if w.temperature_c <= 5 || cold_count > 0 || season_id == "winter" {
            decrees.push(DecreeView {
                title: "Frost-Watch & Hearth Charter".into(),
                domain: "Shelter".into(),
                summary: "Keep the communal Hut hearth lit and gather timber before nightfall."
                    .into(),
                urgent: cold_count > 0,
            });
        }
        if damaged_places > 0 {
            decrees.push(DecreeView {
                title: "Corvée Repair Levy".into(),
                domain: "Works".into(),
                summary: format!(
                    "{damaged_places} village structure(s) require timber and tool maintenance."
                ),
                urgent: false,
            });
        }
        if decrees.is_empty() {
            decrees.push(DecreeView {
                title: "Charter of the Open Commons".into(),
                domain: "Commerce & Peace".into(),
                summary:
                    "Free barter at the Market, apprentice instruction, and seasonal stewardship."
                        .into(),
                urgent: false,
            });
        }

        // 4. Daily Schedule Phase
        let (daily_phase, daily_detail) = match w.time_of_day().id() {
            "dawn" | "morning" => (
                "Morning Bell · Well & Field Labor".into(),
                "Villagers draw water at the Well and head to the Field and Woods.".into(),
            ),
            "midday" | "afternoon" | "day" => (
                "High Sun · Market & Guild Craft".into(),
                "Peak trading at the Market, tool crafting, and harvest storage at the Granary."
                    .into(),
            ),
            "dusk" | "evening" => (
                "Vesper Bell · Hearthside & Counsel".into(),
                "Supper rations shared, promises weighed, and prayers offered at the Shrine."
                    .into(),
            ),
            _ => (
                "Night Watch · Slumber & Reflection".into(),
                "Villagers rest by the Hut hearth and consolidate memories into reflections."
                    .into(),
            ),
        };

        // 5. Households & Dynasties
        let mut assigned = BTreeSet::new();
        let mut households = Vec::new();
        for a in &w.agents {
            if !a.alive || assigned.contains(&a.name) {
                continue;
            }
            let mut members = vec![a.name.clone()];
            assigned.insert(a.name.clone());
            if let Some(p) = &a.partner {
                if !assigned.contains(p) && w.agents.iter().any(|x| x.name == *p && x.alive) {
                    members.push(p.clone());
                    assigned.insert(p.clone());
                }
            }
            for other in &w.agents {
                if other.alive
                    && !assigned.contains(&other.name)
                    && (other.parents.contains(&a.name)
                        || a.parents.contains(&other.name)
                        || (!a.parents.is_empty() && a.parents == other.parents))
                {
                    members.push(other.name.clone());
                    assigned.insert(other.name.clone());
                }
            }
            let wealth: u32 = members
                .iter()
                .filter_map(|n| w.agent(n))
                .map(|m| m.coin + m.tools * 3 + m.wood + m.herbs * 2)
                .sum();
            let food: u32 = members
                .iter()
                .filter_map(|n| w.agent(n))
                .map(|m| m.food + m.water)
                .sum();
            let title = if members.len() > 1 {
                format!("House of {}", a.name)
            } else {
                format!("{}'s Homestead", a.name)
            };
            let tradition = a
                .memories
                .iter()
                .find(|m| {
                    matches!(
                        m.kind,
                        society_sim::MemoryKind::Tradition | society_sim::MemoryKind::Milestone
                    )
                })
                .map_or_else(
                    || format!("{} craft & parish duty", humanize(a.role.id())),
                    |m| m.text.clone(),
                );
            households.push(HouseholdView {
                title,
                hearth_place: a.place.title().to_owned(),
                members,
                wealth,
                food,
                tradition,
            });
        }

        // 6. Social Factions & Feuds
        let mut stewardship_circle = Vec::new();
        let mut craft_commons = Vec::new();
        let mut shrine_flock = Vec::new();
        for a in &living {
            if a.devotion >= 2.2 || a.place.id() == "shrine" || a.skills.counsel >= 3 {
                shrine_flock.push(a.name.clone());
            } else if a.standing >= 2.3 || a.skills.stewardship >= 3 || a.role.id() == "steward" {
                stewardship_circle.push(a.name.clone());
            } else {
                craft_commons.push(a.name.clone());
            }
        }
        let mut factions = Vec::new();
        if !stewardship_circle.is_empty() {
            factions.push(FactionView {
                name: "The Granary Bench".into(),
                ethos: "Order, granary reserves, and civic stewardship".into(),
                members: stewardship_circle,
                cohesion: 0.84,
            });
        }
        if !craft_commons.is_empty() {
            factions.push(FactionView {
                name: "Fellowship of Field & Timber".into(),
                ethos: "Practical labor, toolmaking, and mutual harvest".into(),
                members: craft_commons,
                cohesion: 0.78,
            });
        }
        if !shrine_flock.is_empty() {
            factions.push(FactionView {
                name: "Keepers of the Shrine Vow".into(),
                ethos: "Sacred oaths, herbal healing, and parish reconciliation".into(),
                members: shrine_flock,
                cohesion: 0.88,
            });
        }

        let mut feuds = Vec::new();
        for a in &living {
            for rel in &a.relations {
                if rel.grudge >= 1.2 && a.name < rel.other {
                    feuds.push(FeudView {
                        a: a.name.clone(),
                        b: rel.other.clone(),
                        grudge: rel.grudge,
                        reason: format!(
                            "Trust {:.1}/4 · Affinity {:.1}/4",
                            a.trust_of(&rel.other),
                            rel.affinity
                        ),
                    });
                }
            }
        }
        feuds.sort_by(|x, y| {
            y.grudge
                .partial_cmp(&x.grudge)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        // 7. Rumor Mill, Promises & Oral Lore
        let mut rumors = Vec::new();
        for a in &living {
            if let Some(p) = &a.promise {
                let status = match p.kept {
                    Some(true) => "Kept Oath",
                    Some(false) => "Broken Oath",
                    None => "Active Vow",
                };
                rumors.push(RumorView {
                    speaker: a.name.clone(),
                    subject: p.to.clone(),
                    text: format!("“{}”", p.text),
                    turn: p.made_turn,
                    kind: status,
                });
            }
            for b in a.beliefs.iter().rev().take(1) {
                rumors.push(RumorView {
                    speaker: b.source.clone().unwrap_or_else(|| a.name.clone()),
                    subject: b.about.clone(),
                    text: format!("{} is {}", b.about, b.value),
                    turn: b.as_of_tick,
                    kind: "Hearsay",
                });
            }
        }
        rumors.sort_by_key(|r| std::cmp::Reverse(r.turn));
        rumors.truncate(10);

        // 8. Memorials of the Fallen
        let memorials: Vec<MemorialView> = w
            .agents
            .iter()
            .filter(|a| !a.alive)
            .map(|a| {
                let (turn, cause) = w
                    .events
                    .iter()
                    .rev()
                    .find(|e| e.kind == EventKind::Death && e.actor == a.name)
                    .map_or_else(
                        || (w.turn, "passed away".into()),
                        |e| (e.turn, death_cause(&e.text)),
                    );
                MemorialView {
                    name: a.name.clone(),
                    role: humanize(a.role.id()),
                    generation: a.generation,
                    turn,
                    cause,
                }
            })
            .collect();

        // 9. Prosperity & Inequality Metrics
        let avg_health = living.iter().map(|a| f32::from(a.health)).sum::<f32>() / pop;
        let total_resources = (sp_food + c_food + sp_water + c_water + sp_timber + c_timber) as f32;
        let prosperity_score = (total_resources / (pop * 8.0))
            .clamp(0.0, 1.0)
            .mul_add(
                30.0,
                (avg_health / 10.0).mul_add(35.0, (w.village_morale / 4.0) * 35.0),
            )
            .clamp(0.0, 100.0);

        let coins: Vec<f32> = living.iter().map(|a| a.coin as f32).collect();
        let mean_coin = coins.iter().sum::<f32>() / pop;
        let gini_coin = if mean_coin <= 0.01 {
            0.0
        } else {
            let diff_sum: f32 = coins
                .iter()
                .flat_map(|x| coins.iter().map(move |y| (x - y).abs()))
                .sum();
            (diff_sum / (2.0 * pop * pop * mean_coin)).clamp(0.0, 1.0)
        };

        let era_name = if w.turn < 8 {
            "The Founding Charter".into()
        } else if hungry_count >= 2 || prosperity_score < 38.0 {
            "The Lean Rationing".into()
        } else if season_id == "winter" || w.temperature_c <= 2 {
            "The Great Frost Watch".into()
        } else if prosperity_score >= 75.0 && w.village_morale >= 2.8 {
            "The Golden Commons".into()
        } else {
            format!("The {} Stewardship", humanize(season_id))
        };

        Self {
            era_name,
            daily_phase,
            daily_detail,
            leader,
            decrees,
            commodities,
            prosperity_score,
            gini_coin,
            households,
            factions,
            feuds,
            rumors,
            memorials,
        }
    }
}

impl Snapshot {
    fn from_world(w: &World) -> Self {
        let report = VillageReport::analyze(w);
        Self {
            turn: w.turn,
            day: w.day,
            season: humanize(w.season().id()),
            time_of_day: humanize(w.time_of_day().id()),
            weather_kind: humanize(w.weather_kind.id()),
            weather: w.weather,
            temperature_c: w.temperature_c,
            village_morale: w.village_morale,
            halted: w.halted.clone(),
            agents: w
                .agents
                .iter()
                .map(|a| AgentView {
                    agent: a.clone(),
                    legal_actions: if a.alive {
                        society_sim::affordances(w, a)
                    } else {
                        Vec::new()
                    },
                    death: w
                        .events
                        .iter()
                        .rev()
                        .find(|e| e.kind == EventKind::Death && e.actor == a.name)
                        .map(|e| DeathView {
                            turn: e.turn,
                            cause: death_cause(&e.text),
                        }),
                })
                .collect(),
            places: w
                .places
                .iter()
                .map(|(place, state)| PlaceView {
                    place: *place,
                    state: state.clone(),
                    present: w
                        .living()
                        .filter(|a| a.place == *place)
                        .map(|a| a.name.clone())
                        .collect(),
                })
                .collect(),
            // A bounded tail keeps the feed responsive in long-lived villages.
            // Persisted events remain visible immediately after a restart.
            events: w
                .events
                .iter()
                .rev()
                .take(400)
                .cloned()
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect(),
            report,
        }
    }
}

#[derive(Clone)]
struct Inspector {
    name: String,
    persona: String,
    judgment_state: serde_json::Value,
}

fn fixed_place_pos(id: &str) -> egui::Pos2 {
    // Ring laid out by hand so the map is stable and legible: seven nodes,
    // adjacent pairs joined. Coordinates are in map-space, not screen-space.
    let (x, y) = match id {
        "granary" => (0.50, 0.12),
        "market" => (0.78, 0.28),
        "well" => (0.86, 0.58),
        "hut" => (0.66, 0.84),
        "shrine" => (0.34, 0.84),
        "woods" => (0.14, 0.58),
        "field" => (0.22, 0.28),
        _ => (0.5, 0.5),
    };
    egui::pos2(x * MAP_W, y * MAP_H)
}

const EDGES: [(&str, &str); 7] = [
    ("granary", "market"),
    ("granary", "field"),
    ("market", "well"),
    ("well", "hut"),
    ("hut", "shrine"),
    ("shrine", "woods"),
    ("woods", "field"),
];

impl App {
    fn new(rt: tokio::runtime::Handle, state: AppState, launch: Launch<'_>) -> Self {
        let Launch {
            port,
            view,
            select,
            expand_state,
            select_none,
            autostart,
            ..
        } = launch;
        // Default selection is the first villager, because the inspector must
        // have something in it when opened straight to that view.
        let first = state
            .world
            .try_lock()
            .ok()
            .and_then(|w| w.agents.first().map(|a| a.name.clone()))
            .unwrap_or_else(|| "Ada".into());
        let mut app = Self {
            rt,
            state,
            port,
            snapshot: None,
            turns: Vec::new(),
            selected: if select_none {
                None
            } else {
                Some(select.map_or(first, str::to_owned))
            },
            selected_place: None,
            expand_state,
            inspector: None,
            // A view flag lets a screenshot capture each panel without a click,
            // so observing the society never takes focus from the observer.
            tab: match view {
                "council" | "economy" | "chronicle" => Tab::Council,
                "feed" => Tab::Feed,
                "inspector" => Tab::Inspector,
                _ => Tab::World,
            },
            roster_filter: RosterFilter::All,
            roster_search: String::new(),
            feed_filter: FeedFilter::All,
            feed_search: String::new(),
            custom_event_text: String::new(),
            theme_applied: false,
            // The settlement keeps living on its own; a window that starts
            // paused would need a click just to grow.
            auto: autostart,
            speed: 1.0,
            last_turn: Instant::now(),
            status: "ready".into(),
            busy: false,
            pending: None,
        };
        app.refresh();
        app
    }

    fn ensure_theme(&mut self, ctx: &egui::Context) {
        if self.theme_applied {
            return;
        }
        self.theme_applied = true;
        let mut visuals = egui::Visuals::dark();
        visuals.panel_fill = Color32::from_rgb(22, 25, 31);
        visuals.window_fill = Color32::from_rgb(18, 20, 26);
        visuals.faint_bg_color = Color32::from_rgb(29, 33, 41);
        visuals.extreme_bg_color = Color32::from_rgb(14, 16, 21);
        visuals.selection.bg_fill = Color32::from_rgb(82, 62, 34);
        visuals.selection.stroke = egui::Stroke::new(1.5, Color32::from_rgb(235, 195, 115));
        visuals.widgets.noninteractive.bg_fill = Color32::from_rgb(28, 32, 40);
        visuals.widgets.noninteractive.bg_stroke =
            egui::Stroke::new(1.0, Color32::from_rgb(52, 58, 70));
        visuals.widgets.inactive.bg_fill = Color32::from_rgb(35, 40, 50);
        visuals.widgets.hovered.bg_fill = Color32::from_rgb(48, 55, 68);
        visuals.widgets.active.bg_fill = Color32::from_rgb(65, 74, 92);
        ctx.set_visuals(visuals);
    }

    fn refresh(&mut self) {
        self.poll();
    }

    fn poll(&mut self) {
        // The window is a consumer: read the world without blocking the engine.
        let state = self.state.clone();
        if let Ok(w) = state.world.try_lock() {
            self.snapshot = Some(Snapshot::from_world(&w));
            self.inspector = self
                .selected
                .as_deref()
                .and_then(|name| w.agent(name))
                .map(|a| Inspector {
                    name: a.name.clone(),
                    persona: a.persona.clone(),
                    judgment_state: society_sim::judgment_state(&w, a),
                });
        }
        if let Ok(run) = self.state.run.try_lock() {
            self.turns.clone_from(&run);
        }
    }

    fn trigger_intervention(&mut self, kind: VillageIntervention) {
        let state = self.state.clone();
        if let Ok(mut w) = state.world.try_lock() {
            let turn = w.turn;
            match kind {
                VillageIntervention::RingBell => {
                    let market = find_place(&w, "market");
                    for a in w.agents.iter_mut().filter(|a| a.alive) {
                        a.place = market;
                    }
                    w.push_event(Event {
                        turn,
                        actor: "Parish Bell".into(),
                        place: market,
                        kind: EventKind::System,
                        action: "assembly_bell".into(),
                        text: "The bronze parish bell tolled across the commons, summoning all living villagers to the Market square.".into(),
                        social: Some("civic".into()),
                        intensity: Some(0.9),
                        grounded: Some(1.0),
                        divine: true,
                    });
                    self.status = "rang parish bell — villagers gathered at Market".into();
                },
                VillageIntervention::HarvestFeast => {
                    let granary = find_place(&w, "granary");
                    for a in w.agents.iter_mut().filter(|a| a.alive) {
                        a.hunger = 0;
                        a.thirst = 0;
                        a.hope = (a.hope + 0.5).min(4.0);
                        a.stress = (a.stress - 0.5).max(0.0);
                    }
                    w.village_morale = (w.village_morale + 0.4).min(4.0);
                    w.push_event(Event {
                        turn,
                        actor: "Village Commons".into(),
                        place: granary,
                        kind: EventKind::Harvest,
                        action: "grand_feast".into(),
                        text: "A communal feast of fresh loaves, cider, and roasted roots restored every villager's spirit.".into(),
                        social: Some("celebration".into()),
                        intensity: Some(0.95),
                        grounded: Some(1.0),
                        divine: true,
                    });
                    self.status = "held grand parish feast".into();
                },
                VillageIntervention::MerchantCaravan => {
                    let market = find_place(&w, "market");
                    for a in w.agents.iter_mut().filter(|a| a.alive) {
                        a.food += 1;
                        a.coin += 2;
                        if a.tools == 0 {
                            a.tools = 1;
                        }
                    }
                    w.push_event(Event {
                        turn,
                        actor: "Peddler Caravan".into(),
                        place: market,
                        kind: EventKind::Trade,
                        action: "caravan_arrival".into(),
                        text: "Wagons from the valley road rolled into the Market, trading iron tools, grain, and silver coin.".into(),
                        social: Some("mercantile".into()),
                        intensity: Some(0.8),
                        grounded: Some(1.0),
                        divine: true,
                    });
                    self.status = "merchant caravan supplied the village".into();
                },
                VillageIntervention::ShrineBlessing => {
                    let shrine = find_place(&w, "shrine");
                    for a in w.agents.iter_mut().filter(|a| a.alive) {
                        a.hope = (a.hope + 0.6).min(4.0);
                        a.devotion = (a.devotion + 0.4).min(4.0);
                        a.stress = (a.stress - 0.4).max(0.0);
                        a.warmth = (a.warmth + 1).min(4);
                    }
                    w.village_morale = (w.village_morale + 0.5).min(4.0);
                    w.push_event(Event {
                        turn,
                        actor: "Shrine Altar".into(),
                        place: shrine,
                        kind: EventKind::Divine,
                        action: "providence_blessing".into(),
                        text: "Warm golden light settled over the Shrine stones, lifting dread and kindling hope across the parish.".into(),
                        social: Some("sacred".into()),
                        intensity: Some(1.0),
                        grounded: Some(1.0),
                        divine: true,
                    });
                    self.status = "blessed the village shrine".into();
                },
                VillageIntervention::HerbalRemedy => {
                    let well = find_place(&w, "well");
                    for a in w.agents.iter_mut().filter(|a| a.alive) {
                        a.ailment = None;
                        a.health = (a.health + 3).min(10);
                        a.herbs += 1;
                    }
                    w.push_event(Event {
                        turn,
                        actor: "Apothecary Guild".into(),
                        place: well,
                        kind: EventKind::Heal,
                        action: "parish_remedy".into(),
                        text: "Steeped willow-bark and clean spring draughts cleared every fever and ailment in the village.".into(),
                        social: Some("healing".into()),
                        intensity: Some(0.9),
                        grounded: Some(1.0),
                        divine: true,
                    });
                    self.status = "cured village ailments".into();
                },
            }
        }
        self.poll();
    }

    fn inject_custom_chronicle(&mut self) {
        let text = self.custom_event_text.trim().to_owned();
        if text.is_empty() {
            return;
        }
        if let Ok(mut w) = self.state.world.try_lock() {
            let turn = w.turn;
            let market = find_place(&w, "market");
            w.push_event(Event {
                turn,
                actor: "Chronicle".into(),
                place: market,
                kind: EventKind::Divine,
                action: "decree".into(),
                text,
                social: Some("providence".into()),
                intensity: Some(0.85),
                grounded: Some(1.0),
                divine: true,
            });
            self.custom_event_text.clear();
            self.status = "inscribed custom event into village chronicle".into();
        }
        self.poll();
    }

    fn request_turn(&mut self, n: usize) {
        if self.busy
            || self
                .snapshot
                .as_ref()
                .and_then(|s| s.halted.clone())
                .is_some()
        {
            return;
        }
        self.busy = true;
        self.status = format!("running {n} turn(s)…");
        let st = self.state.clone();
        let started = Instant::now();
        let (tx, rx) = std::sync::mpsc::channel();
        self.rt.spawn(async move {
            let result = {
                let mut w = st.world.lock().await;
                let mut recs = Vec::new();
                let mut err = None;
                for _ in 0..n {
                    match st.engine.step(&mut w).await {
                        Ok(r) => {
                            apply_village_systems(&mut w);
                            recs.push(r);
                        },
                        Err(e) => {
                            err = Some(e.to_string());
                            break;
                        },
                    }
                }
                if !recs.is_empty() {
                    st.record(recs.iter().cloned()).await;
                }
                (recs, err)
            };
            let _ = tx.send(result);
        });
        // Poll on the next frames rather than blocking here.
        self.pending = Some((rx, started));
    }
}

// The window keeps its own handle to the running turn so it never blocks the
// frame loop.
impl App {
    fn drain_pending(&mut self) {
        let Some((rx, started)) = self.pending.take() else {
            return;
        };
        match rx.try_recv() {
            Ok((recs, err)) => {
                self.busy = false;
                let secs = started.elapsed().as_secs_f32();
                self.status = err.map_or_else(
                    || format!("{} turn(s) in {:.1}s", recs.len(), secs),
                    |e| format!("halted: {e}"),
                );
            },
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                self.pending = Some((rx, started));
            },
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                self.busy = false;
                self.status = "turn worker vanished".into();
            },
        }
    }
}

impl eframe::App for App {
    fn ui(&mut self, root: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = root.ctx().clone();
        self.ensure_theme(&ctx);
        self.drain_pending();
        self.poll();

        if self.auto && !self.busy {
            let interval = Duration::from_secs_f32(3.0 / self.speed.max(0.25));
            if self.last_turn.elapsed() >= interval {
                self.last_turn = Instant::now();
                self.request_turn(1);
            }
        }
        let halted = self.snapshot.as_ref().and_then(|s| s.halted.clone());
        egui::Panel::top("bar").show(root, |ui| {
            ui.horizontal(|ui| {
                ui.heading(RichText::new("☖ society").color(Color32::from_rgb(235, 200, 130)));
                ui.separator();
                if let Some(s) = &self.snapshot {
                    ui.label(RichText::new(format!("Day {} · Turn {}", s.day, s.turn)).strong());
                    ui.separator();
                    ui.label(
                        RichText::new(format!("{} · {}", s.season, s.time_of_day))
                            .color(season_accent(&s.season)),
                    );
                    ui.separator();
                    ui.label(
                        RichText::new(&s.report.era_name)
                            .italics()
                            .color(Color32::from_rgb(210, 185, 140)),
                    );
                    ui.separator();
                    ui.label(format!(
                        "cost ${:.4}",
                        society_server::total_cost(&self.turns).max(0.0)
                    ));
                }
                ui.separator();
                ui.label(format!(":{0}", self.port));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let halted_flag = halted.is_some();
                    if halted_flag && self.auto {
                        self.auto = false;
                    }
                    if ui
                        .add_enabled(
                            !halted_flag && !self.busy,
                            egui::Button::new(if self.auto { "Pause" } else { "Run" }),
                        )
                        .clicked()
                    {
                        self.auto = !self.auto;
                        self.last_turn = Instant::now();
                    }
                    if ui
                        .add_enabled(!halted_flag && !self.busy, egui::Button::new("+5 Turns"))
                        .clicked()
                    {
                        self.request_turn(5);
                    }
                    if ui
                        .add_enabled(!halted_flag && !self.busy, egui::Button::new("Step"))
                        .clicked()
                    {
                        self.request_turn(1);
                    }
                    ui.add_enabled_ui(!halted_flag, |ui| {
                        ui.add(
                            egui::Slider::new(&mut self.speed, 0.25..=5.0)
                                .text("speed")
                                .logarithmic(true),
                        );
                    });
                });
            });
            if let Some(s) = &self.snapshot {
                ui.horizontal_wrapped(|ui| {
                    ui.label(
                        RichText::new(format!("{} · {} °C", s.weather_kind, s.temperature_c))
                            .strong(),
                    );
                    ui.separator();
                    ui.label(format!("Harvest ×{:.2}", s.weather));
                    ui.separator();
                    ui.label(format!("Village morale {:.1}/4", s.village_morale));
                    ui.separator();
                    ui.label(
                        RichText::new(format!("Prosperity {:.0}/100", s.report.prosperity_score))
                            .color(Color32::from_rgb(155, 210, 165)),
                    );
                    if let Some(leader) = &s.report.leader {
                        ui.separator();
                        ui.label(
                            RichText::new(format!("{}: {}", leader.title, leader.name))
                                .color(Color32::from_rgb(230, 195, 125)),
                        );
                    }
                    if let Some(decree) = s.report.decrees.first() {
                        ui.separator();
                        ui.label(RichText::new(format!("Decree: {}", decree.title)).color(
                            if decree.urgent {
                                Color32::from_rgb(235, 155, 110)
                            } else {
                                Color32::from_rgb(175, 195, 225)
                            },
                        ));
                    }
                });
            }
            ui.horizontal(|ui| {
                ui.selectable_value(&mut self.tab, Tab::World, "World");
                ui.selectable_value(&mut self.tab, Tab::Council, "Council & Economy");
                ui.selectable_value(&mut self.tab, Tab::Feed, "Feed");
                ui.selectable_value(&mut self.tab, Tab::Inspector, "Inspector");
                ui.separator();
                ui.menu_button("🔔 Parish Bell & Providence", |ui| {
                    ui.label(RichText::new("Village-wide interventions").weak().small());
                    if ui
                        .button("🔔 Ring Parish Bell (Gather at Market)")
                        .clicked()
                    {
                        self.trigger_intervention(VillageIntervention::RingBell);
                        ui.close();
                    }
                    if ui.button("🍞 Hold Grand Harvest Feast").clicked() {
                        self.trigger_intervention(VillageIntervention::HarvestFeast);
                        ui.close();
                    }
                    if ui
                        .button("🐎 Send Merchant Caravan (+Supplies & Coin)")
                        .clicked()
                    {
                        self.trigger_intervention(VillageIntervention::MerchantCaravan);
                        ui.close();
                    }
                    if ui
                        .button("✨ Bless Shrine Stones (+Hope & Warmth)")
                        .clicked()
                    {
                        self.trigger_intervention(VillageIntervention::ShrineBlessing);
                        ui.close();
                    }
                    if ui
                        .button("🌿 Steep Parish Remedy (Heal Ailments)")
                        .clicked()
                    {
                        self.trigger_intervention(VillageIntervention::HerbalRemedy);
                        ui.close();
                    }
                });
                ui.separator();
                let (status, status_color) = match &halted {
                    Some(reason) => (format!("halted · {reason}"), Color32::from_rgb(200, 60, 60)),
                    None => (self.status.clone(), ui.visuals().weak_text_color()),
                };
                ui.add(egui::Label::new(RichText::new(status).color(status_color)).truncate());
            });
        });

        let alive_count = self
            .snapshot
            .as_ref()
            .map_or(0, |s| s.agents.iter().filter(|a| a.alive).count());
        egui::Panel::right("roster")
            .resizable(true)
            .default_size(285.0)
            .show(root, |ui| {
                ui.heading(format!(
                    "Villagers {}/{}",
                    alive_count,
                    society_sim::POPULATION_CAP
                ));
                ui.horizontal(|ui| {
                    ui.selectable_value(&mut self.roster_filter, RosterFilter::All, "All");
                    ui.selectable_value(&mut self.roster_filter, RosterFilter::Living, "Living");
                    ui.selectable_value(
                        &mut self.roster_filter,
                        RosterFilter::Distressed,
                        "At Risk",
                    );
                });
                ui.add(
                    egui::TextEdit::singleline(&mut self.roster_search)
                        .hint_text("Filter villager, role, or place…"),
                );
                ui.add_space(4.0);
                egui::ScrollArea::vertical()
                    .id_salt("roster_scroll")
                    .show(ui, |ui| {
                        if let Some(s) = &self.snapshot {
                            let needle = self.roster_search.trim().to_lowercase();
                            for a in &s.agents {
                                match self.roster_filter {
                                    RosterFilter::Living if !a.alive => continue,
                                    RosterFilter::Distressed
                                        if !a.alive
                                            || (a.hunger < 2
                                                && a.thirst < 2
                                                && a.ailment.is_none()
                                                && a.health >= 6) =>
                                    {
                                        continue;
                                    },
                                    _ => {},
                                }
                                if !needle.is_empty()
                                    && !a.name.to_lowercase().contains(&needle)
                                    && !a.role.id().to_lowercase().contains(&needle)
                                    && !a.place.title().to_lowercase().contains(&needle)
                                {
                                    continue;
                                }
                                let selected = self.selected.as_deref() == Some(a.name.as_str());
                                ui.group(|ui| {
                                    ui.set_min_width(ui.available_width());
                                    let label = format!(
                                        "{}{} · {}",
                                        a.name,
                                        gen_marker(a.generation),
                                        humanize(a.role.id())
                                    );
                                    let text = if a.alive {
                                        RichText::new(label).strong().color(role_color(a.role.id()))
                                    } else {
                                        RichText::new(format!("{label} · dead"))
                                            .strikethrough()
                                            .weak()
                                    };
                                    if ui.selectable_label(selected, text).clicked() {
                                        self.selected = Some(a.name.clone());
                                        self.tab = Tab::Inspector;
                                    }
                                    ui.label(
                                        RichText::new(format!(
                                            "📍 {} · {} · {} coin",
                                            a.place.title(),
                                            humanize(a.mood.id()),
                                            a.coin
                                        ))
                                        .small(),
                                    );
                                    if a.alive && !a.current_goal.is_empty() {
                                        ui.label(
                                            RichText::new(format!("Goal: {}", a.current_goal))
                                                .italics()
                                                .small()
                                                .weak(),
                                        );
                                    }
                                    ui.label(
                                        RichText::new(format!(
                                            "Food {} · Water {} · Health {}/10",
                                            a.food, a.water, a.health
                                        ))
                                        .small(),
                                    );
                                    ui.label(
                                        RichText::new(format!(
                                            "Hunger {}/3 · Thirst {}/3",
                                            a.hunger, a.thirst
                                        ))
                                        .small()
                                        .color(
                                            if a.alive && (a.hunger >= 2 || a.thirst >= 2) {
                                                Color32::from_rgb(230, 140, 95)
                                            } else {
                                                ui.visuals().weak_text_color()
                                            },
                                        ),
                                    );
                                    if let Some(ailment) = a.ailment {
                                        ui.label(
                                            RichText::new(humanize(ailment.id()))
                                                .color(Color32::from_rgb(230, 140, 95)),
                                        );
                                    }
                                });
                            }
                        }
                    });
            });

        egui::CentralPanel::default().show(root, |ui| match self.tab {
            Tab::World => self.draw_map(ui),
            Tab::Council => self.draw_council(ui),
            Tab::Feed => self.draw_feed(ui),
            Tab::Inspector => self.draw_inspector(ui),
        });

        ctx.request_repaint_after(Duration::from_millis(250));
    }
}

impl App {
    fn draw_map(&mut self, ui: &mut egui::Ui) {
        egui::ScrollArea::vertical()
            .id_salt("world_scroll")
            .show(ui, |ui| {
                if let Some(s) = &self.snapshot {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(
                            RichText::new(&s.report.daily_phase)
                                .strong()
                                .color(Color32::from_rgb(230, 200, 135)),
                        );
                        ui.label(RichText::new("—").weak());
                        ui.label(RichText::new(&s.report.daily_detail).weak().small());
                    });
                }
                let height = (ui.available_height() * 0.58).clamp(290.0, 430.0);
                ui.allocate_ui(egui::vec2(ui.available_width(), height), |ui| {
                    self.draw_ring(ui);
                });
                self.draw_stockpiles(ui);
                self.draw_world_cards(ui);
            });
    }

    fn draw_stockpiles(&self, ui: &mut egui::Ui) {
        let Some(s) = &self.snapshot else {
            return;
        };
        ui.heading("Village stockpiles");
        ui.label(
            RichText::new(
                "Stores at each place; carried supplies are listed in each villager’s inspector.",
            )
            .weak()
            .small(),
        );
        egui::ScrollArea::horizontal()
            .id_salt("stockpiles_scroll")
            .show(ui, |ui| {
                egui::Grid::new("stockpiles")
                    .striped(true)
                    .min_col_width(43.0)
                    .show(ui, |ui| {
                        for title in [
                            "Place",
                            "Food",
                            "Water",
                            "Wood",
                            "Herbs",
                            "Tools",
                            "Coin",
                            "Condition",
                            "Hearth",
                            "Fertility",
                            "Owner",
                        ] {
                            ui.strong(title);
                        }
                        ui.end_row();
                        for p in &s.places {
                            ui.label(humanize(p.place.id()));
                            for n in [
                                p.state.food,
                                p.state.water,
                                p.state.wood,
                                p.state.herbs,
                                p.state.tools,
                                p.state.coin,
                            ] {
                                ui.label(n.to_string());
                            }
                            ui.label(format!("{}/5", p.state.condition));
                            ui.label(if p.state.hearth_lit { "Lit" } else { "Unlit" });
                            ui.label(format!("×{:.2}", p.state.fertility));
                            ui.label(p.state.owner.as_deref().unwrap_or("Communal"));
                            ui.end_row();
                        }
                    });
            });
    }

    fn draw_ring(&mut self, ui: &mut egui::Ui) {
        // Fill the panel: the ring looks wrong marooned in a corner of a large
        // window, and a hand-placed layout should still breathe with the frame.
        let avail = ui.available_size();
        let h = (avail.y - 55.0).max(240.0);
        let w = avail.x.max(320.0);
        let (rect, _) = ui.allocate_exact_size(egui::vec2(w, h), egui::Sense::hover());
        let painter = ui.painter_at(rect);
        let sx = rect.width() / MAP_W;
        // Reserve room around the ring for villagers and their labels.
        let to_screen = |p: egui::Pos2| {
            egui::pos2(
                (p.x / MAP_W).mul_add(rect.width() - 140.0, rect.left() + 70.0),
                (p.y / MAP_H).mul_add(rect.height() - 130.0, rect.top() + 65.0),
            )
        };

        // Atmospheric canvas backdrop tinted by season and time of day.
        let (sky_bg, border_tint) = self.snapshot.as_ref().map_or_else(
            || (Color32::from_rgb(20, 24, 30), Color32::from_gray(70)),
            |s| canvas_atmosphere(&s.season, &s.time_of_day),
        );
        painter.rect_filled(rect, 10.0, sky_bg);
        painter.rect_stroke(
            rect,
            10.0,
            egui::Stroke::new(1.0, border_tint),
            egui::StrokeKind::Inside,
        );

        // Subtle central Village Commons plaza circle & compass paths.
        let commons_center = to_screen(egui::pos2(0.50 * MAP_W, 0.50 * MAP_H));
        painter.circle_filled(
            commons_center,
            38.0 * sx.min(1.3),
            Color32::from_rgba_unmultiplied(180, 155, 110, 16),
        );
        painter.circle_stroke(
            commons_center,
            38.0 * sx.min(1.3),
            egui::Stroke::new(1.0, Color32::from_rgba_unmultiplied(190, 165, 120, 40)),
        );
        painter.text(
            commons_center,
            egui::Align2::CENTER_CENTER,
            "Parish\nCommons",
            egui::FontId::proportional(10.5),
            Color32::from_rgba_unmultiplied(200, 185, 150, 95),
        );

        // Spokes from the commons & perimeter cobblestone ring.
        for id in [
            "granary", "market", "well", "hut", "shrine", "woods", "field",
        ] {
            painter.line_segment(
                [commons_center, to_screen(fixed_place_pos(id))],
                egui::Stroke::new(1.0, Color32::from_rgba_unmultiplied(120, 115, 100, 45)),
            );
        }
        for (a, b) in EDGES {
            let pa = to_screen(fixed_place_pos(a));
            let pb = to_screen(fixed_place_pos(b));
            painter.line_segment(
                [pa, pb],
                egui::Stroke::new(4.0, Color32::from_rgba_unmultiplied(45, 42, 36, 180)),
            );
            painter.line_segment(
                [pa, pb],
                egui::Stroke::new(1.6, Color32::from_rgb(135, 125, 105)),
            );
        }

        if let Some(s) = &self.snapshot {
            // Weather particle overlay (rain streaks, snow crystals, or sun motes).
            draw_weather_overlay(&painter, rect, &s.weather_kind, s.turn);

            for p in &s.places {
                let center = to_screen(fixed_place_pos(p.place.id()));
                let node_rect = egui::Rect::from_center_size(center, egui::vec2(142.0, 54.0));
                if p.state.hearth_lit {
                    painter.rect_filled(
                        node_rect.expand(4.0),
                        11.0,
                        Color32::from_rgba_unmultiplied(240, 155, 65, 38),
                    );
                }
                let is_sel_place = self.selected_place.as_deref() == Some(p.place.id());
                painter.rect_filled(node_rect, 8.0, place_card_fill(p.place.id(), is_sel_place));
                painter.rect_stroke(
                    node_rect,
                    8.0,
                    egui::Stroke::new(
                        if is_sel_place { 2.0 } else { 1.2 },
                        if p.state.hearth_lit {
                            Color32::from_rgb(235, 165, 85)
                        } else if p.state.condition <= 2 {
                            Color32::from_rgb(210, 95, 80)
                        } else {
                            Color32::from_rgb(115, 125, 140)
                        },
                    ),
                    egui::StrokeKind::Inside,
                );
                painter.text(
                    center,
                    egui::Align2::CENTER_CENTER,
                    format!(
                        "{} {}{}\n{} food · {} water · {}/5",
                        place_glyph(p.place.id()),
                        humanize(p.place.id()),
                        if p.state.hearth_lit { " 🔥" } else { "" },
                        p.state.food,
                        p.state.water,
                        p.state.condition
                    ),
                    egui::FontId::proportional(11.5),
                    Color32::from_gray(232),
                );
                if ui
                    .interact(
                        node_rect,
                        ui.scope_id().with(("place", p.place.id())),
                        egui::Sense::click_and_drag(),
                    )
                    .on_hover_text(format!(
                    "{}\n{}\nCondition {}/5 · Fertility ×{:.2}\nHearth {} · Owner {}\nPresent: {}",
                    p.place.title(),
                    inventory_text(
                        p.state.food,
                        p.state.water,
                        p.state.wood,
                        p.state.herbs,
                        p.state.tools,
                        p.state.coin
                    ),
                    p.state.condition,
                    p.state.fertility,
                    if p.state.hearth_lit { "lit" } else { "unlit" },
                    p.state.owner.as_deref().unwrap_or("communal"),
                    p.present.join(", ")
                ))
                    .clicked()
                {
                    self.selected_place = Some(p.place.id().to_owned());
                }
            }
            // Draw every villager after the buildings so a neighbouring place
            // cannot paint over their marker or name.
            for p in &s.places {
                let center = to_screen(fixed_place_pos(p.place.id()));
                let n = p.present.len().max(1) as f32;
                let orbit = 72.0;
                let dot = 7.0 * sx.min(1.5);
                let margin = dot + 22.0;
                for (i, name) in p.present.iter().enumerate() {
                    // Start at the top and go clockwise.
                    let angle =
                        (i as f32 / n).mul_add(std::f32::consts::TAU, -std::f32::consts::FRAC_PI_2);
                    let mut pos = center + egui::vec2(angle.cos(), angle.sin()) * orbit;
                    // Keep the marker inside the map: a villager at the top of
                    // the ring must not be clipped by the panel edge.
                    pos.x = pos.x.clamp(rect.left() + margin, rect.right() - margin);
                    pos.y = pos.y.clamp(rect.top() + margin, rect.bottom() - margin);
                    let agent_view = s.agents.iter().find(|a| a.name == *name);
                    let hover_summary = agent_view.map_or_else(
                        || format!("Inspect {name}"),
                        |av| {
                            format!(
                                "Inspect {} ({})\nMood: {} · Health {}/10\nGoal: {}",
                                av.name,
                                humanize(av.role.id()),
                                humanize(av.mood.id()),
                                av.health,
                                if av.current_goal.is_empty() {
                                    "—"
                                } else {
                                    &av.current_goal
                                }
                            )
                        },
                    );
                    if ui
                        .interact(
                            egui::Rect::from_center_size(pos, egui::vec2(24.0, 24.0)),
                            ui.scope_id().with(("villager", name)),
                            egui::Sense::click(),
                        )
                        .on_hover_text(hover_summary)
                        .clicked()
                    {
                        self.selected = Some(name.clone());
                        self.tab = Tab::Inspector;
                    }
                    let selected = self.selected.as_deref() == Some(name.as_str());
                    if selected {
                        painter.circle_filled(pos, dot + 4.0, ui.visuals().selection.bg_fill);
                        painter.circle_stroke(
                            pos,
                            dot + 4.0,
                            egui::Stroke::new(2.0, ui.visuals().selection.stroke.color),
                        );
                    }
                    let fill_col = if selected {
                        Color32::from_rgb(245, 235, 200)
                    } else if let Some(av) = agent_view {
                        role_color(av.role.id())
                    } else {
                        Color32::from_rgb(90, 160, 220)
                    };
                    painter.circle_filled(pos, dot, fill_col);
                    if let Some(av) = agent_view {
                        if av.ailment.is_some() || av.hunger >= 2 || av.thirst >= 2 {
                            painter.circle_stroke(
                                pos,
                                dot + 2.5,
                                egui::Stroke::new(1.5, Color32::from_rgb(235, 110, 85)),
                            );
                        }
                    }
                    // Label outward from the place marker, then clamped, so two
                    // neighbours cannot collide and nothing leaves the panel.
                    let dir = (pos - center).normalized();
                    let lp = pos + dir * (dot + 9.0);
                    let lp = egui::pos2(
                        lp.x.clamp(rect.left() + 18.0, rect.right() - 18.0),
                        lp.y.clamp(rect.top() + 10.0, rect.bottom() - 10.0),
                    );
                    // A backing pill keeps the name legible where it crosses
                    // a path line; without it the stroke runs through the text.
                    let galley = painter.layout_no_wrap(
                        name.clone(),
                        if selected {
                            egui::FontId::proportional(12.0)
                        } else {
                            egui::FontId::proportional(11.5)
                        },
                        if selected {
                            ui.visuals().selection.stroke.color
                        } else {
                            Color32::from_gray(215)
                        },
                    );
                    let pad = egui::vec2(4.0, 1.0);
                    let bg = egui::Rect::from_center_size(lp, galley.size() + pad * 2.0);
                    let bg_fill = if selected {
                        ui.visuals().selection.bg_fill
                    } else {
                        Color32::from_rgb(18, 18, 22)
                    };
                    painter.rect_filled(bg, 4.0, bg_fill);
                    painter.galley(
                        lp - galley.size() * 0.5,
                        galley,
                        if selected {
                            ui.visuals().selection.stroke.color
                        } else {
                            Color32::from_gray(215)
                        },
                    );
                }
            }
        }

        ui.add_space(10.0);
        ui.label(
            RichText::new("Hover a place for its stockpiles. Click a villager to inspect. All resources and buildings are listed below.")
                .weak()
                .small(),
        );
    }

    fn draw_world_cards(&mut self, ui: &mut egui::Ui) {
        let Some(s) = &self.snapshot else {
            return;
        };
        ui.add_space(10.0);
        ui.separator();
        ui.columns(2, |cols| {
            cols[0].heading("Active Parish Decrees");
            for d in &s.report.decrees {
                cols[0].group(|ui| {
                    ui.horizontal(|ui| {
                        ui.strong(&d.title);
                        ui.label(RichText::new(format!("[{}]", d.domain)).small().color(
                            if d.urgent {
                                Color32::from_rgb(235, 145, 105)
                            } else {
                                Color32::from_rgb(150, 195, 230)
                            },
                        ));
                    });
                    ui.label(RichText::new(&d.summary).small());
                });
            }

            cols[1].heading("Live Voices Around the Commons");
            let recent_speeches: Vec<&Event> = s
                .events
                .iter()
                .rev()
                .filter(|e| !e.text.is_empty())
                .take(4)
                .collect();
            if recent_speeches.is_empty() {
                cols[1].label(RichText::new("Quiet across the village green.").weak());
            }
            for ev in recent_speeches {
                cols[1].group(|ui| {
                    let (_, col) = event_style(ev.kind);
                    ui.horizontal(|ui| {
                        if ui
                            .selectable_label(false, RichText::new(&ev.actor).strong())
                            .clicked()
                            && s.agents.iter().any(|a| a.name == ev.actor)
                        {
                            self.selected = Some(ev.actor.clone());
                            self.tab = Tab::Inspector;
                        }
                        ui.label(
                            RichText::new(format!("@ {} · turn {}", ev.place.title(), ev.turn))
                                .small()
                                .color(col),
                        );
                    });
                    ui.label(RichText::new(&ev.text).small());
                });
            }
        });
    }

    fn draw_council(&mut self, ui: &mut egui::Ui) {
        let mut inscribe = false;
        let Some(s) = &self.snapshot else {
            return;
        };
        egui::ScrollArea::vertical()
            .id_salt("council_scroll")
            .show(ui, |ui| {
                ui.heading(format!("Parish Council & Economy · {}", s.report.era_name));
                ui.label(
                    RichText::new(format!(
                        "{} — {}",
                        s.report.daily_phase, s.report.daily_detail
                    ))
                    .weak(),
                );
                ui.add_space(6.0);

                ui.columns(2, |cols| {
                    cols[0].group(|ui| {
                        ui.heading("Civic Leadership & Decrees");
                        if let Some(l) = &s.report.leader {
                            ui.horizontal(|ui| {
                                ui.strong(format!("{}:", l.title));
                                if ui.selectable_label(false, &l.name).clicked() {
                                    self.selected = Some(l.name.clone());
                                    self.tab = Tab::Inspector;
                                }
                                ui.label(
                                    RichText::new(format!(
                                        "({}) · Mandate {:.1}/4.0",
                                        l.role, l.support
                                    ))
                                    .weak(),
                                );
                            });
                        }
                        ui.add_space(4.0);
                        for d in &s.report.decrees {
                            ui.label(
                                RichText::new(format!("• {} ({})", d.title, d.domain)).strong(),
                            );
                            ui.label(RichText::new(&d.summary).small().weak());
                        }
                    });

                    cols[1].group(|ui| {
                        ui.heading("Market Exchange & Scarcity Index");
                        ui.label(
                            RichText::new(format!(
                                "Prosperity {:.1}/100 · Coin Gini {:.2}",
                                s.report.prosperity_score, s.report.gini_coin
                            ))
                            .small()
                            .weak(),
                        );
                        egui::Grid::new("market_quotes_grid")
                            .striped(true)
                            .show(ui, |ui| {
                                for h in ["Good", "Stores", "Carried", "Total", "Price", "Market"] {
                                    ui.strong(h);
                                }
                                ui.end_row();
                                for q in &s.report.commodities {
                                    ui.label(q.name);
                                    ui.label(q.stockpile.to_string());
                                    ui.label(q.carried.to_string());
                                    ui.label(q.total.to_string());
                                    ui.label(format!("{:.1} c", q.price_coin));
                                    let col = match q.status {
                                        "Scarcity" => Color32::from_rgb(230, 125, 100),
                                        "Surplus" => Color32::from_rgb(145, 210, 160),
                                        _ => Color32::from_gray(200),
                                    };
                                    ui.colored_label(col, q.status);
                                    ui.end_row();
                                }
                            });
                    });
                });

                ui.add_space(8.0);
                ui.columns(2, |cols| {
                    cols[0].group(|ui| {
                        ui.heading("Households & Hearths");
                        for hh in &s.report.households {
                            ui.group(|ui| {
                                ui.horizontal_wrapped(|ui| {
                                    ui.strong(&hh.title);
                                    ui.label(
                                        RichText::new(format!(
                                            "· {} · Worth {} · Rations {}",
                                            hh.hearth_place, hh.wealth, hh.food
                                        ))
                                        .small()
                                        .weak(),
                                    );
                                });
                                ui.label(
                                    RichText::new(format!("Members: {}", hh.members.join(", ")))
                                        .small(),
                                );
                                ui.label(RichText::new(&hh.tradition).small().italics().weak());
                            });
                        }
                    });

                    cols[1].group(|ui| {
                        ui.heading("Village Factions & Feuds");
                        for f in &s.report.factions {
                            ui.group(|ui| {
                                ui.horizontal(|ui| {
                                    ui.strong(&f.name);
                                    ui.label(
                                        RichText::new(format!(
                                            "cohesion {:.0}%",
                                            f.cohesion * 100.0
                                        ))
                                        .small()
                                        .weak(),
                                    );
                                });
                                ui.label(RichText::new(&f.ethos).small().weak());
                                ui.label(
                                    RichText::new(f.members.join(", "))
                                        .small()
                                        .color(Color32::from_rgb(215, 195, 145)),
                                );
                            });
                        }
                        if !s.report.feuds.is_empty() {
                            ui.add_space(4.0);
                            ui.strong("Active Grudges & Tensions");
                            for feud in &s.report.feuds {
                                ui.colored_label(
                                    Color32::from_rgb(230, 140, 120),
                                    format!(
                                        "⚔ {} ↔ {} (Grudge {:.1}/4 · {})",
                                        feud.a, feud.b, feud.grudge, feud.reason
                                    ),
                                );
                            }
                        }
                    });
                });

                ui.add_space(8.0);
                ui.columns(2, |cols| {
                    cols[0].group(|ui| {
                        ui.heading("Rumor Mill & Parish Oaths");
                        if s.report.rumors.is_empty() {
                            ui.label(RichText::new("No active rumors or oaths recorded.").weak());
                        }
                        for r in &s.report.rumors {
                            ui.label(format!(
                                "[{}] {} → {}: {} (turn {})",
                                r.kind, r.speaker, r.subject, r.text, r.turn
                            ));
                        }
                    });
                    cols[1].group(|ui| {
                        ui.heading("Memorial Roll & Chronicle Inscription");
                        if s.report.memorials.is_empty() {
                            ui.label(
                                RichText::new("No villagers have perished since the founding.")
                                    .weak(),
                            );
                        } else {
                            for m in &s.report.memorials {
                                ui.label(format!(
                                    "✝ {}{} ({}) — {} at turn {}",
                                    m.name,
                                    gen_marker(m.generation),
                                    m.role,
                                    m.cause,
                                    m.turn
                                ));
                            }
                        }
                        ui.separator();
                        ui.label(RichText::new("Inscribe a parish chronicle event:").small());
                        ui.horizontal(|ui| {
                            ui.add(
                                egui::TextEdit::singleline(&mut self.custom_event_text)
                                    .hint_text("e.g. A shooting star crossed the Shrine…"),
                            );
                            if ui.button("Inscribe").clicked() {
                                inscribe = true;
                            }
                        });
                    });
                });
            });
        if inscribe {
            self.inject_custom_chronicle();
        }
    }

    fn draw_feed(&mut self, ui: &mut egui::Ui) {
        let Some(s) = &self.snapshot else {
            return;
        };
        if self.turns.is_empty() && s.events.is_empty() {
            ui.label("No village events yet. Run or step to begin.");
            return;
        }
        ui.horizontal_wrapped(|ui| {
            ui.label(
                RichText::new("Recent village history · saved events survive a restart")
                    .weak()
                    .small(),
            );
            ui.separator();
            ui.selectable_value(&mut self.feed_filter, FeedFilter::All, "All");
            ui.selectable_value(&mut self.feed_filter, FeedFilter::Social, "Social & Bonds");
            ui.selectable_value(&mut self.feed_filter, FeedFilter::Economy, "Work & Trade");
            ui.selectable_value(
                &mut self.feed_filter,
                FeedFilter::Crisis,
                "Conflict & Death",
            );
            ui.selectable_value(&mut self.feed_filter, FeedFilter::Sacred, "Sacred & Civic");
            ui.add(
                egui::TextEdit::singleline(&mut self.feed_search)
                    .desired_width(160.0)
                    .hint_text("Search feed…"),
            );
        });
        // Merge the saved event history with this session's inference details.
        // A restart no longer hides crafting, healing, trade, prayer, or bonds.
        let turns: BTreeSet<u64> = s
            .events
            .iter()
            .map(|e| e.turn)
            .chain(self.turns.iter().map(|r| r.turn))
            .collect();
        let needle = self.feed_search.trim().to_lowercase();
        egui::ScrollArea::vertical()
            .id_salt("feed_scroll")
            .stick_to_bottom(true)
            .show(ui, |ui| {
                for turn in turns {
                    day_divider(ui, &format!("turn {turn}"));
                    let rec = self.turns.iter().find(|r| r.turn == turn);
                    if let Some(rec) = rec {
                        ui.label(
                            RichText::new(format!(
                                "day {} · ${:.5}",
                                rec.day,
                                society_server::turn_cost(rec)
                            ))
                            .weak()
                            .small(),
                        );
                    }
                    for e in s.events.iter().filter(|e| e.turn == turn) {
                        if !matches_feed_filter(e.kind, self.feed_filter) {
                            continue;
                        }
                        if !needle.is_empty()
                            && !e.actor.to_lowercase().contains(&needle)
                            && !e.text.to_lowercase().contains(&needle)
                            && !e.action.to_lowercase().contains(&needle)
                        {
                            continue;
                        }
                        let (label, color) = event_style(e.kind);
                        ui.horizontal_wrapped(|ui| {
                            if ui
                                .selectable_label(false, RichText::new(&e.actor).strong())
                                .clicked()
                                && s.agents.iter().any(|a| a.name == e.actor)
                            {
                                self.selected = Some(e.actor.clone());
                                self.tab = Tab::Inspector;
                            }
                            ui.label(
                                RichText::new(format!(
                                    "{label} · {} · {}",
                                    humanize(&e.action),
                                    e.place.title()
                                ))
                                .color(color),
                            );
                        });
                        if !e.text.is_empty() {
                            ui.label(&e.text);
                        }
                        if let Some(a) = rec.and_then(|r| {
                            r.agents
                                .iter()
                                .find(|a| (&a.name, &a.action) == (&e.actor, &e.action))
                        }) {
                            ui.label(RichText::new(turn_meta(a)).weak().small());
                            if let Some(note) = &a.note {
                                ui.label(RichText::new(note).small().color(color));
                            }
                        } else if let Some(social) = &e.social {
                            ui.label(
                                RichText::new(format!(
                                    "Social {} · intensity {}",
                                    humanize(social),
                                    fmt(e.intensity)
                                ))
                                .weak()
                                .small(),
                            );
                        }
                        ui.add_space(6.0);
                    }
                    // Rejected actions do not always emit events. Keep those
                    // visible with the actual reason supplied by the simulator.
                    if let Some(rec) = rec {
                        for a in &rec.agents {
                            if s.events.iter().any(|e| {
                                e.turn == turn && e.actor == a.name && e.action == a.action
                            }) {
                                continue;
                            }
                            ui.label(format!(
                                "{} · {} · {}",
                                a.name,
                                humanize(&a.action),
                                if a.accepted { "Accepted" } else { "Rejected" }
                            ));
                            // This row has no gate-score metadata line, but the
                            // judgment layer's labels still belong to the turn.
                            if let Some(labels) = turn_labels(a) {
                                ui.label(RichText::new(labels).weak().small());
                            }
                            match turn_words(a) {
                                TurnWords::Spoken { say, note } => {
                                    ui.label(say);
                                    if let Some(note) = note {
                                        ui.label(RichText::new(note).weak().small());
                                    }
                                },
                                TurnWords::Silent { note } => {
                                    ui.label(RichText::new(note).weak().small());
                                },
                                TurnWords::Nothing => {},
                            }
                        }
                    }
                }
            });
    }

    fn draw_inspector(&self, ui: &mut egui::Ui) {
        let Some(insp) = &self.inspector else {
            ui.label("Select a villager from the roster or the map.");
            return;
        };
        let Some(s) = &self.snapshot else {
            return;
        };
        let Some(a) = s.agents.iter().find(|a| a.name == insp.name) else {
            return;
        };
        ui.heading(format!("{} · {}", a.name, humanize(a.role.id())));
        ui.label(RichText::new(&insp.persona).weak());
        ui.separator();
        egui::ScrollArea::vertical()
            .id_salt(("inspector_scroll", &a.name))
            .show(ui, |ui| {
                if !a.alive {
                    let death = a.death.as_ref().map_or_else(
                        || "dead".into(),
                        |d| format!("{} at turn {}", d.cause, d.turn),
                    );
                    ui.colored_label(
                        Color32::from_rgb(230, 130, 120),
                        format!("Deceased · {death}"),
                    );
                }
                ui.horizontal_wrapped(|ui| {
                    ui.strong(a.place.title());
                    ui.label(format!("Mood: {}", humanize(a.mood.id())));
                    ui.label(format!("Standing: {:.1}/4", a.standing));
                });
                let lineage = if a.parents.is_empty() {
                    format!(
                        "Generation {} · founder · age {} turns",
                        a.generation,
                        s.turn.saturating_sub(a.born_turn)
                    )
                } else {
                    format!(
                        "Generation {} · born turn {} · child of {}",
                        a.generation,
                        a.born_turn,
                        a.parents.join(" and ")
                    )
                };
                ui.label(RichText::new(lineage).weak());
                if let Some(partner) = &a.partner {
                    ui.label(format!("Partner: {partner}"));
                }
                ui.add_space(8.0);
                ui.strong("Current goal");
                ui.label(if a.current_goal.is_empty() {
                    "No goal recorded yet."
                } else {
                    &a.current_goal
                });
                ui.add_space(8.0);
                ui.strong("Vitals & outlook");
                if let Some(ailment) = a.ailment {
                    ui.colored_label(
                        Color32::from_rgb(230, 140, 95),
                        format!("Ailment: {}", humanize(ailment.id())),
                    );
                } else {
                    ui.label("Ailment: none");
                }
                let width = ((ui.available_width() - 24.0) / 3.0).max(100.0);
                egui::Grid::new("vitals_grid")
                    .num_columns(3)
                    .spacing([8.0, 6.0])
                    .show(ui, |ui| {
                        for (i, (name, value, max, danger)) in [
                            ("Health", f32::from(a.health), 10.0, a.health < 5),
                            ("Energy", f32::from(a.energy), 10.0, a.energy <= 2),
                            ("Warmth", f32::from(a.warmth), 4.0, a.warmth <= 1),
                            ("Hunger", f32::from(a.hunger), 3.0, a.hunger >= 2),
                            ("Thirst", f32::from(a.thirst), 3.0, a.thirst >= 2),
                            ("Stress", a.stress, 4.0, a.stress >= 2.8),
                            ("Hope", a.hope, 4.0, false),
                            ("Devotion", a.devotion, 4.0, false),
                            ("Standing", a.standing, 4.0, false),
                        ]
                        .into_iter()
                        .enumerate()
                        {
                            let mut bar = egui::ProgressBar::new((value / max).clamp(0.0, 1.0))
                                .desired_width(width)
                                .text(format!("{name} {value:.1}/{max:.0}"));
                            if danger {
                                bar = bar.fill(Color32::from_rgb(185, 90, 65));
                            }
                            ui.add(bar);
                            if i % 3 == 2 {
                                ui.end_row();
                            }
                        }
                    });
                if a.starving_turns > 0 {
                    ui.label(format!("Consecutive starving turns: {}", a.starving_turns));
                }
                ui.add_space(8.0);
                ui.strong("Carried supplies");
                ui.horizontal_wrapped(|ui| {
                    for (label, n) in [
                        ("Food", a.food),
                        ("Water", a.water),
                        ("Wood", a.wood),
                        ("Herbs", a.herbs),
                        ("Tools", a.tools),
                        ("Coin", a.coin),
                    ] {
                        ui.label(format!("{label}: {n}"));
                    }
                });
                ui.add_space(8.0);
                ui.strong("Skills");
                ui.horizontal_wrapped(|ui| {
                    for (name, level) in [
                        ("Farming", a.skills.farming),
                        ("Forestry", a.skills.forestry),
                        ("Herbalism", a.skills.herbalism),
                        ("Craft", a.skills.craft),
                        ("Stewardship", a.skills.stewardship),
                        ("Counsel", a.skills.counsel),
                    ] {
                        ui.label(format!("{name} {level}/5"));
                    }
                });
                ui.add_space(8.0);
                ui.strong("Available actions");
                ui.horizontal_wrapped(|ui| {
                    if a.legal_actions.is_empty() {
                        ui.label("None");
                    }
                    for action in &a.legal_actions {
                        ui.label(RichText::new(humanize(action)).monospace().small());
                    }
                });
                ui.label(
                    RichText::new("Villagers choose their own actions when the simulation runs.")
                        .weak()
                        .small(),
                );
                if let Some(promise) = &a.promise {
                    ui.add_space(8.0);
                    ui.strong("Promise");
                    ui.label(format!(
                        "To {}: “{}” · turn {} · {}",
                        promise.to,
                        promise.text,
                        promise.made_turn,
                        match promise.kept {
                            Some(true) => "kept",
                            Some(false) => "broken",
                            None => "unresolved",
                        }
                    ));
                }
                ui.add_space(8.0);
                ui.strong("Relationships & trust");
                egui::ScrollArea::horizontal()
                    .id_salt("relations_scroll")
                    .show(ui, |ui| {
                        egui::Grid::new("relations_grid")
                            .striped(true)
                            .show(ui, |ui| {
                                for title in [
                                    "Villager",
                                    "Kinship",
                                    "Trust /4",
                                    "Affinity /4",
                                    "Respect /4",
                                    "Grudge /4",
                                ] {
                                    ui.strong(title);
                                }
                                ui.end_row();
                                let names: BTreeSet<&str> = a
                                    .trust
                                    .iter()
                                    .map(|(n, _)| n.as_str())
                                    .chain(a.relations.iter().map(|r| r.other.as_str()))
                                    .collect();
                                for name in names {
                                    let dead = s
                                        .agents
                                        .iter()
                                        .any(|other| other.name == name && !other.alive);
                                    ui.label(if dead {
                                        RichText::new(format!("{name} (dead)")).weak()
                                    } else {
                                        RichText::new(name)
                                    });
                                    let relation = a.relation_with(name);
                                    ui.label(relation.map_or_else(
                                        || "Unrecorded".into(),
                                        |r| humanize(r.kinship.id()),
                                    ));
                                    ui.label(format!("{:.1}", a.trust_of(name)));
                                    for value in [
                                        relation.map(|r| r.affinity),
                                        relation.map(|r| r.respect),
                                        relation.map(|r| r.grudge),
                                    ] {
                                        ui.label(
                                            value.map_or_else(|| "—".into(), |v| format!("{v:.1}")),
                                        );
                                    }
                                    ui.end_row();
                                }
                            });
                    });
                ui.add_space(8.0);
                ui.strong("Memories");
                ui.label(
                    RichText::new(format!(
                        "Reflection budget used: {:.1}/{}",
                        a.reflection_poignancy,
                        society_sim::REFLECTION_BUDGET
                    ))
                    .weak()
                    .small(),
                );
                if a.memories.is_empty() {
                    ui.label("No memories recorded.");
                }
                for m in a.memories.iter().rev() {
                    ui.group(|ui| {
                        ui.label(if m.permanent {
                            RichText::new(&m.text).color(Color32::from_rgb(220, 150, 130))
                        } else {
                            RichText::new(&m.text)
                        });
                        ui.label(
                            RichText::new(format!(
                                "{} · turn {} · salience {:.2} · valence {:+.2}{}",
                                memory_kind(m.kind),
                                m.turn,
                                m.salience,
                                m.valence,
                                if m.permanent { " · permanent" } else { "" }
                            ))
                            .weak()
                            .small(),
                        );
                        ui.label(
                            RichText::new(format!(
                                "Retrieved {} times · last accessed turn {}{}",
                                m.accesses,
                                m.last_access,
                                if m.tags.is_empty() {
                                    String::new()
                                } else {
                                    format!(" · {}", m.tags.join(", "))
                                }
                            ))
                            .weak()
                            .small(),
                        );
                    });
                }
                egui::CollapsingHeader::new(format!("Beliefs ({})", a.beliefs.len())).show(
                    ui,
                    |ui| {
                        if a.beliefs.is_empty() {
                            ui.label("No observations recorded.");
                        }
                        for belief in &a.beliefs {
                            ui.label(format!(
                                "{} = {} · turn {} · {}",
                                belief.about,
                                belief.value,
                                belief.as_of_tick,
                                belief.source.as_deref().map_or_else(
                                    || "observed directly".into(),
                                    |n| format!("told by {n}")
                                )
                            ));
                        }
                    },
                );
                egui::CollapsingHeader::new(format!("Known facts ({})", a.facts.len())).show(
                    ui,
                    |ui| {
                        if a.facts.is_empty() {
                            ui.label("No facts recorded.");
                        }
                        for fact in &a.facts {
                            ui.label(format!(
                                "#{} · {}: {} · confidence {:.2}",
                                fact.id, fact.subject, fact.text, fact.confidence
                            ));
                            ui.label(
                                RichText::new(format!(
                                    "Since turn {} · {} · invalidated {} · expired {}",
                                    fact.valid_at,
                                    if fact.live(s.turn) {
                                        "current"
                                    } else {
                                        "historical"
                                    },
                                    fact.invalid_at
                                        .map_or_else(|| "—".into(), |t| t.to_string()),
                                    fact.expired_at
                                        .map_or_else(|| "—".into(), |t| t.to_string())
                                ))
                                .weak()
                                .small(),
                            );
                        }
                    },
                );
                egui::CollapsingHeader::new("Judgment state (what Jev sees)")
                    .default_open(self.expand_state)
                    .show(ui, |ui| {
                        ui.code(
                            serde_json::to_string_pretty(&insp.judgment_state)
                                .unwrap_or_else(|_| "{}".into()),
                        );
                    });
            });
    }
}

/// Horizontal day-boundary divider with centered label.
fn day_divider(ui: &mut egui::Ui, text: &str) {
    let height = 20.0;
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), height),
        egui::Sense::hover(),
    );
    let painter = ui.painter_at(rect);
    let y = rect.center().y;
    painter.line_segment(
        [egui::pos2(rect.left(), y), egui::pos2(rect.right(), y)],
        egui::Stroke::new(1.0, Color32::from_gray(90)),
    );
    let font = egui::FontId::proportional(11.0);
    let galley = painter.layout_no_wrap(text.to_owned(), font, Color32::from_gray(150));
    let pad = egui::vec2(8.0, 2.0);
    let bg = egui::Rect::from_center_size(rect.center(), galley.size() + pad * 2.0);
    painter.rect_filled(bg, 4.0, ui.visuals().panel_fill);
    painter.galley(
        rect.center() - galley.size() * 0.5,
        galley,
        Color32::from_gray(150),
    );
}

/// Compact generation marker for the roster: founders are unmarked, children
/// carry `g<generation>` right after the name.
fn gen_marker(generation: u8) -> String {
    if generation == 0 {
        String::new()
    } else {
        format!(" g{generation}")
    }
}
/// Derive a short cause phrase from a death event's text.
fn death_cause(text: &str) -> String {
    let lower = text.to_lowercase();
    if lower.contains("starv") || lower.contains("hunger") || lower.contains("died of") {
        "starved".into()
    } else {
        "died".into()
    }
}

fn fmt(v: Option<f32>) -> String {
    v.map_or_else(|| "—".into(), |x| format!("{x:.2}"))
}

/// The per-turn metadata line: verdict and gate scores, then the judgment
/// layer's own labels. Optional fields are omitted, not rendered blank, so a
/// turn saved before they existed still reads as one line.
#[must_use]
fn turn_meta(a: &AgentTurn) -> String {
    let base = format!(
        "{} · social {} · grounded {} · intensity {}",
        if a.accepted { "Accepted" } else { "Rejected" },
        humanize(&a.social),
        fmt(a.grounded),
        fmt(a.intensity)
    );
    match turn_labels(a) {
        Some(labels) => format!("{base} · {labels}"),
        None => base,
    }
}

/// The judgment layer's labels for a turn: motive, register, addressee.
///
/// `why` used to be the voice model's own rationale and is now the motive
/// label the judgment layer chose, so it is worth showing; `register` and
/// `addressee` are derived in code and were computed but never rendered.
/// Absent fields are omitted rather than rendered blank, which keeps a turn
/// saved before they existed readable.
#[must_use]
fn turn_labels(a: &AgentTurn) -> Option<String> {
    let mut labels = Vec::new();
    if !a.why.trim().is_empty() {
        labels.push(format!("Motive: {}", a.why.trim()));
    }
    if let Some(register) = &a.register {
        labels.push(format!("Register: {register}"));
    }
    if let Some(addressee) = &a.addressee {
        labels.push(format!("To: {addressee}"));
    }
    if labels.is_empty() {
        None
    } else {
        Some(labels.join(" · "))
    }
}

/// What the feed prints for one turn's words.
///
/// A villager can now be committed silent: the voice model's line was a
/// verbatim copy of the card it was given, so it was dropped and `say` is
/// empty while `note` explains why. An empty `say` must not become an empty
/// label; the note takes the line's place (and keeps the muted style the note
/// already has), never both.
#[derive(Debug, PartialEq, Eq)]
enum TurnWords<'a> {
    /// The spoken line, with the note beneath it when the turn carried one.
    Spoken { say: &'a str, note: Option<&'a str> },
    /// A silent turn: the note replaces the missing line.
    Silent { note: &'a str },
    /// Neither words nor a reason to print.
    Nothing,
}

#[must_use]
fn turn_words(a: &AgentTurn) -> TurnWords<'_> {
    if a.say.trim().is_empty() {
        a.note
            .as_deref()
            .map_or(TurnWords::Nothing, |note| TurnWords::Silent { note })
    } else {
        TurnWords::Spoken {
            say: a.say.as_str(),
            note: a.note.as_deref(),
        }
    }
}

#[allow(dead_code, reason = "compile-time assertion only; never called")]
const fn _assert_state_send_sync() {
    const fn check<T: Send + Sync>() {}
    check::<AppState>();
    check::<Arc<AppState>>();
}

fn humanize(id: &str) -> String {
    let text = id.replace('_', " ").replace("go:", "Go to ");
    let mut chars = text.chars();
    chars
        .next()
        .map(|c| format!("{}{}", c.to_uppercase(), chars.as_str()))
        .unwrap_or_default()
}

fn inventory_text(food: u32, water: u32, wood: u32, herbs: u32, tools: u32, coin: u32) -> String {
    format!(
        "Food {food} · Water {water} · Wood {wood} · Herbs {herbs} · Tools {tools} · Coin {coin}"
    )
}

const fn matches_feed_filter(kind: EventKind, filter: FeedFilter) -> bool {
    use EventKind::{
        Accusation, Betrayal, Birth, Bond, Confusion, Craft, Death, Divine, Harvest, Heal, Move,
        Prayer, Promise, Repair, Rest, Speech, System, Trade, Transfer,
    };
    match filter {
        FeedFilter::All => true,
        FeedFilter::Social => matches!(kind, Speech | Promise | Bond | Heal | Birth | Rest),
        FeedFilter::Economy => matches!(kind, Harvest | Transfer | Craft | Repair | Trade | Move),
        FeedFilter::Crisis => matches!(kind, Betrayal | Accusation | Death | Confusion),
        FeedFilter::Sacred => matches!(kind, Divine | Prayer | System),
    }
}

fn role_color(role_id: &str) -> Color32 {
    match role_id {
        "steward" | "elder" => Color32::from_rgb(235, 195, 115),
        "farmer" => Color32::from_rgb(150, 210, 135),
        "herbalist" | "healer" => Color32::from_rgb(130, 215, 185),
        "woodcutter" | "forester" => Color32::from_rgb(185, 155, 115),
        "smith" | "builder" | "artisan" => Color32::from_rgb(215, 150, 110),
        _ => Color32::from_rgb(145, 185, 235),
    }
}

fn season_accent(season: &str) -> Color32 {
    match season.to_lowercase().as_str() {
        "spring" => Color32::from_rgb(155, 220, 155),
        "summer" => Color32::from_rgb(240, 210, 120),
        "autumn" => Color32::from_rgb(235, 160, 100),
        "winter" => Color32::from_rgb(160, 205, 245),
        _ => Color32::from_gray(210),
    }
}

fn canvas_atmosphere(season: &str, time_of_day: &str) -> (Color32, Color32) {
    let tod = time_of_day.to_lowercase();
    let base = if tod.contains("night") {
        Color32::from_rgb(13, 16, 26)
    } else if tod.contains("dawn") || tod.contains("dusk") || tod.contains("evening") {
        Color32::from_rgb(28, 23, 30)
    } else {
        match season.to_lowercase().as_str() {
            "winter" => Color32::from_rgb(22, 28, 36),
            "autumn" => Color32::from_rgb(27, 24, 22),
            _ => Color32::from_rgb(20, 27, 25),
        }
    };
    (base, season_accent(season))
}

fn place_glyph(id: &str) -> &'static str {
    match id {
        "granary" => "🏛",
        "market" => "⚖",
        "well" => "💧",
        "hut" => "🏠",
        "shrine" => "⛩",
        "woods" => "🌲",
        "field" => "🌾",
        _ => "◈",
    }
}

fn place_card_fill(id: &str, selected: bool) -> Color32 {
    if selected {
        return Color32::from_rgb(62, 54, 38);
    }
    match id {
        "granary" => Color32::from_rgb(44, 39, 32),
        "market" => Color32::from_rgb(42, 36, 44),
        "well" => Color32::from_rgb(30, 42, 48),
        "hut" => Color32::from_rgb(45, 36, 33),
        "shrine" => Color32::from_rgb(38, 35, 48),
        "woods" => Color32::from_rgb(29, 41, 34),
        "field" => Color32::from_rgb(41, 42, 29),
        _ => Color32::from_rgb(38, 42, 48),
    }
}

fn draw_weather_overlay(painter: &egui::Painter, rect: egui::Rect, weather_kind: &str, turn: u64) {
    let wk = weather_kind.to_lowercase();
    let phase = (turn % 17) as f32;
    if wk.contains("rain") || wk.contains("storm") {
        for i in 0..18 {
            let fx = (phase.mul_add(19.0, i as f32 * 53.0) % rect.width()).max(8.0);
            let fy = (phase.mul_add(23.0, i as f32 * 37.0) % (rect.height() - 24.0)).max(8.0);
            let p1 = egui::pos2(rect.left() + fx, rect.top() + fy);
            let p2 = p1 + egui::vec2(-4.0, 12.0);
            painter.line_segment(
                [p1, p2],
                egui::Stroke::new(1.0, Color32::from_rgba_unmultiplied(130, 175, 230, 55)),
            );
        }
    } else if wk.contains("snow") || wk.contains("frost") {
        for i in 0..16 {
            let fx = (phase.mul_add(13.0, i as f32 * 61.0) % rect.width()).max(8.0);
            let fy = (phase.mul_add(17.0, i as f32 * 41.0) % (rect.height() - 16.0)).max(8.0);
            painter.circle_filled(
                egui::pos2(rect.left() + fx, rect.top() + fy),
                2.0,
                Color32::from_rgba_unmultiplied(230, 240, 255, 65),
            );
        }
    }
}

const fn memory_kind(kind: society_sim::MemoryKind) -> &'static str {
    use society_sim::MemoryKind::{Milestone, Own, Reflection, Scar, Tradition, Witnessed};
    match kind {
        Own => "Own",
        Witnessed => "Witnessed",
        Reflection => "Reflection",
        Scar => "Scar",
        Tradition => "Tradition",
        Milestone => "Milestone",
    }
}

const fn event_style(kind: EventKind) -> (&'static str, Color32) {
    use EventKind::{
        Accusation, Betrayal, Birth, Bond, Confusion, Craft, Death, Divine, Harvest, Heal, Move,
        Prayer, Promise, Repair, Rest, Speech, System, Trade, Transfer,
    };
    let label = match kind {
        Speech => "Speech",
        Move => "Movement",
        Transfer => "Supplies",
        Harvest => "Harvest",
        Rest => "Rest",
        Betrayal => "Betrayal",
        Accusation => "Accusation",
        Promise => "Promise",
        Death => "Death",
        Birth => "Birth",
        Divine => "Divine",
        Craft => "Crafting",
        Heal => "Healing",
        Prayer => "Prayer",
        Repair => "Repair",
        Trade => "Trade",
        Bond => "Bond",
        Confusion => "Rejected",
        System => "Village",
    };
    let color = match kind {
        Death | Betrayal | Accusation | Confusion => Color32::from_rgb(230, 140, 120),
        Birth | Heal | Bond => Color32::from_rgb(150, 205, 165),
        Harvest | Transfer | Craft | Repair | Trade => Color32::from_rgb(215, 190, 130),
        Divine | Prayer => Color32::from_rgb(190, 165, 230),
        _ => Color32::from_gray(200),
    };
    (label, color)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn observer(world: World, rt: &tokio::runtime::Runtime) -> App {
        use society_server::inference::judgment::{Judge, JudgeConfig};
        let judge = Arc::new(Judge::new(JudgeConfig {
            endpoint: "http://127.0.0.1:1".into(),
            api_key: "test".into(),
            model: "test".into(),
            timeout: Duration::from_secs(1),
        }));
        let voice = Arc::new(society_server::Voice::new(
            society_server::VoiceConfig::default(),
        ));
        let save_path = std::path::PathBuf::from("/unused-ui-test-save.json");
        let engine = Arc::new(society_server::Engine::new(
            Arc::clone(&judge),
            Arc::clone(&voice),
            2,
            save_path.clone(),
        ));
        App::new(
            rt.handle().clone(),
            AppState {
                world: Arc::new(tokio::sync::Mutex::new(world)),
                run: Arc::new(tokio::sync::Mutex::new(Vec::new())),
                engine,
                judge,
                voice,
                save_path,
            },
            Launch {
                port: 8100,
                view: "world",
                select: Some("Ada"),
                expand_state: false,
                select_none: false,
                size: None,
                autostart: false,
                focus: false,
            },
        )
    }

    fn render(app: &mut App, tab: Tab, size: [f32; 2]) -> String {
        let ctx = egui::Context::default();
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size.into())),
                ..Default::default()
            },
            |root| {
                egui::CentralPanel::default().show(root, |ui| match tab {
                    Tab::World => app.draw_map(ui),
                    Tab::Council => app.draw_council(ui),
                    Tab::Feed => app.draw_feed(ui),
                    Tab::Inspector => app.draw_inspector(ui),
                });
            },
        );
        #[allow(
            clippy::items_after_statements,
            reason = "helper sits next to its single use"
        )]
        fn texts(shape: &egui::epaint::Shape, out: &mut String) {
            match shape {
                egui::epaint::Shape::Text(t) => {
                    out.push_str(&t.galley.job.text);
                    out.push('\n');
                },
                egui::epaint::Shape::Vec(shapes) => {
                    for s in shapes {
                        texts(s, out);
                    }
                },
                _ => {},
            }
        }
        let mut text = String::new();
        output.textures_delta.clear(); // Headless renderer does not upload textures.
        for clipped in output.shapes {
            texts(&clipped.shape, &mut text);
        }
        text
    }

    #[test]
    fn world_and_inspector_render_extended_simulation_state() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let mut w = World::founding(7);
        let ada = w.agent_mut("Ada").unwrap();
        ada.water = 7;
        ada.ailment = Some(society_sim::Ailment::Fever);
        ada.current_goal = "Find medicinal herbs".into();
        let mut app = observer(w, &rt);
        for size in [[900.0, 600.0], [1120.0, 760.0]] {
            let world = render(&mut app, Tab::World, size);
            assert!(world.contains("Village stockpiles"), "{world}");
            assert!(
                world.contains("Water") && world.contains("Herbs"),
                "{world}"
            );
        }
        let inspector = render(&mut app, Tab::Inspector, [900.0, 2400.0]);
        for expected in [
            "Steward",
            "Fever",
            "Find medicinal herbs",
            "Water: 7",
            "Health",
            "Thirst",
            "Skills",
            "Relationships & trust",
            "Available actions",
            "Memories",
        ] {
            assert!(
                inspector.contains(expected),
                "missing {expected} in {inspector}"
            );
        }
    }

    #[test]
    fn saved_events_render_without_session_turn_records() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let mut w = World::founding(7);
        for (kind, action) in [
            (EventKind::Craft, "craft"),
            (EventKind::Heal, "heal"),
            (EventKind::Trade, "trade"),
            (EventKind::Prayer, "pray"),
            (EventKind::Repair, "repair"),
            (EventKind::Bond, "console"),
        ] {
            w.push_event(Event {
                turn: 0,
                actor: "Ada".into(),
                place: Place::Market,
                kind,
                action: action.into(),
                text: format!("saved {action}"),
                social: None,
                intensity: None,
                grounded: None,
                divine: false,
            });
        }
        let mut app = observer(w, &rt);
        assert!(app.turns.is_empty());
        let feed = render(&mut app, Tab::Feed, [1120.0, 1800.0]);
        for expected in [
            "saved craft",
            "saved heal",
            "saved trade",
            "saved pray",
            "saved repair",
            "saved console",
        ] {
            assert!(feed.contains(expected), "missing {expected} in {feed}");
        }
    }

    #[test]
    fn council_and_village_systems_enrich_living_parish() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let mut w = World::founding(7);
        w.turn = 12;
        if let Some(ada) = w.agent_mut("Ada") {
            ada.hunger = 2;
            ada.food = 0;
        }
        let emitted = apply_village_systems(&mut w);
        assert!(emitted >= 1, "expected festival or parish relief event");
        let mut app = observer(w, &rt);
        let council = render(&mut app, Tab::Council, [1120.0, 1800.0]);
        for expected in [
            "Parish Council & Economy",
            "Civic Leadership & Decrees",
            "Market Exchange & Scarcity Index",
            "Households & Hearths",
            "Village Factions & Feuds",
        ] {
            assert!(
                council.contains(expected),
                "missing {expected} in {council}"
            );
        }
    }

    /// A committed turn with no matching event, so the feed takes the
    /// eventless row path: the one that prints the villager's words.
    fn feed_with_turn(say: &str, note: Option<&str>) -> String {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let w = World::founding(7);
        let mut app = observer(w, &rt);
        app.turns.push(TurnRecord {
            turn: 3,
            day: 1,
            intent_call: None,
            gate_call: None,
            agents: vec![AgentTurn {
                name: "Ada".into(),
                action: "speak".into(),
                accepted: true,
                note: note.map(str::to_owned),
                say: say.into(),
                why: "hunger".into(),
                social: "speak".into(),
                grounded: Some(0.7),
                intensity: Some(1.5),
                confidence: Some(0.9),
                latency_ms: 12,
                model: "test".into(),
                state: serde_json::Value::Null,
                addressee: Some("Bo".into()),
                register: Some("wary and guarded".into()),
                dilemma: None,
            }],
            births: Vec::new(),
        });
        render(&mut app, Tab::Feed, [1120.0, 1200.0])
    }

    /// A turn with the two fields the observer could not see before, and no
    /// words: the state the engine commits when it drops a copied line.
    fn silent_turn(note: Option<&str>) -> AgentTurn {
        AgentTurn {
            name: "Ada".into(),
            action: "speak".into(),
            accepted: true,
            note: note.map(str::to_owned),
            say: String::new(),
            why: "hunger".into(),
            social: "speak".into(),
            grounded: Some(0.7),
            intensity: Some(1.5),
            confidence: Some(0.9),
            latency_ms: 12,
            model: "test".into(),
            state: serde_json::Value::Null,
            addressee: Some("Bo".into()),
            register: Some("wary and guarded".into()),
            dilemma: None,
        }
    }

    #[test]
    fn silent_turn_renders_its_note_instead_of_an_empty_line() {
        let note = "dropped a line copied verbatim from the card";
        let feed = feed_with_turn("", Some(note));
        // The note takes the missing line's place, printed once and not twice.
        assert_eq!(feed.matches(note).count(), 1, "{feed}");
        // The words drawn for that turn are the note: the last thing the row
        // shows is the note, where the line would have been. (The flat text
        // dump also contains blank lines between sections, so the property
        // under test is the row's own text, not the absence of blanks.)
        let header = feed
            .lines()
            .position(|l| l.contains("Ada · Speak · Accepted"))
            .expect("the villager's row");
        let row: Vec<&str> = feed
            .lines()
            .skip(header)
            .take_while(|l| !l.trim().is_empty())
            .collect();
        assert_eq!(row.last().copied(), Some(note), "{feed}");
        let turn = silent_turn(Some(note));
        assert_eq!(turn_words(&turn), TurnWords::Silent { note });
        // The judgment layer's labels are visible on the turn row.
        assert!(feed.contains("Motive: hunger"), "{feed}");
        assert!(feed.contains("Register: wary and guarded"), "{feed}");
        assert!(feed.contains("To: Bo"), "{feed}");
    }

    #[test]
    fn spoken_turn_renders_its_line() {
        let say = "the granary is short again";
        let note = "accepted after the gate";
        let feed = feed_with_turn(say, Some(note));
        assert!(feed.contains(say), "{feed}");
        assert!(feed.contains(note), "{feed}");
        let mut turn = silent_turn(Some(note));
        turn.say = say.into();
        assert_eq!(
            turn_words(&turn),
            TurnWords::Spoken {
                say,
                note: Some(note)
            }
        );
    }

    #[test]
    fn turn_metadata_prints_judgment_labels_and_omits_absent_ones() {
        let turn = silent_turn(Some("silent"));
        assert_eq!(
            turn_meta(&turn),
            "Accepted · social Speak · grounded 0.70 · intensity 1.50 \
             · Motive: hunger · Register: wary and guarded · To: Bo"
        );
        let mut bare = turn;
        bare.why = String::new();
        bare.register = None;
        bare.addressee = None;
        assert_eq!(
            turn_meta(&bare),
            "Accepted · social Speak · grounded 0.70 · intensity 1.50"
        );
    }
}
