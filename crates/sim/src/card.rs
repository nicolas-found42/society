//! State formatting. Two distinct renderings, because they go to different
//! readers with different failure modes:
//!
//! * `judgment_state` — structured JSON for the judgment layer. Field names are
//!   semantically load-bearing (measured: `here` vs `food_here` flips the
//!   decision), so they are a published contract, not an implementation detail.
//! * `agent_card` — terse prose for a 0.6B model. No narration, no restating,
//!   one line requested.

use serde_json::{json, Map, Value};

use crate::action::affordances;
use crate::world::{Agent, Place, World, ADJACENCY};

/// The exact JSON handed to the judgment layer for one agent's turn.
pub fn judgment_state(w: &World, a: &Agent) -> Value {
    let here = w.place_state(a.place);
    let present: Map<String, Value> = w
        .present(a.place, &a.name)
        .into_iter()
        .map(|n| {
            let other = w.agent(&n).expect("present agent exists");
            (
                n,
                json!({
                    "carries_food": other.food,
                    "hunger_of_3": other.hunger,
                    "role": other.role.id(),
                    "mood": other.mood.id(),
                    "health_of_10": other.health,
                    "ailment": other.ailment.map(|x| x.id()),
                }),
            )
        })
        .collect();

    let memories: Vec<Value> = a
        .memories_brief(w.turn)
        .into_iter()
        .map(|(weight, text)| json!({ "text": text, "weight": round2(weight) }))
        .collect();

    let mut rel = Map::new();
    for (n, v) in &a.trust {
        rel.insert(n.clone(), json!(round2(*v)));
    }

    let kinship: Map<String, Value> = a
        .relations
        .iter()
        .map(|r| {
            (
                r.other.clone(),
                json!({
                    "kinship": r.kinship.id(),
                    "affinity": round2(r.affinity),
                    "respect": round2(r.respect),
                    "grudge": round2(r.grudge),
                }),
            )
        })
        .collect();

    json!({
        "name": a.name,
        "personality": a.persona,
        "role": a.role.id(),
        "mood": a.mood.id(),
        "current_goal": a.current_goal,
        "place": a.place.id(),
        // Deliberately explicit: the judgment layer reads key names as meaning.
        "food_carried": a.food,
        "food_here": here.food,
        "food_here_owner": here.owner,
        "hunger_of_3": a.hunger,
        "starving_turns": a.starving_turns,
        "standing_in_village": round2(a.standing),
        "vitals": {
            "hunger_of_3": a.hunger,
            "thirst_of_3": a.thirst,
            "energy_of_10": a.energy,
            "warmth_of_4": a.warmth,
            "health_of_10": a.health,
            "stress_of_4": round2(a.stress),
            "hope_of_4": round2(a.hope),
            "devotion_of_4": round2(a.devotion),
            "ailment": a.ailment.map(|x| x.id()),
        },
        "inventory": {
            "food": a.food,
            "water": a.water,
            "wood": a.wood,
            "herbs": a.herbs,
            "tools": a.tools,
            "coin": a.coin,
        },
        "place_resources": {
            "food": here.food,
            "water": here.water,
            "wood": here.wood,
            "herbs": here.herbs,
            "tools": here.tools,
            "coin": here.coin,
            "condition_of_5": here.condition,
            "hearth_lit": here.hearth_lit,
            "fertility": round2(here.fertility),
        },
        "environment": {
            "day": w.day,
            "turn": w.turn,
            "season": w.season().id(),
            "time_of_day": w.time_of_day().id(),
            "weather_kind": w.weather_kind.id(),
            "weather_multiplier": round2(w.weather),
            "temperature_c": w.temperature_c,
            "village_morale": round2(w.village_morale),
        },
        "skills": {
            "farming": a.skills.farming,
            "forestry": a.skills.forestry,
            "herbalism": a.skills.herbalism,
            "craft": a.skills.craft,
            "stewardship": a.skills.stewardship,
            "counsel": a.skills.counsel,
        },
        "people_present": present,
        "adjacent_places": a.place.adjacent().iter().map(|p| p.id()).collect::<Vec<_>>(),
        "memories": memories,
        "trust": rel,
        "kinship": kinship,
        "partner": a.partner,
        "unresolved_promise": a.promise.as_ref().map(|p| json!({
            "to": p.to, "text": p.text, "made_turn": p.made_turn
        })),
        "legal_actions": affordances(w, a),
    })
}

/// Terse card for the small model. The model writes one line; it must not
/// restate this card, and it is told so.
pub fn agent_card(w: &World, a: &Agent, decision: &str) -> String {
    let here = w.place_state(a.place);
    let others = w.present(a.place, &a.name);
    let present = if others.is_empty() {
        "nobody".to_string()
    } else {
        others
            .iter()
            .map(|n| {
                let o = w.agent(n).expect("present agent exists");
                format!(
                    "{} ({}, carries {} food, hunger {}/3, mood {})",
                    o.name,
                    o.role.id(),
                    o.food,
                    o.hunger,
                    o.mood.id()
                )
            })
            .collect::<Vec<_>>()
            .join(", ")
    };
    // Dedupe: identical memories (broadcast events, repeated lines) make nine
    // agents converge on one prompt, and the small model then parrots them.
    let mut seen = std::collections::HashSet::new();
    let mut own = Vec::new();
    let mut other = Vec::new();
    for (_weight, text) in a.memories_brief(w.turn) {
        if !seen.insert(text.clone()) {
            continue;
        }
        let line = format!("- {text}");
        if text.starts_with("You ") {
            own.push(line);
        } else {
            other.push(line);
        }
    }
    let mut mems: Vec<String> = own.into_iter().take(2).collect();
    mems.extend(other.into_iter().take(1));

    let mut s = String::new();
    s.push_str(&format!(
        "You are {}, the village {} ({}). Mood: {}.\n",
        a.name,
        a.role.id(),
        a.persona,
        a.mood.id()
    ));
    s.push_str(&format!(
        "Season: {}, {} ({}°C). In the {}. You carry {} food, {} water, {} wood, {} herbs, {} tools.\n",
        w.season().id(),
        w.time_of_day().id(),
        w.temperature_c,
        a.place.id(),
        a.food,
        a.water,
        a.wood,
        a.herbs,
        a.tools
    ));
    s.push_str(&format!(
        "Local stores: {} food, {} water, {} wood, {} herbs. People here: {}.\n",
        here.food,
        here.water,
        here.wood,
        here.herbs,
        present
    ));
    s.push_str(&format!(
        "Hunger {}/3, Thirst {}/3, Energy {}/10, Warmth {}/4, Standing {}/4. Goal: {}.\n",
        a.hunger, a.thirst, a.energy, a.warmth, round1(a.standing), a.current_goal
    ));
    if !mems.is_empty() {
        s.push_str("You remember (context, not a script; never repeat these words):\n");
        for m in &mems {
            s.push_str(m);
            s.push('\n');
        }
    }
    s.push_str(&format!("You have decided to {decision}.\n"));
    s.push_str("Say one line aloud. Do not describe the situation or count your food unless you are making a demand or an offer.");
    s
}

/// Rich world summary and dashboard for the observer UI and chronicler.
pub fn world_brief(w: &World, max_events: usize) -> String {
    render_village_ui(w, max_events)
}

fn meter(val: u32, max: u32, width: usize) -> String {
    let clamped = val.min(max);
    let filled = if max == 0 {
        0
    } else {
        ((clamped as f32 / max as f32) * width as f32).round() as usize
    };
    let empty = width.saturating_sub(filled);
    format!("{}{}", "█".repeat(filled), "░".repeat(empty))
}

/// Full Unicode dashboard rendering of the entire village state, ring topology,
/// stockpiles, villager vitals, social bonds, and recent chronicle.
pub fn render_village_ui(w: &World, max_events: usize) -> String {
    let season = w.season();
    let tod = w.time_of_day();
    let total_food: u32 = w.places.iter().map(|(_, s)| s.food).sum::<u32>()
        + w.living().map(|a| a.food).sum::<u32>();
    let total_water: u32 = w.places.iter().map(|(_, s)| s.water).sum::<u32>()
        + w.living().map(|a| a.water).sum::<u32>();
    let total_wood: u32 = w.places.iter().map(|(_, s)| s.wood).sum::<u32>()
        + w.living().map(|a| a.wood).sum::<u32>();
    let total_herbs: u32 = w.places.iter().map(|(_, s)| s.herbs).sum::<u32>()
        + w.living().map(|a| a.herbs).sum::<u32>();
    let total_tools: u32 = w.places.iter().map(|(_, s)| s.tools).sum::<u32>()
        + w.living().map(|a| a.tools).sum::<u32>();
    let total_coin: u32 = w.places.iter().map(|(_, s)| s.coin).sum::<u32>()
        + w.living().map(|a| a.coin).sum::<u32>();

    let mut s = String::new();
    s.push_str("╔══════════════════════════════════════════════════════════════════════════════════════════════╗\n");
    s.push_str(&format!(
        "║  {} {:<8} │ {} {:<9} │ Day {:<2} Turn {:<3} │ {} {:<13} {:>3}°C (x{:.1}) │ Pop {:>2}/{:<2} ║\n",
        season.icon(),
        season.id().to_uppercase(),
        tod.icon(),
        tod.id(),
        w.day,
        w.turn,
        w.weather_kind.icon(),
        w.weather_kind.id(),
        w.temperature_c,
        w.weather,
        w.living().count(),
        crate::world::POPULATION_CAP
    ));
    s.push_str("╠══════════════════════════════════════════════════════════════════════════════════════════════╣\n");
    s.push_str(&format!(
        "║  VILLAGE COMMONS │ 🌾 Food {:>2}  💧 Water {:>2}  🪵 Wood {:>2}  🌿 Herbs {:>2}  ⚒ Tools {:>2}  🪙 Coin {:>2}   ║\n",
        total_food, total_water, total_wood, total_herbs, total_tools, total_coin
    ));
    s.push_str("╚══════════════════════════════════════════════════════════════════════════════════════════════╝\n");

    s.push_str(&render_place_map(w));
    s.push_str("\n┌─ PLACES & STOCKPILES ────────────────────────────────────────────────────────────────────────┐\n");
    for (p, st) in &w.places {
        let who: Vec<String> = w
            .living()
            .filter(|a| a.place == *p)
            .map(|a| format!("{}({})", a.name, a.role.icon()))
            .collect();
        let hearth = if st.hearth_lit { "🔥" } else { "  " };
        let owner_tag = st
            .owner
            .as_ref()
            .map(|o| format!(" [held:{o}]"))
            .unwrap_or_default();
        s.push_str(&format!(
            "│ {} {:<8} {} [{}] │ 🌾{:>2} 💧{:>2} 🪵{:>2} 🌿{:>2} ⚒{:>1} 🪙{:>2}{} │ {}\n",
            p.icon(),
            p.id(),
            hearth,
            meter(st.condition as u32, 5, 5),
            st.food,
            st.water,
            st.wood,
            st.herbs,
            st.tools,
            st.coin,
            owner_tag,
            if who.is_empty() {
                "nobody".to_string()
            } else {
                who.join(", ")
            }
        ));
    }
    s.push_str("└──────────────────────────────────────────────────────────────────────────────────────────────┘\n");

    s.push_str("\n┌─ VILLAGERS ──────────────────────────────────────────────────────────────────────────────────┐\n");
    s.push_str("│ Name   Role       At       Mood         Vitals (Full/Hydr/Eng/Wrm/HP)  Carry (F/W/Wd/H/T)   │\n");
    s.push_str("├──────────────────────────────────────────────────────────────────────────────────────────────┤\n");
    for a in w.living() {
        let fullness = 3u8.saturating_sub(a.hunger);
        let hydration = 3u8.saturating_sub(a.thirst);
        let ailment_str = a
            .ailment
            .map(|al| format!(" !{}", al.id()))
            .unwrap_or_default();
        let partner_str = a
            .partner
            .as_ref()
            .map(|p| format!(" ♥{p}"))
            .unwrap_or_default();
        s.push_str(&format!(
            "│ {:<6} {} {:<9} {:<8} {} {:<9} [{}/{}/{}e/{}w/{:>2}hp]  {:>1}f {:>1}w {:>1}wd {:>1}h {:>1}t │ st:{:.1}{}{}\n",
            a.name,
            a.role.icon(),
            a.role.id(),
            a.place.id(),
            a.mood.icon(),
            a.mood.id(),
            fullness,
            hydration,
            a.energy,
            a.warmth,
            a.health,
            a.food,
            a.water,
            a.wood,
            a.herbs,
            a.tools,
            a.standing,
            partner_str,
            ailment_str
        ));
    }
    s.push_str("└──────────────────────────────────────────────────────────────────────────────────────────────┘\n");

    if max_events > 0 {
        let start = w.events.len().saturating_sub(max_events);
        s.push_str("\n┌─ VILLAGE CHRONICLE ──────────────────────────────────────────────────────────────────────────┐\n");
        for e in &w.events[start..] {
            s.push_str(&format!("│ [t{:>3}] {:<8} │ {}\n", e.turn, format!("{:?}", e.kind), e.line()));
        }
        s.push_str("└──────────────────────────────────────────────────────────────────────────────────────────────┘\n");
    }
    s
}

/// Renders a visual topological ring map of the 7 village locations.
pub fn render_place_map(w: &World) -> String {
    let badge = |p: Place| -> String {
        let st = w.place_state(p);
        let count = w.living().filter(|a| a.place == p).count();
        format!(
            "[{} {:<7} 🌾{:>2} 👤{}]",
            p.icon(),
            p.id(),
            st.food,
            count
        )
    };
    format!(
        "\n          {}\n               ╱              ╲\n  {}          {}\n       │                              │\n  {}          {}\n               ╲              ╱\n          {} ──── {}\n",
        badge(Place::Granary),
        badge(Place::Market),
        badge(Place::Field),
        badge(Place::Well),
        badge(Place::Woods),
        badge(Place::Hut),
        badge(Place::Shrine),
    )
}

/// Detailed dossier card for inspecting a single villager's psyche, skills, kinship, and beliefs.
pub fn render_villager_inspector(w: &World, name: &str) -> Option<String> {
    let a = w.agent(name)?;
    let (top_skill, top_lvl) = a.skills.top_skill();
    let mut s = String::new();
    s.push_str(&format!(
        "┏━━ VILLAGER DOSSIER: {} ({} {}) ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━┓\n",
        a.name,
        a.role.icon(),
        a.role.id().to_uppercase()
    ));
    s.push_str(&format!("┃ Persona: {}\n", a.persona));
    s.push_str(&format!(
        "┃ Location: {} ({}) │ Generation: {} │ Born Turn: {} │ Alive: {}\n",
        a.place.title(),
        a.place.id(),
        a.generation,
        a.born_turn,
        a.alive
    ));
    s.push_str(&format!(
        "┃ Mood: {} {} │ Stress: {:.1}/4 │ Hope: {:.1}/4 │ Devotion: {:.1}/4 │ Standing: {:.1}/4\n",
        a.mood.icon(),
        a.mood.id(),
        a.stress,
        a.hope,
        a.devotion,
        a.standing
    ));
    s.push_str(&format!("┃ Active Drive: {}\n", a.current_goal));
    s.push_str(&format!(
        "┃ Skills: Farming {} · Forestry {} · Herbalism {} · Craft {} · Stewardship {} · Counsel {} (Best: {} {})\n",
        a.skills.farming,
        a.skills.forestry,
        a.skills.herbalism,
        a.skills.craft,
        a.skills.stewardship,
        a.skills.counsel,
        top_skill,
        top_lvl
    ));
    if !a.relations.is_empty() {
        let rels: Vec<String> = a
            .relations
            .iter()
            .map(|r| {
                format!(
                    "{}:{} (trust {:.1}, grudge {:.1})",
                    r.other,
                    r.kinship.id(),
                    a.trust_of(&r.other),
                    r.grudge
                )
            })
            .collect();
        s.push_str(&format!("┃ Ties: {}\n", rels.join(" | ")));
    }
    s.push_str("┗━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━┛\n");
    Some(s)
}

/// Renders a compact social & kinship summary across all living villagers.
pub fn render_social_matrix(w: &World) -> String {
    let mut s = String::from("SOCIAL BONDS & KINSHIP:\n");
    for a in w.living() {
        let highlights: Vec<String> = a
            .relations
            .iter()
            .filter(|r| r.kinship != crate::world::Kinship::Acquaintance || r.grudge >= 1.0)
            .map(|r| format!("{}[{}]", r.other, r.kinship.id()))
            .collect();
        if !highlights.is_empty() {
            s.push_str(&format!("  {:<6} -> {}\n", a.name, highlights.join(", ")));
        }
    }
    s
}

pub fn adjacency_table() -> &'static [(&'static str, [&'static str; 2])] {
    &ADJACENCY
}

fn round2(x: f32) -> f64 {
    (x * 100.0).round() as f64 / 100.0
}

/// Same quantisation for anything embedded in prose.
fn round1(x: f32) -> f32 {
    (x * 10.0).round() / 10.0
}

/// Places an agent could reach in `n` moves, used by the observer UI only.
pub fn reachable(from: Place, n: usize) -> Vec<Place> {
    let mut seen = vec![from];
    let mut frontier = vec![from];
    for _ in 0..n {
        let mut next = Vec::new();
        for p in frontier {
            for q in p.adjacent() {
                if !seen.contains(q) {
                    seen.push(*q);
                    next.push(*q);
                }
            }
        }
        frontier = next;
    }
    seen
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::World;

    #[test]
    fn judgment_state_names_the_food_field() {
        let w = World::founding(1);
        let a = &w.agents[0];
        let v = judgment_state(&w, a);
        // Measured: `here` instead of `food_here` makes the judge say "speak"
        // even when the agent is starving in a full granary.
        assert!(v.get("food_here").is_some());
        assert!(v.get("food_carried").is_some());
        assert!(v.get("legal_actions").is_some());
    }

    #[test]
    fn reachable_covers_the_ring() {
        let r = reachable(Place::Granary, 2);
        assert!(r.contains(&Place::Granary));
        assert!(r.contains(&Place::Market));
        assert!(r.contains(&Place::Field));
    }
}
