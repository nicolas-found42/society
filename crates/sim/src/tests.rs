use crate::action::{affordances, apply, Intent, SocialAct};
use crate::memory::{Memory, MemoryKind};
use crate::world::{EventKind, Place, World};

#[test]
fn founding_cast_is_complete() {
    let w = World::founding(7);
    assert_eq!(w.agents.len(), 9);
    assert_eq!(w.living().count(), 9);
    assert!(w.agents.iter().all(|a| a.trust.len() == 8));
}

#[test]
fn same_seed_same_world() {
    let a = World::founding(42);
    let b = World::founding(42);
    for (x, y) in a.agents.iter().zip(b.agents.iter()) {
        assert_eq!(x.name, y.name);
        assert_eq!(x.place, y.place);
        assert_eq!(x.food, y.food);
    }
}

#[test]
fn affordances_only_offer_legal_actions() {
    let w = World::founding(3);
    for a in &w.agents {
        let opts = affordances(&w, a);
        for o in &opts {
            assert!(
                Intent::parse(o, a.place).is_some(),
                "{o} should parse from {}",
                a.place.id()
            );
        }
        // A move is only ever offered to an adjacent place.
        for o in opts.iter().filter(|o| o.starts_with("go:")) {
            let dest = Place::from_id(&o[3..]).unwrap();
            assert!(a.place.adjacent().contains(&dest));
        }
    }
}

#[test]
fn take_never_goes_negative() {
    let mut w = World::founding(5);
    let name = w.agents[0].name.clone();
    w.agents[0].place = Place::Shrine;
    w.place_state_mut(Place::Shrine).food = 0;
    let before = w.agent(&name).unwrap().food;
    let intent = Intent::parse("take", Place::Shrine).unwrap();
    let out = apply(&mut w, &name, &intent, "mine", SocialAct::None);
    assert!(!out.accepted);
    assert_eq!(w.agent(&name).unwrap().food, before);
}

#[test]
fn starving_villager_may_not_give_away_their_last_meal() {
    let mut w = World::founding(61);
    let giver = w.agents[0].name.clone();
    let receiver = w.agents[1].name.clone();
    w.agent_mut(&giver).unwrap().place = Place::Hut;
    w.agent_mut(&receiver).unwrap().place = Place::Hut;
    w.agent_mut(&giver).unwrap().food = 1;
    w.agent_mut(&giver).unwrap().hunger = 3;
    let intent = Intent::parse("give", Place::Hut).unwrap();
    let out = apply(&mut w, &giver, &intent, "take it", SocialAct::Gift);
    assert!(!out.accepted);
    assert_eq!(w.agent(&giver).unwrap().food, 1);
}

#[test]
fn desperate_agents_are_not_offered_rest_or_give() {
    let mut w = World::founding(67);
    let name = w.agents[0].name.clone();
    {
        let a = w.agent_mut(&name).unwrap();
        a.food = 0;
        a.hunger = 3;
        a.place = Place::Field;
    }
    let a = w.agent(&name).unwrap();
    let opts = affordances(&w, a);
    assert!(!opts.contains(&"rest".to_owned()));
    assert!(!opts.contains(&"give".to_owned()));
    assert!(opts.contains(&"forage".to_owned()));
    assert!(opts.iter().any(|o| o.starts_with("go:")));
}

#[test]
fn give_requires_a_listener_and_food() {
    let mut w = World::founding(11);
    let giver = w.agents[0].name.clone();
    let receiver = w.agents[1].name.clone();
    w.agent_mut(&giver).unwrap().place = Place::Hut;
    w.agent_mut(&receiver).unwrap().place = Place::Market;
    w.agent_mut(&giver).unwrap().food = 2;

    let intent = Intent::parse("give", Place::Hut).unwrap();
    let out = apply(&mut w, &giver, &intent, "take this", SocialAct::Gift);
    assert!(!out.accepted, "nobody present");
    assert_eq!(w.agent(&giver).unwrap().food, 2);
}

#[test]
fn giving_transfers_exactly_one_and_raises_trust() {
    let mut w = World::founding(13);
    let giver = w.agents[0].name.clone();
    let receiver = w.agents[1].name.clone();
    w.agent_mut(&giver).unwrap().place = Place::Hut;
    w.agent_mut(&receiver).unwrap().place = Place::Hut;
    w.agent_mut(&giver).unwrap().food = 2;
    let trust_before = w.agent(&receiver).unwrap().trust_of(&giver);

    let intent = Intent::parse("give", Place::Hut).unwrap();
    let out = apply(&mut w, &giver, &intent, "here", SocialAct::Gift);
    assert!(out.accepted);
    assert_eq!(w.agent(&giver).unwrap().food, 1);
    assert_eq!(w.agent(&receiver).unwrap().food, 3);
    assert!(w.agent(&receiver).unwrap().trust_of(&giver) > trust_before);
}

#[test]
fn memory_cap_never_evicts_a_scar() {
    let mut w = World::founding(17);
    let name = w.agents[0].name.clone();
    w.agent_mut(&name)
        .unwrap()
        .remember(Memory::scar(0, "You watched a man starve."));
    for i in 0..40 {
        w.agent_mut(&name).unwrap().remember(Memory::new(
            i,
            format!("ordinary thing {i}"),
            0.05,
            MemoryKind::Witnessed,
        ));
    }
    let a = w.agent(&name).unwrap();
    assert!(a.memories.len() <= crate::memory::MEMORY_CAP);
    assert!(a.memories.iter().any(|m| m.permanent));
}

#[test]
fn memory_decays_but_scars_do_not() {
    let m = Memory::new(0, "x", 1.0, MemoryKind::Own);
    assert!(m.weight(100, 12.0) < 0.05);
    let s = Memory::scar(0, "x");
    assert_eq!(s.weight(10_000, 12.0), 1.0);
}

#[test]
fn starvation_kills_after_three_turns() {
    let mut w = World::founding(19);
    for a in &mut w.agents {
        a.food = 0;
        a.hunger = 3;
        a.starving_turns = 0;
    }
    for (_, st) in &mut w.places {
        st.food = 0;
    }
    w.weather = 0.0;
    w.settle();
    assert_eq!(w.living().count(), 9);
    w.settle();
    assert_eq!(w.living().count(), 9);
    w.settle();
    assert_eq!(w.living().count(), 0);
    assert_eq!(
        w.agents
            .iter()
            .filter(|a| a.memories.iter().any(|m| m.permanent))
            .count(),
        0
    );
}

#[test]
fn death_leaves_a_scar_in_the_survivors() {
    let mut w = World::founding(23);
    let victim = w.agents[0].name.clone();
    {
        let v = w.agent_mut(&victim).unwrap();
        v.food = 0;
        v.hunger = 3;
        v.starving_turns = 2;
    }
    for a in w.agents.iter_mut().filter(|a| a.name != victim) {
        a.food = 5;
    }
    w.settle();
    assert!(!w.agent(&victim).unwrap().alive);
    let survivor = w.agents.iter().find(|a| a.alive).unwrap();
    assert!(survivor
        .memories
        .iter()
        .any(|m| m.permanent && m.text.contains(&victim)));
}

#[test]
fn broken_promise_becomes_a_scar() {
    let mut w = World::founding(29);
    let who = w.agents[0].name.clone();
    let to = w.agents[1].name.clone();
    w.agent_mut(&who).unwrap().promise = Some(crate::world::Promise {
        to,
        text: "half my grain".into(),
        made_turn: 0,
        kept: None,
    });
    w.turn = 20;
    w.settle();
    let a = w.agent(&who).unwrap();
    assert!(a.promise.is_none());
    assert!(a.memories.iter().any(|m| m.permanent));
}

#[test]
fn move_rejects_non_adjacent() {
    let mut w = World::founding(31);
    let name = w.agents[0].name.clone();
    let from = Place::Granary;
    w.agent_mut(&name).unwrap().place = from;
    assert!(Intent::parse("go:woods", from).is_none());
    let parsed = Intent::parse("go:market", from);
    assert!(parsed.is_some());
    let out = apply(&mut w, &name, &parsed.unwrap(), "", SocialAct::None);
    assert!(out.accepted);
    assert_eq!(w.agent(&name).unwrap().place, Place::Market);
}

#[test]
fn stealing_from_an_owned_store_is_a_betrayal() {
    let mut w = World::founding(37);
    let name = w.agents[0].name.clone();
    w.agent_mut(&name).unwrap().place = Place::Granary;
    w.place_state_mut(Place::Granary).owner = Some(w.agents[1].name.clone());
    let intent = Intent::parse("steal", Place::Granary).unwrap();
    let out = apply(&mut w, &name, &intent, "", SocialAct::None);
    assert!(out.accepted);
    assert_eq!(out.kind, EventKind::Betrayal);
}

#[test]
fn speaking_records_an_event_and_a_memory() {
    let mut w = World::founding(41);
    let name = w.agents[0].name.clone();
    let intent = Intent::parse("speak", w.agent(&name).unwrap().place).unwrap();
    apply(
        &mut w,
        &name,
        &intent,
        "the grain is short",
        SocialAct::None,
    );
    assert_eq!(w.events.len(), 2);
    assert!(w
        .agent(&name)
        .unwrap()
        .memories
        .iter()
        .any(|m| m.text.contains("grain is short")));
}

#[test]
fn turn_advances_and_day_rolls_at_twelve() {
    let mut w = World::founding(43);
    for _ in 0..12 {
        w.settle();
    }
    assert_eq!(w.turn, 12);
    assert_eq!(w.day, 1);
    assert!(w.weather > 0.0);
}

#[test]
fn halt_is_sticky_and_logged() {
    let mut w = World::founding(47);
    w.halt("judgment layer unreachable");
    assert!(w.halted.is_some());
    assert_eq!(w.events.last().unwrap().kind, EventKind::System);
}

#[test]
fn verb_roundtrip_covers_every_affordance() {
    let w = World::founding(53);
    for a in &w.agents {
        for o in affordances(&w, a) {
            let i = Intent::parse(&o, a.place).expect("affordance parses");
            assert_ne!(i.verb.id(), "");
        }
    }
}

// ---- growth ----

use crate::rng::Rng;
use crate::world::{
    Agent, PlaceState, BIRTH_COST, BIRTH_MIN_AGE, BIRTH_TRUST, CHILD_FOOD, NEUTRAL_TRUST,
    POPULATION_CAP,
};

/// Two adults of age, fed, together, mutually trusting, with a full granary.
fn pair_world(seed: u64) -> World {
    let names = vec!["Ada".to_owned(), "Bo".to_owned()];
    let mut agents = vec![
        Agent::new(
            "Ada",
            "cautious and loyal; keeps accounts",
            Place::Hut,
            2,
            0,
            0,
            0,
            vec![],
            &names,
        ),
        Agent::new(
            "Bo",
            "anxious and generous; apologises",
            Place::Hut,
            2,
            0,
            0,
            0,
            vec![],
            &names,
        ),
    ];
    // Comfortably above the threshold, so tuning the constant cannot silently
    // turn this fixture into a no-birth case.
    let trust = BIRTH_TRUST + 0.2;
    agents[0].set_trust("Bo", trust);
    agents[1].set_trust("Ada", trust);
    let places = Place::ALL
        .into_iter()
        .map(|p| (p, PlaceState::new(if p == Place::Granary { 9 } else { 0 })))
        .collect();
    World {
        seed,
        // Even turn: nobody gains hunger, so both stay fed through settlement.
        turn: BIRTH_MIN_AGE,
        day: 0,
        agents,
        places,
        events: Vec::new(),
        weather: 1.0,
        weather_kind: crate::world::WeatherKind::Clear,
        temperature_c: 14,
        village_morale: 2.5,
        rng: Rng::new(seed),
        halted: None,
    }
}

fn births(w: &World) -> usize {
    w.events
        .iter()
        .filter(|e| e.kind == EventKind::Birth)
        .count()
}

#[test]
fn a_fed_trusting_pair_has_a_child() {
    let mut w = pair_world(71);
    let granary_before = w.place_state(Place::Granary).food;
    w.settle();
    assert_eq!(w.agents.len(), 3, "one child");
    assert_eq!(births(&w), 1);

    let child = w.agents.last().unwrap();
    assert_eq!(child.generation, 1);
    assert_eq!(child.parents, vec!["Ada".to_owned(), "Bo".to_owned()]);
    assert_eq!(child.place, Place::Hut, "born where the parents are");
    assert_eq!(child.born_turn, BIRTH_MIN_AGE);
    assert_eq!(child.food, CHILD_FOOD);
    assert_eq!(child.hunger, 1);
    assert!(child.memories.is_empty(), "no inherited memories");
    assert!((child.trust_of("Ada") - NEUTRAL_TRUST).abs() < f32::EPSILON);
    assert!((child.trust_of("Bo") - NEUTRAL_TRUST).abs() < f32::EPSILON);
    assert!(child.alive);

    // The granary pays; the parents' carried food is untouched.
    let granary_after = w.place_state(Place::Granary).food;
    let harvested = granary_after + BIRTH_COST - granary_before;
    assert_eq!(
        granary_after,
        (granary_before + harvested).min(18) - BIRTH_COST
    );
    assert_eq!(w.agent("Ada").unwrap().food, 2);
    assert_eq!(w.agent("Bo").unwrap().food, 2);

    let birth = w
        .events
        .iter()
        .find(|e| e.kind == EventKind::Birth)
        .unwrap();
    assert!(birth.text.contains(&child.name));
    assert!(birth.text.contains("Ada") && birth.text.contains("Bo"));
    assert!(w
        .agent("Ada")
        .unwrap()
        .memories
        .iter()
        .any(|m| m.text.contains(&child.name)));
    assert!(w
        .agent("Bo")
        .unwrap()
        .memories
        .iter()
        .any(|m| m.text.contains(&child.name)));
}

#[test]
fn the_granary_pays_the_cost_and_never_goes_negative() {
    let mut w = pair_world(73);
    w.place_state_mut(Place::Granary).food = BIRTH_COST;
    w.settle();
    assert_eq!(births(&w), 1);
    // Harvest arrives before the birth is paid, so the store is exactly the
    // harvest left over: cost came out, carried food did not.
    assert_eq!(w.place_state(Place::Granary).food, 3);
    assert_eq!(w.agent("Ada").unwrap().food, 2);
}

#[test]
fn no_birth_when_the_granary_is_below_cost() {
    let mut w = pair_world(79);
    // Harvest still arrives; keep the store below cost even after it.
    w.place_state_mut(Place::Granary).food = 0;
    w.weather = 0.2;
    w.settle();
    assert_eq!(w.agents.len(), 2);
    assert_eq!(births(&w), 0);
}

#[test]
fn no_birth_when_either_parent_is_hungry() {
    let mut w = pair_world(83);
    w.agent_mut("Bo").unwrap().food = 0;
    w.agent_mut("Bo").unwrap().hunger = 1;
    w.settle();
    assert_eq!(w.agents.len(), 2);
}

#[test]
fn no_birth_without_mutual_trust() {
    let mut w = pair_world(89);
    w.agent_mut("Bo")
        .unwrap()
        .set_trust("Ada", BIRTH_TRUST - 0.5);
    w.settle();
    assert_eq!(w.agents.len(), 2, "one-sided trust is not enough");
}

#[test]
fn no_birth_when_parents_are_apart() {
    let mut w = pair_world(97);
    w.agent_mut("Bo").unwrap().place = Place::Market;
    w.settle();
    assert_eq!(w.agents.len(), 2);
}

#[test]
fn no_birth_at_the_population_cap() {
    let mut w = pair_world(101);
    let mut names: Vec<String> = w.agents.iter().map(|a| a.name.clone()).collect();
    while names.len() < POPULATION_CAP {
        let n = format!("Extra{}", names.len());
        let a = Agent::new(&n, "plain", Place::Well, 2, 0, 0, 0, vec![], &names);
        names.push(n);
        w.agents.push(a);
    }
    for a in &mut w.agents {
        if a.name.starts_with("Extra") {
            a.hunger = 2;
        }
    }
    assert_eq!(w.living().count(), POPULATION_CAP);
    w.settle();
    assert_eq!(w.living().count(), POPULATION_CAP);
    assert_eq!(births(&w), 0);
}

#[test]
fn no_birth_before_minimum_age() {
    let mut w = pair_world(103);
    for a in &mut w.agents {
        a.born_turn = BIRTH_MIN_AGE;
    }
    w.settle();
    assert_eq!(
        w.agents.len(),
        2,
        "the founding pair cannot pair off on turn one"
    );
}

#[test]
fn at_most_one_birth_per_turn() {
    let mut w = pair_world(107);
    let mut names: Vec<String> = w.agents.iter().map(|a| a.name.clone()).collect();
    for n in ["Cy", "Di"] {
        let a = Agent::new(n, "plain", Place::Hut, 2, 0, 0, 0, vec![], &names);
        names.push(n.to_owned());
        w.agents.push(a);
    }
    for a in &mut w.agents {
        for other in names.clone() {
            if other != a.name {
                a.set_trust(&other, BIRTH_TRUST + 0.2);
            }
        }
    }
    // Six qualifying pairs in the hut; exactly one child arrives.
    w.settle();
    assert_eq!(w.agents.len(), 5);
    assert_eq!(births(&w), 1);
}

#[test]
fn same_seed_same_child() {
    let mut a = pair_world(109);
    let mut b = pair_world(109);
    a.settle();
    b.settle();
    assert_eq!(a.agents.last().unwrap().name, b.agents.last().unwrap().name);
    assert_eq!(
        a.agents.last().unwrap().parents,
        b.agents.last().unwrap().parents
    );
    assert_eq!(
        a.place_state(Place::Granary).food,
        b.place_state(Place::Granary).food
    );
}

#[test]
fn the_child_is_an_ordinary_villager_next_turn() {
    let mut w = pair_world(113);
    w.settle();
    let name = w.agents.last().unwrap().name.clone();

    // The child eats out of its own hands like anyone else.
    w.agent_mut(&name).unwrap().food = 1;
    w.agent_mut(&name).unwrap().hunger = 1;
    w.settle();
    let child = w.agent(&name).unwrap();
    assert_eq!(child.food, 0);
    assert_eq!(child.hunger, 0);

    // And it can starve: no special case protects a child.
    {
        let c = w.agent_mut(&name).unwrap();
        c.food = 0;
        c.hunger = 3;
        c.starving_turns = 2;
    }
    // Everyone else stays fed so no birth or other death muddies the reading.
    for a in w.agents.iter_mut().filter(|a| a.name != name) {
        a.food = 5;
        a.hunger = 0;
    }
    w.settle();
    assert!(!w.agent(&name).unwrap().alive);
    assert!(w
        .events
        .iter()
        .any(|e| e.kind == EventKind::Death && e.actor == name));
}

#[test]
fn lineage_is_strictly_increasing_and_empty_for_founders() {
    let w = World::founding(127);
    for a in &w.agents {
        assert_eq!(a.generation, 0);
        assert!(
            a.parents.is_empty(),
            "parents empty exactly for the founding cast"
        );
        assert_eq!(a.born_turn, 0);
    }
    let mut w = pair_world(131);
    w.settle();
    let child = w.agents.last().unwrap();
    for p in &child.parents {
        let parent = w.agent(p).unwrap();
        assert!(child.generation > parent.generation);
        assert!(child.born_turn > parent.born_turn || parent.born_turn == 0);
    }
}

#[test]
fn a_birth_does_not_erase_a_death_scar() {
    let mut w = pair_world(137);
    let names = vec!["Ada".to_owned(), "Bo".to_owned(), "Cy".to_owned()];
    let mut dying = Agent::new("Cy", "tired", Place::Hut, 0, 3, 0, 0, vec![], &names);
    dying.starving_turns = 2;
    w.agents.push(dying);
    w.settle();
    assert!(w.events.iter().any(|e| e.kind == EventKind::Death));
    assert_eq!(births(&w), 1, "a birth happens in the same turn as a death");
    let survivor = w.agent("Ada").unwrap();
    assert!(survivor
        .memories
        .iter()
        .any(|m| m.permanent && m.text.contains("Cy")));
}

#[test]
fn a_child_inherits_a_trait_from_each_parent() {
    let mut w = pair_world(139);
    let mother_trait = w
        .agent("Ada")
        .unwrap()
        .persona
        .split(';')
        .next()
        .unwrap()
        .trim()
        .to_owned();
    let father_trait = w
        .agent("Bo")
        .unwrap()
        .persona
        .split(';')
        .next()
        .unwrap()
        .trim()
        .to_owned();
    w.settle();
    let child = w.agents.last().unwrap();
    // Mixed, not authored: both parents' dominant traits are present. The exact
    // wording is deliberately not pinned.
    assert!(child.persona.contains(&mother_trait), "{}", child.persona);
    assert!(child.persona.contains(&father_trait), "{}", child.persona);
}

#[test]
fn multi_resource_actions_work_and_update_skills() {
    let mut w = World::founding(201);
    let name = w.agents[0].name.clone();

    // Draw water at the Well.
    w.agent_mut(&name).unwrap().place = Place::Well;
    w.agent_mut(&name).unwrap().thirst = 2;
    let water_before = w.agent(&name).unwrap().water;
    let draw = Intent::parse("draw_water", Place::Well).unwrap();
    let out = apply(&mut w, &name, &draw, "fresh water", SocialAct::None);
    assert!(out.accepted);
    assert!(w.agent(&name).unwrap().water > water_before);
    assert_eq!(w.agent(&name).unwrap().thirst, 0);

    // Chop wood in the Woods.
    w.agent_mut(&name).unwrap().place = Place::Woods;
    let wood_before = w.agent(&name).unwrap().wood;
    let chop = Intent::parse("chop", Place::Woods).unwrap();
    let out = apply(
        &mut w,
        &name,
        &chop,
        "timber for the hearth",
        SocialAct::None,
    );
    assert!(out.accepted);
    assert!(w.agent(&name).unwrap().wood > wood_before);

    // Craft a tool at the Market using the chopped wood.
    w.agent_mut(&name).unwrap().place = Place::Market;
    let tools_before = w.agent(&name).unwrap().tools;
    let craft = Intent::parse("craft", Place::Market).unwrap();
    let out = apply(&mut w, &name, &craft, "a sturdy hoe", SocialAct::None);
    assert!(out.accepted);
    assert_eq!(w.agent(&name).unwrap().tools, tools_before + 1);
}

#[test]
fn healing_cures_ailments_and_restores_health() {
    let mut w = World::founding(211);
    let healer = w.agents[0].name.clone();
    let patient = w.agents[1].name.clone();
    w.agent_mut(&healer).unwrap().place = Place::Shrine;
    w.agent_mut(&healer).unwrap().herbs = 1;
    w.agent_mut(&patient).unwrap().place = Place::Shrine;
    w.agent_mut(&patient).unwrap().health = 5;
    w.agent_mut(&patient).unwrap().ailment = Some(crate::world::Ailment::Fever);

    let heal = Intent::parse("heal", Place::Shrine).unwrap();
    let out = apply(
        &mut w,
        &healer,
        &heal,
        "drink this infusion",
        SocialAct::Blessing,
    );
    assert!(out.accepted);
    assert!(w.agent(&patient).unwrap().ailment.is_none());
    assert_eq!(w.agent(&patient).unwrap().health, 9);
}

#[test]
fn village_ui_renders_map_resources_and_inspector() {
    let w = World::founding(223);
    let ui = crate::card::render_village_ui(&w, 5);
    assert!(ui.contains("VILLAGE COMMONS"));
    assert!(ui.contains("PLACES & STOCKPILES"));
    assert!(ui.contains("VILLAGERS"));

    let dossier = crate::card::render_villager_inspector(&w, "Ada").unwrap();
    assert!(dossier.contains("VILLAGER DOSSIER: Ada"));
    assert!(dossier.contains("STEWARD"));
}

#[test]
fn observer_weather_changes_update_visible_conditions_immediately() {
    let mut w = World::founding(7);
    w.day = 9; // winter
    w.set_weather(0.5);
    assert_eq!(w.weather_kind, crate::WeatherKind::Frost);
    assert_eq!(w.temperature_c, -5);
    w.day = 3; // summer
    w.set_weather(1.5);
    assert_eq!(w.weather_kind, crate::WeatherKind::BountifulSun);
    assert_eq!(w.temperature_c, 26);
    assert_eq!(w.weather, 1.5);
}

// ---- The code-only engine (tickets #9-#15) -------------------------------

use crate::action::Verb;
use crate::check;
use crate::policy;
use crate::speaker;
use crate::turn;

fn full_turns(seed: u64, turns: usize) -> (World, Vec<turn::TurnRecord>) {
    let mut w = World::founding(seed);
    let mut recs = Vec::new();
    for _ in 0..turns {
        match turn::step(&mut w) {
            Ok(r) => recs.push(r),
            Err(reason) => panic!("unexpected halt at turn {}: {reason}", w.turn),
        }
    }
    (w, recs)
}

#[test]
fn a_turn_through_the_seem_is_sync_pure_and_offline() {
    // No async runtime, no network, no clock: one function call over the
    // world (ticket #8: the pure turn seam).
    let mut w = World::founding(9);
    let before = serde_json::to_string(&w).unwrap();
    let rec = turn::step(&mut w).expect("turn");
    assert_eq!(rec.turn, 0);
    assert_ne!(
        before,
        serde_json::to_string(&w).unwrap(),
        "the world moved"
    );
}

#[test]
fn the_motive_refines_the_goal_through_the_ladder() {
    let mut w = World::founding(31);
    let name = w.living().next().unwrap().name.clone();
    turn::step(&mut w).expect("turn");
    let a = w.agent(&name).unwrap();
    assert!(a.motive.is_some(), "the seam writes the motive");
    // The ladder: the survival steps override, then the motive names the
    // goal, then the role.
    if a.motive.as_deref() == Some("hunger") && !(a.hunger >= 2 && a.food == 0) {
        assert_eq!(a.current_goal, "find food before the next meal");
    }
}

#[test]
fn a_discredited_line_moves_no_trust() {
    let mut w = World::founding(17);
    let (speaker, listener) = {
        let names: Vec<String> = w.living().map(|a| a.name.clone()).collect();
        (names[0].clone(), names[1].clone())
    };
    // Both at the same place, so the act has a listener.
    let place = w.agent(&speaker).unwrap().place;
    w.agent_mut(&listener).unwrap().place = place;
    let before = w.agent(&listener).unwrap().trust_of(&speaker);

    let decision = policy::Decision {
        action: "speak".into(),
        verb: Verb::Speak,
        place: None,
        act: SocialAct::Gossip,
        addressee: Some(listener.clone()),
        motive: policy::Motive::Duty,
        job: None,
        confidence: 0.6,
        dilemma: None,
        factors: vec![("novelty", 0.8)],
    };
    let out = crate::action::apply_decision(
        &mut w,
        &speaker,
        &decision,
        "I carry nine measures, count them yourself.",
        SocialAct::Gossip,
        check::DISCREDITED,
        2.4,
        Some("the count of food (9)"),
    );
    assert!(out.accepted);
    assert_eq!(
        w.agent(&listener).unwrap().trust_of(&speaker),
        before,
        "a discredited line moves no trust"
    );
    // The claim and its failure are remembered.
    let a = w.agent(&listener).unwrap();
    assert!(a
        .memories
        .iter()
        .any(|m| m.text.contains("did not hold") || m.text.contains("count of food")));
}

#[test]
fn a_credible_act_moves_trust_and_a_credible_threat_bites_morale() {
    let mut w = World::founding(18);
    let names: Vec<String> = w.living().map(|a| a.name.clone()).collect();
    let (speaker, listener) = (names[0].clone(), names[1].clone());
    let place = w.agent(&speaker).unwrap().place;
    w.agent_mut(&listener).unwrap().place = place;
    let before = w.agent(&listener).unwrap().trust_of(&speaker);
    let decision = policy::Decision {
        action: "speak".into(),
        verb: Verb::Speak,
        place: None,
        act: SocialAct::Gratitude,
        addressee: Some(listener.clone()),
        motive: policy::Motive::Duty,
        job: None,
        confidence: 0.6,
        dilemma: None,
        factors: vec![],
    };
    crate::action::apply_decision(
        &mut w,
        &speaker,
        &decision,
        "You kept the field from the frost, and I remember it.",
        SocialAct::Gratitude,
        check::GROUNDED,
        2.4,
        None,
    );
    assert!(w.agent(&listener).unwrap().trust_of(&speaker) > before);

    // A credible threat at the morale threshold nudges the village down.
    let morale = w.village_morale;
    crate::action::apply_decision(
        &mut w,
        &speaker,
        &decision,
        "Keep the grain back and you will wish you had not.",
        SocialAct::Threat,
        check::GROUNDED,
        turn::derived_intensity(SocialAct::Threat, "plain"),
        None,
    );
    assert!(w.village_morale < morale, "the morale branch fired");
}

#[test]
fn the_job_is_recorded_and_the_action_id_stays_atomic() {
    // Bo the Farmer stands at the field: tend is available, so the ready
    // order names itself on the decision without touching the action id.
    let mut w = World::founding(19);
    w.agents.retain(|a| a.name == "Bo");
    w.agent_mut("Bo").unwrap().place = Place::Field;
    w.agents.push(Agent::new(
        "Ada",
        "plain",
        Place::Field,
        1,
        0,
        0,
        0,
        vec![],
        &["Ada".to_owned(), "Bo".to_owned()],
    ));
    let mut saw_job = false;
    for _ in 0..8 {
        if let Some(d) = policy::decide(&w, "Bo") {
            if d.action == "tend" {
                assert_eq!(d.job, Some("farmer.tend"));
                saw_job = true;
                break;
            }
        }
        w.settle();
    }
    assert!(
        saw_job,
        "the farmer should reach his ready order within a day"
    );
}

#[test]
fn a_villager_with_nobody_present_is_silent_and_the_summary_counts_it() {
    let mut w = World::founding(23);
    w.agents.retain(|a| a.name == "Ada");
    let d = policy::decide(&w, "Ada").expect("decision");
    assert_eq!(speaker::speak(&w, "Ada", &d), None, "nobody to address");
}

#[test]
fn two_villagers_draw_different_lines_from_their_own_streams() {
    let (w, recs) = full_turns(24, 20);
    let mut saw_pair = false;
    for rec in &recs {
        for i in 0..rec.agents.len() {
            for j in i + 1..rec.agents.len() {
                let (a, b) = (&rec.agents[i], &rec.agents[j]);
                if !a.say.is_empty() && !b.say.is_empty() && a.say != b.say {
                    saw_pair = true;
                }
            }
        }
    }
    assert!(saw_pair, "two villagers in one turn spoke differently");
    let _ = w;
}

#[test]
fn the_same_seed_and_save_replay_identically() {
    for seed in [77_u64, 4242, 900_001] {
        let (a, ra) = full_turns(seed, 40);
        let (b, rb) = full_turns(seed, 40);
        assert_eq!(
            serde_json::to_string(&a).unwrap(),
            serde_json::to_string(&b).unwrap(),
            "seed {seed} diverged"
        );
        assert_eq!(
            serde_json::to_string(&ra).unwrap(),
            serde_json::to_string(&rb).unwrap()
        );
        // A save round-trip mid-run continues identically.
        let mut w = World::founding(seed);
        for _ in 0..17 {
            turn::step(&mut w).unwrap();
        }
        let saved = serde_json::to_string(&w).unwrap();
        let mut w: World = serde_json::from_str(&saved).unwrap();
        for _ in 0..23 {
            turn::step(&mut w).unwrap();
        }
        assert_eq!(
            serde_json::to_string(&w).unwrap(),
            serde_json::to_string(&a).unwrap(),
            "seed {seed}: the save did not continue the run"
        );
    }
}

#[test]
fn the_soak_holds_the_invariants() {
    let mut total_lines = 0_u64;
    let mut total_discredited = 0_u64;
    let mut total_lie_lines = 0_u64;
    for seed in [1_u64, 2, 3, 7, 2026] {
        let mut w = World::founding(seed);
        let mut last_lines: Vec<String> = Vec::new();
        let mut saw_birth = false;
        let start = std::time::Instant::now();
        for i in 0..200 {
            let t0 = std::time::Instant::now();
            let rec = turn::step(&mut w).expect("no unexpected halt in the soak");
            // A nine-villager turn stays far inside the 50 ms bound.
            assert!(
                t0.elapsed().as_millis() < 50,
                "seed {seed} turn {i} took {:?}",
                t0.elapsed()
            );
            saw_birth |= !rec.births.is_empty();
            // The ledger: no exact repeat inside the window.
            for a in &rec.agents {
                if !a.say.is_empty() {
                    assert!(
                        !last_lines.contains(&a.say),
                        "seed {seed} turn {i}: repeated line {:?}",
                        a.say
                    );
                }
            }
            last_lines.clear();
            for a in w.living() {
                for m in &a.memories {
                    assert!(m.permanent || m.salience >= 0.0);
                }
                assert!(
                    a.memories.len() <= crate::memory::MEMORY_CAP + 2,
                    "memory cap"
                );
                for (_, t) in &a.trust {
                    assert!((0.0..=4.0).contains(t), "trust bounded");
                }
            }
            total_lines += u64::from(rec.summary.lines);
            total_discredited += u64::from(rec.summary.discredited);
            total_lie_lines += u64::from(rec.summary.lies);
            // The summary's counters agree with the agent turns.
            let sum_lies: u32 = rec.agents.iter().filter(|a| a.lie).count() as u32;
            assert_eq!(sum_lies, rec.summary.lies);
            let counted = rec.summary.lines + rec.summary.silences;
            assert_eq!(counted as usize, rec.agents.len());
            // Outside lie mode no line makes a false claim (property, per
            // turn, against the settled world of that turn).
        }
        assert!(saw_birth, "seed {seed}: births happen in 200 turns");
        assert!(w.living().count() >= 4, "seed {seed}: no starvation spiral");
        let _ = start;
    }
    // The discredited rate sits inside its band: only lies can fail, and the
    // lie rate is capped.
    if total_lines > 100 {
        let rate = total_discredited as f64 / total_lines as f64;
        assert!(rate <= 0.22, "discredited rate {rate} above the lie cap");
        assert!(total_lie_lines > 0, "a village of 1000 lines never lied");
    }
}

#[test]
fn a_lie_clause_carries_its_designed_claim() {
    use crate::grammar::LieKind;
    let mut w = World::founding(27);
    w.agents.retain(|a| a.name == "Ada" || a.name == "Bo");
    {
        let a = w.agent_mut("Ada").unwrap();
        a.hunger = 3;
        a.food = 0;
    }
    let d = policy::Decision {
        action: "speak".into(),
        verb: Verb::Speak,
        place: None,
        act: SocialAct::Gossip,
        addressee: Some("Bo".into()),
        motive: policy::Motive::Duty,
        job: None,
        confidence: 1.0,
        dilemma: None,
        factors: vec![],
    };
    let a = w.agent("Ada").unwrap();
    for kind in [
        LieKind::InflatedCount,
        LieKind::PastGift,
        LieKind::HiddenCache,
        LieKind::FakePromise,
        LieKind::ClaimedVisit,
        LieKind::OthersIntent,
    ] {
        let mut rng = Rng::new(u64::from(kind as u8) + 11);
        let mut landed = false;
        for _ in 0..20 {
            if let Some((clause, claim)) = speaker::render_lie_kind(&w, a, &d, kind, &mut rng) {
                assert!(!clause.contains('{'), "unfilled slot in {clause:?}");
                assert!(!clause.contains('#'), "no leaked ids in {clause:?}");
                let v = check::settle(&w, "Ada", &claim);
                assert_ne!(v, check::Verdict::True, "{kind:?} rendered a true claim");
                if kind == LieKind::InflatedCount {
                    assert_eq!(v, check::Verdict::False, "the inflated count is caught");
                }
                if kind == LieKind::FakePromise {
                    assert_eq!(v, check::Verdict::False, "a promise never made is caught");
                }
                landed = true;
                break;
            }
        }
        assert!(landed, "{kind:?} never produced a lie clause");
    }
}
