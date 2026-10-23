//! The observer payload: the village, written once.
//!
//! The control API serves the whole village on every `/state` request, and a
//! village that has lived a while carries a lot of memory: at turn 233 the
//! twelve villagers held 18,279 facts between them, 3.4 MB of JSON.
//!
//! Building that payload as a `serde_json::Value` tree costs about twelve
//! times the payload in allocations - every fact becomes a map with owned
//! keys - and the tree is then walked again to write the bytes. This module
//! writes the bytes straight from the world instead: the payload is a set of
//! `Serialize` types, and an agent's record is the agent's own serialization
//! with the observer's two rounded fields folded in, so a field added to the
//! world is carried without anyone remembering to carry it.
//!
//! One thing changes on the wire, and it changes for the better: a `f32` in
//! the world used to be widened to `f64` on its way through a `Value`, so a
//! trust of 3.2 was published as `3.200000047683716`. Written directly, it is
//! `3.2`, which is what the rounding was for in the first place.

use std::cell::Cell;

use serde::ser::{SerializeStruct, Serializer};
use serde::Serialize;
use serde_json::Value;
use society_sim::{Agent, Place, PlaceState, Role, World};

thread_local! {
    /// The size of the last payload written on this thread. A buffer that
    /// knows its size is written once; one that does not is written into a
    /// series of ever-larger buffers, and every one of those is a copy.
    static LAST_BYTES: Cell<usize> = const { Cell::new(0) };
}

/// The whole village, as the observer API serves it.
#[derive(Debug, Serialize)]
pub struct Snapshot<'a> {
    turn: u64,
    day: u64,
    weather: f32,
    weather_kind: &'static str,
    season: &'static str,
    time_of_day: &'static str,
    temperature_c: i32,
    village_morale: f32,
    halted: Option<&'a str>,
    agents: Vec<AgentView<'a>>,
    places: Vec<PlaceView<'a, &'a str>>,
    social: SocialFabric<'a>,
    event_count: usize,
}

/// The village, as the observer API serves it.
///
/// # Panics
///
/// Panics if the village cannot be serialized, which cannot happen for the
/// plain-data state it holds.
#[must_use]
pub fn snapshot(w: &World) -> Snapshot<'_> {
    let living: Vec<&Agent> = w.living().collect();
    Snapshot {
        turn: w.turn,
        day: w.day,
        weather: w.weather,
        weather_kind: w.weather_kind.id(),
        season: w.season().id(),
        time_of_day: w.time_of_day().id(),
        temperature_c: w.temperature_c,
        village_morale: w.village_morale,
        halted: w.halted.as_deref(),
        agents: w.agents.iter().map(AgentView::living).collect(),
        places: w
            .places
            .iter()
            .map(|(place, state)| {
                place_view(
                    state,
                    *place,
                    living
                        .iter()
                        .filter(|a| a.place == *place)
                        .map(|a| a.name.as_str())
                        .collect(),
                    recent_place_dialogue(w, *place, 4),
                )
            })
            .collect(),
        social: social_fabric(w),
        event_count: w.events.len(),
    }
}

/// The payload as the bytes that go on the wire.
///
/// The buffer is sized from the last payload written on this thread, so the
/// village is written once into a buffer that already fits it, rather than
/// into a series of ever-larger ones.
///
/// # Panics
///
/// Panics if the village cannot be serialized, which cannot happen for the
/// plain-data state it holds.
#[must_use]
pub fn encode(w: &World) -> Vec<u8> {
    let hint = LAST_BYTES.with(Cell::get);
    let mut bytes = Vec::with_capacity(hint.max(64 * 1024));
    serde_json::to_writer(&mut bytes, &snapshot(w)).expect("the snapshot serialises");
    LAST_BYTES.with(|last| last.set(bytes.len()));
    bytes
}

/// The fields the living village does not carry. A villager's claims about
/// the world are the whole of what they have learned, and they grow with every
/// day the village lives; the village itself does not need them, and one
/// villager's claims are served by `/agent/{name}`.
const NOT_IN_THE_LIVING_VILLAGE: &[&str] = &["facts"];

/// A villager, as the observer API shows them.
///
/// There are two views of a villager, and the API serves both:
///
/// * the **living village** (`/state`) carries what the settlement is now:
///   every field of the villager except their claims about the world;
/// * the **whole memory** (`/agent/{name}`) carries everything, claims
///   included.
///
/// In both, the standing and the trust are rounded for display.
#[derive(Debug)]
pub struct AgentView<'a> {
    agent: &'a Agent,
    extra: &'a [(&'static str, Value)],
    skip: &'static [&'static str],
}

impl<'a> AgentView<'a> {
    /// The villager as the living village shows them: the whole record except
    /// their claims about the world.
    #[must_use]
    pub const fn living(agent: &'a Agent) -> Self {
        Self {
            agent,
            extra: &[],
            skip: NOT_IN_THE_LIVING_VILLAGE,
        }
    }

    /// The villager with their whole memory: every field, claims included.
    #[must_use]
    pub const fn complete(agent: &'a Agent) -> Self {
        Self {
            agent,
            extra: &[],
            skip: &[],
        }
    }

    /// The same view, with extra fields appended after the record.
    #[must_use]
    pub const fn with_extra(mut self, extra: &'a [(&'static str, Value)]) -> Self {
        self.extra = extra;
        self
    }
}

impl Serialize for AgentView<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let standing = (self.agent.standing * 10.0).round() / 10.0;
        let trust = self
            .agent
            .trust
            .iter()
            .map(|(name, value)| (name.as_str(), (value * 10.0).round() / 10.0))
            .collect::<Vec<_>>();
        self.agent.serialize(Rounding {
            inner: serializer,
            standing,
            trust,
            extra: self.extra,
            skip: self.skip,
        })
    }
}

/// A place, as the observer API shows it: the place's own record, plus the
/// identifiers and the neighbours that live in the world rather than in the
/// record.
#[derive(Debug, Serialize)]
pub struct PlaceView<'a, P> {
    #[serde(flatten)]
    state: &'a PlaceState,
    id: &'static str,
    adjacent: Vec<&'static str>,
    present: Vec<P>,
    recent_dialogue: Vec<String>,
}

/// A place and its record, as the observer API shows it. `present` is
/// whatever the caller wants at the place: the names of the villagers there,
/// or their full records.
#[must_use]
pub fn place_view<P>(
    state: &PlaceState,
    place: Place,
    present: Vec<P>,
    recent_dialogue: Vec<String>,
) -> PlaceView<'_, P> {
    PlaceView {
        state,
        id: place.id(),
        adjacent: place.adjacent().iter().map(|q| q.id()).collect(),
        present,
        recent_dialogue,
    }
}

/// The village's living social structure: alliances, feuds, lineages, active
/// and overdue oaths, elders, and territorial stewardship.
#[derive(Debug, Serialize)]
pub struct SocialFabric<'a> {
    living_count: usize,
    elders: Vec<&'a str>,
    alliances: Vec<Alliance<'a>>,
    feuds: Vec<Feud<'a>>,
    promises: Vec<PromiseView<'a>>,
    lineages: Vec<Lineage<'a>>,
    stewardship: Vec<Stewardship<'a>>,
}

/// Two villagers who trust each other.
#[derive(Debug, Serialize)]
struct Alliance<'a> {
    members: [&'a str; 2],
    mutual_trust: f32,
}

/// Two villagers who do not.
#[derive(Debug, Serialize)]
struct Feud<'a> {
    members: [&'a str; 2],
    min_trust: f32,
}

/// A promise one villager made to another.
#[derive(Debug, Serialize)]
struct PromiseView<'a> {
    from: &'a str,
    to: &'a str,
    text: &'a str,
    made_turn: u64,
    age_turns: u64,
    overdue: bool,
    kept: Option<bool>,
}

/// A villager's descent.
#[derive(Debug, Serialize)]
struct Lineage<'a> {
    villager: &'a str,
    generation: u8,
    parents: Vec<&'a str>,
}

/// A place held by a villager.
#[derive(Debug, Serialize)]
struct Stewardship<'a> {
    place: &'static str,
    steward: &'a str,
    food: u32,
}

/// Computes the village's living social structure: alliances, feuds,
/// lineages, active/overdue oaths, elders, and territorial stewardship.
#[must_use]
pub fn social_fabric(w: &World) -> SocialFabric<'_> {
    let living: Vec<&Agent> = w.living().collect();
    let mut alliances = Vec::new();
    let mut feuds = Vec::new();

    for (i, a) in living.iter().enumerate() {
        for b in living.iter().skip(i + 1) {
            let ab = a.trust_of(&b.name);
            let ba = b.trust_of(&a.name);
            if ab >= 2.7 && ba >= 2.7 {
                alliances.push(Alliance {
                    members: [a.name.as_str(), b.name.as_str()],
                    mutual_trust: (f32::midpoint(ab, ba) * 10.0).round() / 10.0,
                });
            } else if ab <= 1.0 || ba <= 1.0 {
                feuds.push(Feud {
                    members: [a.name.as_str(), b.name.as_str()],
                    min_trust: (ab.min(ba) * 10.0).round() / 10.0,
                });
            }
        }
    }

    let promises = living
        .iter()
        .filter_map(|a| {
            a.promise.as_ref().map(|p| {
                let age = w.turn.saturating_sub(p.made_turn);
                PromiseView {
                    from: a.name.as_str(),
                    to: p.to.as_str(),
                    text: p.text.as_str(),
                    made_turn: p.made_turn,
                    age_turns: age,
                    overdue: p.kept.is_none() && age >= 8,
                    kept: p.kept,
                }
            })
        })
        .collect();

    let lineages = living
        .iter()
        .filter(|a| !a.parents.is_empty() || a.generation > 0)
        .map(|a| Lineage {
            villager: a.name.as_str(),
            generation: a.generation,
            parents: a.parents.iter().map(String::as_str).collect(),
        })
        .collect();

    let elders = living
        .iter()
        .filter(|a| a.role == Role::Elder)
        .map(|a| a.name.as_str())
        .collect();

    let stewardship = w
        .places
        .iter()
        .filter_map(|(place, state)| {
            state.owner.as_ref().map(|owner| Stewardship {
                place: place.id(),
                steward: owner.as_str(),
                food: state.food,
            })
        })
        .collect();

    SocialFabric {
        living_count: living.len(),
        elders,
        alliances,
        feuds,
        promises,
        lineages,
        stewardship,
    }
}

/// The last few lines spoken at one place, oldest first. A world query, not a
/// simulation fact: it reads the event log the world already carries.
///
/// Provenance is the `spoken` flag on the event (ticket #4): a line enters
/// conversation history iff a villager actually said words there. Deaths,
/// births, foundings, halts, rejections, silent chores, and observer prose
/// (whispers, rumors, feasts, oaths, goals) are narrative context, never
/// dialogue - even with nonempty text at the same place. `spoken` defaults
/// to `false` for pre-flag saves, so old chronicles exclude rather than
/// invent speech.
#[must_use]
pub fn recent_place_dialogue(w: &World, place: Place, limit: usize) -> Vec<String> {
    let mut out: Vec<String> = w
        .events
        .iter()
        .rev()
        .filter(|e| e.place == place && e.spoken && !e.text.trim().is_empty())
        .take(limit)
        .map(|e| format!("Turn {} {}: {}", e.turn, e.actor, e.text))
        .collect();
    out.reverse();
    out
}

/// Writes an agent's record, rounding the standing and the trust on the way
/// out, and appending any extra fields after it.
///
/// The agent's own `Serialize` writes the record; this sits in front of it and
/// replaces two of the fields as they pass. Replacing them here rather than
/// building a `Value` and overwriting two entries is what keeps the payload
/// off the heap: the whole village's memory is written once, as bytes.
struct Rounding<'a, S> {
    inner: S,
    standing: f32,
    trust: Vec<(&'a str, f32)>,
    extra: &'a [(&'static str, Value)],
    skip: &'static [&'static str],
}

/// The struct serializer that does the rounding.
struct RoundingStruct<'a, S> {
    inner: S,
    standing: f32,
    trust: Vec<(&'a str, f32)>,
    extra: &'a [(&'static str, Value)],
    skip: &'static [&'static str],
}

impl<S: SerializeStruct> SerializeStruct for RoundingStruct<'_, S> {
    type Ok = S::Ok;
    type Error = S::Error;

    fn serialize_field<T: ?Sized + Serialize>(
        &mut self,
        key: &'static str,
        value: &T,
    ) -> Result<(), Self::Error> {
        if self.skip.contains(&key) {
            return Ok(());
        }
        match key {
            "standing" => self.inner.serialize_field(key, &self.standing),
            "trust" => self.inner.serialize_field(key, &self.trust),
            _ => self.inner.serialize_field(key, value),
        }
    }

    fn skip_field(&mut self, key: &'static str) -> Result<(), Self::Error> {
        self.inner.skip_field(key)
    }

    fn end(self) -> Result<Self::Ok, Self::Error> {
        let RoundingStruct {
            mut inner, extra, ..
        } = self;
        for (key, value) in extra {
            inner.serialize_field(key, value)?;
        }
        inner.end()
    }
}

impl<'a, S: Serializer> Serializer for Rounding<'a, S> {
    type Ok = S::Ok;
    type Error = S::Error;
    type SerializeSeq = S::SerializeSeq;
    type SerializeTuple = S::SerializeTuple;
    type SerializeTupleStruct = S::SerializeTupleStruct;
    type SerializeTupleVariant = S::SerializeTupleVariant;
    type SerializeMap = S::SerializeMap;
    type SerializeStruct = RoundingStruct<'a, S::SerializeStruct>;
    type SerializeStructVariant = S::SerializeStructVariant;

    fn serialize_bool(self, value: bool) -> Result<Self::Ok, Self::Error> {
        self.inner.serialize_bool(value)
    }

    fn serialize_i8(self, value: i8) -> Result<Self::Ok, Self::Error> {
        self.inner.serialize_i8(value)
    }

    fn serialize_i16(self, value: i16) -> Result<Self::Ok, Self::Error> {
        self.inner.serialize_i16(value)
    }

    fn serialize_i32(self, value: i32) -> Result<Self::Ok, Self::Error> {
        self.inner.serialize_i32(value)
    }

    fn serialize_i64(self, value: i64) -> Result<Self::Ok, Self::Error> {
        self.inner.serialize_i64(value)
    }

    fn serialize_i128(self, value: i128) -> Result<Self::Ok, Self::Error> {
        self.inner.serialize_i128(value)
    }

    fn serialize_u8(self, value: u8) -> Result<Self::Ok, Self::Error> {
        self.inner.serialize_u8(value)
    }

    fn serialize_u16(self, value: u16) -> Result<Self::Ok, Self::Error> {
        self.inner.serialize_u16(value)
    }

    fn serialize_u32(self, value: u32) -> Result<Self::Ok, Self::Error> {
        self.inner.serialize_u32(value)
    }

    fn serialize_u64(self, value: u64) -> Result<Self::Ok, Self::Error> {
        self.inner.serialize_u64(value)
    }

    fn serialize_u128(self, value: u128) -> Result<Self::Ok, Self::Error> {
        self.inner.serialize_u128(value)
    }

    fn serialize_f32(self, value: f32) -> Result<Self::Ok, Self::Error> {
        self.inner.serialize_f32(value)
    }

    fn serialize_f64(self, value: f64) -> Result<Self::Ok, Self::Error> {
        self.inner.serialize_f64(value)
    }

    fn serialize_char(self, value: char) -> Result<Self::Ok, Self::Error> {
        self.inner.serialize_char(value)
    }

    fn serialize_str(self, value: &str) -> Result<Self::Ok, Self::Error> {
        self.inner.serialize_str(value)
    }

    fn serialize_bytes(self, value: &[u8]) -> Result<Self::Ok, Self::Error> {
        self.inner.serialize_bytes(value)
    }

    fn serialize_none(self) -> Result<Self::Ok, Self::Error> {
        self.inner.serialize_none()
    }

    fn serialize_some<T: ?Sized + Serialize>(self, value: &T) -> Result<Self::Ok, Self::Error> {
        self.inner.serialize_some(value)
    }

    fn serialize_unit(self) -> Result<Self::Ok, Self::Error> {
        self.inner.serialize_unit()
    }

    fn serialize_unit_struct(self, name: &'static str) -> Result<Self::Ok, Self::Error> {
        self.inner.serialize_unit_struct(name)
    }

    fn serialize_unit_variant(
        self,
        name: &'static str,
        variant_index: u32,
        variant: &'static str,
    ) -> Result<Self::Ok, Self::Error> {
        self.inner
            .serialize_unit_variant(name, variant_index, variant)
    }

    fn serialize_newtype_struct<T: ?Sized + Serialize>(
        self,
        name: &'static str,
        value: &T,
    ) -> Result<Self::Ok, Self::Error> {
        self.inner.serialize_newtype_struct(name, value)
    }

    fn serialize_newtype_variant<T: ?Sized + Serialize>(
        self,
        name: &'static str,
        variant_index: u32,
        variant: &'static str,
        value: &T,
    ) -> Result<Self::Ok, Self::Error> {
        self.inner
            .serialize_newtype_variant(name, variant_index, variant, value)
    }

    fn serialize_seq(self, len: Option<usize>) -> Result<Self::SerializeSeq, Self::Error> {
        self.inner.serialize_seq(len)
    }

    fn serialize_tuple(self, len: usize) -> Result<Self::SerializeTuple, Self::Error> {
        self.inner.serialize_tuple(len)
    }

    fn serialize_tuple_struct(
        self,
        name: &'static str,
        len: usize,
    ) -> Result<Self::SerializeTupleStruct, Self::Error> {
        self.inner.serialize_tuple_struct(name, len)
    }

    fn serialize_tuple_variant(
        self,
        name: &'static str,
        variant_index: u32,
        variant: &'static str,
        len: usize,
    ) -> Result<Self::SerializeTupleVariant, Self::Error> {
        self.inner
            .serialize_tuple_variant(name, variant_index, variant, len)
    }

    fn serialize_map(self, len: Option<usize>) -> Result<Self::SerializeMap, Self::Error> {
        self.inner.serialize_map(len)
    }

    fn serialize_struct(
        self,
        name: &'static str,
        len: usize,
    ) -> Result<Self::SerializeStruct, Self::Error> {
        Ok(RoundingStruct {
            inner: self.inner.serialize_struct(name, len)?,
            standing: self.standing,
            trust: self.trust,
            extra: self.extra,
            skip: self.skip,
        })
    }

    fn serialize_struct_variant(
        self,
        name: &'static str,
        variant_index: u32,
        variant: &'static str,
        len: usize,
    ) -> Result<Self::SerializeStructVariant, Self::Error> {
        self.inner
            .serialize_struct_variant(name, variant_index, variant, len)
    }

    fn collect_str<T: ?Sized + std::fmt::Display>(
        self,
        value: &T,
    ) -> Result<Self::Ok, Self::Error> {
        self.inner.collect_str(value)
    }
}

#[cfg(test)]
mod tests {
    use super::{snapshot, social_fabric, AgentView};
    use serde_json::Value;
    use society_sim::World;

    #[test]
    fn the_payload_carries_the_world_and_its_social_fabric() {
        let w = World::founding(7);
        let value = serde_json::to_value(snapshot(&w)).expect("the snapshot serialises");
        assert_eq!(value["turn"], 0);
        assert_eq!(value["day"], 0);
        assert_eq!(value["season"], w.season().id());
        assert_eq!(value["time_of_day"], w.time_of_day().id());
        assert_eq!(value["event_count"], w.events.len());
        assert_eq!(
            value["agents"].as_array().map(Vec::len),
            Some(w.agents.len()),
            "every villager is in the payload"
        );
        assert_eq!(
            value["social"]["living_count"].as_u64(),
            Some(u64::try_from(w.living().count()).expect("a count")),
        );
        assert_eq!(value["places"].as_array().map(Vec::len), Some(7));
    }

    #[test]
    fn the_standing_and_the_trust_are_rounded_for_display() {
        let mut w = World::founding(7);
        w.agent_mut("Ada").unwrap().standing = 2.37;
        w.agent_mut("Ada").unwrap().set_trust("Bo", 3.26);
        let bytes = serde_json::to_string(&AgentView::living(w.agent("Ada").expect("Ada")))
            .expect("serialises");
        // The wire carries one decimal: the rounding is what observers read.
        assert!(bytes.contains("\"standing\":2.4,"), "{bytes}");
        assert!(bytes.contains("[\"Bo\",3.3]"), "{bytes}");
    }

    #[test]
    fn the_living_village_carries_every_field_but_the_claims() {
        let w = World::founding(7);
        let agent = w.agent("Ada").expect("Ada");
        let living =
            serde_json::to_value(AgentView::living(agent)).expect("the villager serialises");
        let record = serde_json::to_value(agent).expect("the villager serialises");
        for (key, value) in record.as_object().expect("a record") {
            if key == "standing" || key == "trust" {
                continue;
            }
            if key == "facts" {
                assert!(
                    living.get(key).is_none(),
                    "the living village carries no claims"
                );
                continue;
            }
            assert_eq!(living.get(key), Some(value), "villager field {key}");
        }
    }

    #[test]
    fn the_whole_memory_carries_every_field() {
        let w = World::founding(7);
        let agent = w.agent("Ada").expect("Ada");
        let whole =
            serde_json::to_value(AgentView::complete(agent)).expect("the villager serialises");
        let record = serde_json::to_value(agent).expect("the villager serialises");
        for (key, value) in record.as_object().expect("a record") {
            if key == "standing" || key == "trust" {
                continue;
            }
            assert_eq!(whole.get(key), Some(value), "villager field {key}");
        }
        assert!(
            whole.get("facts").is_some(),
            "the whole memory carries the claims"
        );
    }

    #[test]
    fn the_social_fabric_counts_what_it_finds() {
        let mut w = World::founding(7);
        w.agent_mut("Ada").unwrap().set_trust("Bo", 3.2);
        w.agent_mut("Bo").unwrap().set_trust("Ada", 3.1);
        w.agent_mut("Ada").unwrap().set_trust("Cor", 0.5);
        let value = serde_json::to_value(social_fabric(&w)).expect("the social serialises");
        assert_eq!(value["alliances"].as_array().map(Vec::len), Some(1));
        assert_eq!(value["feuds"].as_array().map(Vec::len), Some(1));
        assert_eq!(value["living_count"].as_u64(), Some(9));
        let alliance: &Value = &value["alliances"][0];
        assert_eq!(alliance["members"], serde_json::json!(["Ada", "Bo"]));
    }
}
