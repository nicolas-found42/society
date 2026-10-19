//! State formatting. Two distinct renderings, because they go to different
//! readers with different failure modes:
//!
//! * `judgment_state` — structured JSON for the judgment layer. Field names are
//!   semantically load-bearing (measured: `here` vs `food_here` flips the
//!   decision), so they are a published contract, not an implementation detail.
//! * `agent_card` — terse prose for the voice model. No narration, no
//!   restating, one line requested.

use std::fmt::Write as _;

use serde_json::{json, Map, Value};

use crate::action::{action_phrase, affordances, shortlist};
use crate::world::{Agent, Place, World};

/// The exact JSON handed to the judgment layer for one agent's turn.
///
/// # Panics
///
/// Panics if a villager listed as present is missing from the world.
#[must_use]
#[allow(
    clippy::too_many_lines,
    reason = "one sequential rule table; splitting hides the order"
)]
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
                    "ailment": other.ailment.map(super::world::Ailment::id),
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
            "ailment": a.ailment.map(super::world::Ailment::id),
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
        // The menu the intent question offers: one representative per kind of
        // act, not every legal action. `legal_actions` stays for validation and
        // for callers that need the full set; the choice is always checked
        // against it, because an action outside the menu is still legal.
        "offered_actions": shortlist(w, a),
    })
}

/// Terse card for the voice model. The model writes one line; it must not
/// restate this card, and it is told so.
///
/// The card carries no goal, no memory of the villager's own spoken lines, and
/// no recent dialogue, because the 350M voice model returns the nearest
/// salient string instead of answering: with all three present, 10 of 18
/// sampled lines were copies of card text; with all three gone, 0 of 18, and
/// the lines were full sentences rather than one-word fragments ("taken",
/// "Grain"). The judgment layer keeps the goal and the dialogue in
/// [`judgment_state`], so the decision still knows them.
///
/// # Panics
///
/// Panics if a villager listed as present is missing from the world.
#[must_use]
pub fn agent_card(w: &World, a: &Agent, decision: &str) -> String {
    let here = w.place_state(a.place);
    let others = w.present(a.place, &a.name);
    let present = if others.is_empty() {
        "nobody".to_owned()
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
    // agents converge on one prompt, and the voice model then parrots them.
    let mut seen = std::collections::HashSet::new();
    let mut first_person = Vec::new();
    let mut witnessed = Vec::new();
    for (_weight, text) in a.memories_brief(w.turn) {
        // A memory of the villager's own spoken line is quoted in the same
        // shape the model is asked to produce, which makes it the most
        // attractive string in the card to hand back: with the goal, these
        // lines and the dialogue block, 10 of 18 sampled lines were copies.
        // With all three out, 0 of 18 were, and the lines were full sentences
        // instead of one-word fragments ("taken", "Grain"). The line stays in
        // memory and in `judgment_state`; only the prompt drops it.
        if text.starts_with("You said:") {
            continue;
        }
        if !seen.insert(text.clone()) {
            continue;
        }
        if text.starts_with("You ") {
            first_person.push(text);
        } else {
            witnessed.push(text);
        }
    }
    // Two first-person memories (acts and reflections) plus one witnessed
    // event, the slice as it was before, minus the spoken lines. If that
    // leaves nothing, the block is not printed at all.
    let mut mems: Vec<String> = first_person
        .into_iter()
        .take(2)
        .map(|text| format!("- {text}"))
        .collect();
    mems.extend(
        witnessed
            .into_iter()
            .take(1)
            .map(|text| format!("- {text}")),
    );

    let mut s = String::new();
    let _ = writeln!(
        s,
        "You are {}, the village {} ({}). Mood: {}.",
        a.name,
        a.role.id(),
        a.persona,
        a.mood.id()
    );
    let _ = writeln!(s,
        "Season: {}, {} ({}°C). In the {}. You carry {} food, {} water, {} wood, {} herbs, {} tools.",
        w.season().id(),
        w.time_of_day().id(),
        w.temperature_c,
        a.place.id(),
        a.food,
        a.water,
        a.wood,
        a.herbs,
        a.tools
    );
    let _ = writeln!(
        s,
        "Local stores: {} food, {} water, {} wood, {} herbs. People here: {}.",
        here.food, here.water, here.wood, here.herbs, present
    );
    let _ = writeln!(
        s,
        "Hunger {}/3, Thirst {}/3, Energy {}/10, Warmth {}/4, Standing {}/4.",
        a.hunger,
        a.thirst,
        a.energy,
        a.warmth,
        round1(a.standing)
    );
    // No goal line: a goal is a short phrase, and the model returns the
    // nearest short phrase, opening word first ("balance", "counsel them").
    // Removing it alone left the copy count unchanged (10 of 18) because the
    // memory block and the dialogue supplied the same kind of string; gone
    // with them, 0 of 18, and the lines were full sentences instead of
    // one-word fragments ("taken", "Grain"). The judgment state still carries
    // `current_goal`.
    if !mems.is_empty() {
        s.push_str("You remember (context, not a script; never repeat these words):\n");
        for m in &mems {
            s.push_str(m);
            s.push('\n');
        }
    }
    let _ = writeln!(s, "You have decided to {}.", action_phrase(decision));
    s.push_str("Say one line aloud. Do not describe the situation or count your food unless you are making a demand or an offer.");
    s
}

/// Rich world summary and dashboard for the observer UI and chronicler.
#[must_use]
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
#[allow(
    clippy::too_many_lines,
    reason = "one sequential rule table; splitting hides the order"
)]
#[must_use]
pub fn render_village_ui(w: &World, max_events: usize) -> String {
    let season = w.season();
    let tod = w.time_of_day();
    let total_food: u32 =
        w.places.iter().map(|(_, s)| s.food).sum::<u32>() + w.living().map(|a| a.food).sum::<u32>();
    let total_water: u32 = w.places.iter().map(|(_, s)| s.water).sum::<u32>()
        + w.living().map(|a| a.water).sum::<u32>();
    let total_timber: u32 =
        w.places.iter().map(|(_, s)| s.wood).sum::<u32>() + w.living().map(|a| a.wood).sum::<u32>();
    let total_herbs: u32 = w.places.iter().map(|(_, s)| s.herbs).sum::<u32>()
        + w.living().map(|a| a.herbs).sum::<u32>();
    let total_tools: u32 = w.places.iter().map(|(_, s)| s.tools).sum::<u32>()
        + w.living().map(|a| a.tools).sum::<u32>();
    let total_coin: u32 =
        w.places.iter().map(|(_, s)| s.coin).sum::<u32>() + w.living().map(|a| a.coin).sum::<u32>();

    let mut s = String::new();
    s.push_str("╔══════════════════════════════════════════════════════════════════════════════════════════════╗\n");
    let _ = writeln!(s,
        "║  {} {:<8} │ {} {:<9} │ Day {:<2} Turn {:<3} │ {} {:<13} {:>3}°C (x{:.1}) │ Pop {:>2}/{:<2} ║",
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
    );
    s.push_str("╠══════════════════════════════════════════════════════════════════════════════════════════════╣\n");
    let _ = writeln!(s,
        "║  VILLAGE COMMONS │ 🌾 Food {total_food:>2}  💧 Water {total_water:>2}  🪵 Wood {total_timber:>2}  🌿 Herbs {total_herbs:>2}  ⚒ Tools {total_tools:>2}  🪙 Coin {total_coin:>2}   ║"
    );
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
        let _ = writeln!(
            s,
            "│ {} {:<8} {} [{}] │ 🌾{:>2} 💧{:>2} 🪵{:>2} 🌿{:>2} ⚒{:>1} 🪙{:>2}{} │ {}",
            p.icon(),
            p.id(),
            hearth,
            meter(u32::from(st.condition), 5, 5),
            st.food,
            st.water,
            st.wood,
            st.herbs,
            st.tools,
            st.coin,
            owner_tag,
            if who.is_empty() {
                "nobody".to_owned()
            } else {
                who.join(", ")
            }
        );
    }
    s.push_str("└──────────────────────────────────────────────────────────────────────────────────────────────┘\n");

    s.push_str("\n┌─ VILLAGERS ──────────────────────────────────────────────────────────────────────────────────┐\n");
    s.push_str("│ Name   Role       At       Mood         Vitals (Full/Hydr/Eng/Wrm/HP)  Carry (F/W/Wd/H/T)   │\n");
    s.push_str("├──────────────────────────────────────────────────────────────────────────────────────────────┤\n");
    for a in w.living() {
        let fullness = 3_u8.saturating_sub(a.hunger);
        let hydration = 3_u8.saturating_sub(a.thirst);
        let ailment_str = a
            .ailment
            .map(|al| format!(" !{}", al.id()))
            .unwrap_or_default();
        let partner_str = a
            .partner
            .as_ref()
            .map(|p| format!(" ♥{p}"))
            .unwrap_or_default();
        let _ = writeln!(s,
            "│ {:<6} {} {:<9} {:<8} {} {:<9} [{}/{}/{}e/{}w/{:>2}hp]  {:>1}f {:>1}w {:>1}wd {:>1}h {:>1}t │ st:{:.1}{}{}",
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
        );
    }
    s.push_str("└──────────────────────────────────────────────────────────────────────────────────────────────┘\n");

    if max_events > 0 {
        let start = w.events.len().saturating_sub(max_events);
        s.push_str("\n┌─ VILLAGE CHRONICLE ──────────────────────────────────────────────────────────────────────────┐\n");
        for e in &w.events[start..] {
            let _ = writeln!(
                s,
                "│ [t{:>3}] {:<8} │ {}",
                e.turn,
                format!("{:?}", e.kind),
                e.line()
            );
        }
        s.push_str("└──────────────────────────────────────────────────────────────────────────────────────────────┘\n");
    }
    s
}

/// Renders a visual topological ring map of the 7 village locations.
#[must_use]
pub fn render_place_map(w: &World) -> String {
    let badge = |p: Place| -> String {
        let st = w.place_state(p);
        let count = w.living().filter(|a| a.place == p).count();
        format!("[{} {:<7} 🌾{:>2} 👤{}]", p.icon(), p.id(), st.food, count)
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
#[must_use]
pub fn render_villager_inspector(w: &World, name: &str) -> Option<String> {
    let a = w.agent(name)?;
    let (top_skill, top_lvl) = a.skills.top_skill();
    let mut s = String::new();
    let _ = writeln!(
        s,
        "┏━━ VILLAGER DOSSIER: {} ({} {}) ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━┓",
        a.name,
        a.role.icon(),
        a.role.id().to_uppercase()
    );
    let _ = writeln!(s, "┃ Persona: {}", a.persona);
    let _ = writeln!(
        s,
        "┃ Location: {} ({}) │ Generation: {} │ Born Turn: {} │ Alive: {}",
        a.place.title(),
        a.place.id(),
        a.generation,
        a.born_turn,
        a.alive
    );
    let _ =
        writeln!(s,
        "┃ Mood: {} {} │ Stress: {:.1}/4 │ Hope: {:.1}/4 │ Devotion: {:.1}/4 │ Standing: {:.1}/4",
        a.mood.icon(),
        a.mood.id(),
        a.stress,
        a.hope,
        a.devotion,
        a.standing
    );
    let _ = writeln!(s, "┃ Active Drive: {}", a.current_goal);
    let _ = writeln!(s,
        "┃ Skills: Farming {} · Forestry {} · Herbalism {} · Craft {} · Stewardship {} · Counsel {} (Best: {} {})",
        a.skills.farming,
        a.skills.forestry,
        a.skills.herbalism,
        a.skills.craft,
        a.skills.stewardship,
        a.skills.counsel,
        top_skill,
        top_lvl
    );
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
        let _ = writeln!(s, "┃ Ties: {}", rels.join(" | "));
    }
    s.push_str(
        "┗━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━┛\n",
    );
    Some(s)
}

/// Renders a compact social & kinship summary across all living villagers.
#[must_use]
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
            let _ = writeln!(s, "  {:<6} -> {}", a.name, highlights.join(", "));
        }
    }
    s
}

fn round2(x: f32) -> f64 {
    f64::from((x * 100.0).round()) / 100.0
}

/// Same quantisation for anything embedded in prose.
fn round1(x: f32) -> f32 {
    (x * 10.0).round() / 10.0
}

/// Places an agent could reach in `n` moves.
#[cfg(test)]
fn reachable(from: Place, n: usize) -> Vec<Place> {
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
    use crate::memory::{Memory, MemoryKind};
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
    fn judgment_state_offers_a_shortlist_next_to_the_legal_actions() {
        let w = World::founding(1);
        let a = &w.agents[0];
        let v = judgment_state(&w, a);
        // The intent question reads `offered_actions`; validation still reads
        // `legal_actions`, so both must be present and the menu must be the
        // narrower one.
        let offered: Vec<String> =
            serde_json::from_value(v.get("offered_actions").expect("shortlist").clone())
                .expect("a list of ids");
        let legal: Vec<String> =
            serde_json::from_value(v.get("legal_actions").expect("legal list").clone())
                .expect("a list of ids");
        assert!(!offered.is_empty());
        assert!(offered.len() <= legal.len());
        assert!(offered.iter().all(|action| legal.contains(action)));
    }

    #[test]
    fn card_renders_the_decision_as_prose() {
        let w = World::founding(1);
        let card = agent_card(&w, &w.agents[0], "draw_water");
        assert!(card.contains("You have decided to draw water from the well."));
        assert!(
            !card.contains("draw_water"),
            "the card must not hand the model a copyable id:\n{card}"
        );
    }

    #[test]
    fn card_does_not_carry_the_goal() {
        let mut w = World::founding(1);
        let goal = "Secure winter grain";
        w.agent_mut("Ada").unwrap().current_goal = goal.into();
        let a = w.agent("Ada").unwrap();
        let card = agent_card(&w, a, "speak");
        // Measured: the goal is a short phrase, and the voice model returns
        // its opening word ("balance", "counsel them") instead of a line.
        assert!(
            !card.contains(goal),
            "the goal is still in the card:\n{card}"
        );
        assert!(!card.contains("Your drive"), "{card}");
    }

    #[test]
    fn card_does_not_carry_the_villagers_own_spoken_line() {
        let mut w = World::founding(1);
        let said = "You said: \"The barley is mine to count.\"";
        w.agent_mut("Ada")
            .unwrap()
            .memories
            .push(Memory::new(1, said, 0.9, MemoryKind::Own));
        let a = w.agent("Ada").unwrap();
        let card = agent_card(&w, a, "speak");
        assert!(!card.contains(said), "{card}");
        assert!(!card.contains("The barley is mine to count."), "{card}");
        // Her only memory was the spoken line, so the block has nothing left to
        // print and must not appear as a bare heading.
        assert!(!card.contains("You remember"), "{card}");
    }

    #[test]
    fn card_keeps_witnessed_memories_and_reflections() {
        let mut w = World::founding(1);
        let reflection = "You reflected on your place as steward (skill 3, trust of village 2.0).";
        {
            let a = w.agent_mut("Ada").unwrap();
            a.memories.push(Memory::new(
                0,
                "Odo gave you food.",
                0.75,
                MemoryKind::Witnessed,
            ));
            a.memories
                .push(Memory::new(0, reflection, 0.75, MemoryKind::Reflection));
        }
        let a = w.agent("Ada").unwrap();
        let card = agent_card(&w, a, "speak");
        assert!(card.contains("Odo gave you food."), "{card}");
        assert!(card.contains(reflection), "{card}");
    }

    #[test]
    fn reachable_covers_the_ring() {
        let r = reachable(Place::Granary, 2);
        assert!(r.contains(&Place::Granary));
        assert!(r.contains(&Place::Market));
        assert!(r.contains(&Place::Field));
    }
}
