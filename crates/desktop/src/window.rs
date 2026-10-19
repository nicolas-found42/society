//! The window. Deliberately read-mostly: the society runs in the engine and is
//! driven through the dev server, so the UI is an observer, not the owner of
//! state.

use std::sync::Arc;
use std::time::{Duration, Instant};

use eframe::egui;
use egui::{Color32, RichText};
use society_server::{AppState, TurnRecord};
use society_sim::{Agent, Event, EventKind, Place, PlaceState, World};

const MAP_W: f32 = 420.0;
const MAP_H: f32 = 360.0;

pub fn run(
    rt: tokio::runtime::Handle,
    state: AppState,
    port: u16,
    view: &str,
    select: Option<&str>,
    expand_state: bool,
    select_none: bool,
    size: Option<(f32, f32)>,
    autostart: bool,
    focus: bool,
) -> Result<(), eframe::Error> {
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
        Box::new(move |_cc| {
            Ok(Box::new(App::new(
                rt,
                state,
                port,
                &view,
                select,
                expand_state,
                select_none,
                autostart,
            )))
        }),
    )
}

struct App {
    rt: tokio::runtime::Handle,
    state: AppState,
    port: u16,
    snapshot: Option<Snapshot>,
    turns: Vec<TurnRecord>,
    selected: Option<String>,
    inspector: Option<Inspector>,
    tab: Tab,
    /// Open the judgment-state disclosure on first draw. Screenshot aid only.
    expand_state: bool,
    auto: bool,
    speed: f32,
    last_turn: Instant,
    status: String,
    busy: bool,
    pending: Option<(
        std::sync::mpsc::Receiver<(Vec<TurnRecord>, Option<String>)>,
        Instant,
    )>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    World,
    Feed,
    Inspector,
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

impl Snapshot {
    fn from_world(w: &World) -> Self {
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
    fn new(
        rt: tokio::runtime::Handle,
        state: AppState,
        port: u16,
        view: &str,
        select: Option<&str>,
        expand_state: bool,
        select_none: bool,
        autostart: bool,
    ) -> Self {
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
                Some(select.map(str::to_string).unwrap_or(first))
            },
            expand_state,
            inspector: None,
            // A view flag lets a screenshot capture each panel without a click,
            // so observing the society never takes focus from the observer.
            tab: match view {
                "feed" => Tab::Feed,
                "inspector" => Tab::Inspector,
                _ => Tab::World,
            },
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
            self.turns = run.clone();
        }
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
                        Ok(r) => recs.push(r),
                        Err(e) => {
                            err = Some(e.to_string());
                            break;
                        }
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
                self.status = match err {
                    Some(e) => format!("halted: {e}"),
                    None => format!("{} turn(s) in {:.1}s", recs.len(), secs),
                };
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                self.pending = Some((rx, started));
            }
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                self.busy = false;
                self.status = "turn worker vanished".into();
            }
        }
    }
}

impl eframe::App for App {
    fn ui(&mut self, root: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = root.ctx().clone();
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
                ui.heading("society");
                ui.separator();
                if let Some(s) = &self.snapshot {
                    ui.label(format!("day {} · turn {}", s.day, s.turn));
                    ui.separator();
                    ui.label(format!("{} · {}", s.season, s.time_of_day));
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
                });
            }
            ui.horizontal(|ui| {
                ui.selectable_value(&mut self.tab, Tab::World, "World");
                ui.selectable_value(&mut self.tab, Tab::Feed, "Feed");
                ui.selectable_value(&mut self.tab, Tab::Inspector, "Inspector");
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
            .map(|s| s.agents.iter().filter(|a| a.alive).count())
            .unwrap_or(0);
        egui::Panel::right("roster")
            .resizable(true)
            .default_size(260.0)
            .show(root, |ui| {
                ui.heading(format!(
                    "Villagers {}/{}",
                    alive_count,
                    society_sim::POPULATION_CAP
                ));
                egui::ScrollArea::vertical()
                    .id_salt("roster_scroll")
                    .show(ui, |ui| {
                        if let Some(s) = &self.snapshot {
                            for a in &s.agents {
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
                                        RichText::new(label).strong()
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
                                            "{} · {}",
                                            a.place.title(),
                                            humanize(a.mood.id())
                                        ))
                                        .small(),
                                    );
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
                let height = (ui.available_height() * 0.58).clamp(290.0, 430.0);
                ui.allocate_ui(egui::vec2(ui.available_width(), height), |ui| {
                    self.draw_ring(ui)
                });
                self.draw_stockpiles(ui);
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
                rect.left() + 70.0 + p.x / MAP_W * (rect.width() - 140.0),
                rect.top() + 65.0 + p.y / MAP_H * (rect.height() - 130.0),
            )
        };

        for (a, b) in EDGES {
            painter.line_segment(
                [to_screen(fixed_place_pos(a)), to_screen(fixed_place_pos(b))],
                egui::Stroke::new(1.5, Color32::from_gray(120)),
            );
        }

        if let Some(s) = &self.snapshot {
            for p in &s.places {
                let center = to_screen(fixed_place_pos(p.place.id()));
                let node_rect = egui::Rect::from_center_size(center, egui::vec2(132.0, 48.0));
                painter.rect_filled(node_rect, 8.0, Color32::from_rgb(38, 42, 48));
                painter.rect_stroke(
                    node_rect,
                    8.0,
                    egui::Stroke::new(1.0, Color32::from_gray(110)),
                    egui::StrokeKind::Inside,
                );
                painter.text(
                    center,
                    egui::Align2::CENTER_CENTER,
                    format!(
                        "{}\n{} food · {} water",
                        humanize(p.place.id()),
                        p.state.food,
                        p.state.water
                    ),
                    egui::FontId::proportional(12.0),
                    Color32::from_gray(225),
                );
                ui.interact(
                    node_rect,
                    ui.scope_id().with(("place", p.place.id())),
                    egui::Sense::hover(),
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
                ));
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
                        -std::f32::consts::FRAC_PI_2 + (i as f32 / n) * std::f32::consts::TAU;
                    let mut pos = center + egui::vec2(angle.cos(), angle.sin()) * orbit;
                    // Keep the marker inside the map: a villager at the top of
                    // the ring must not be clipped by the panel edge.
                    pos.x = pos.x.clamp(rect.left() + margin, rect.right() - margin);
                    pos.y = pos.y.clamp(rect.top() + margin, rect.bottom() - margin);
                    if ui
                        .interact(
                            egui::Rect::from_center_size(pos, egui::vec2(24.0, 24.0)),
                            ui.scope_id().with(("villager", name)),
                            egui::Sense::click(),
                        )
                        .on_hover_text(format!("Inspect {name}"))
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
                    painter.circle_filled(
                        pos,
                        dot,
                        if selected {
                            Color32::from_rgb(230, 245, 255)
                        } else {
                            Color32::from_rgb(90, 160, 220)
                        },
                    );
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

    fn draw_feed(&mut self, ui: &mut egui::Ui) {
        let Some(s) = &self.snapshot else {
            return;
        };
        if self.turns.is_empty() && s.events.is_empty() {
            ui.label("No village events yet. Run or step to begin.");
            return;
        }
        ui.label(
            RichText::new("Recent village history · saved events survive a restart")
                .weak()
                .small(),
        );
        // Merge the saved event history with this session's inference details.
        // A restart no longer hides crafting, healing, trade, prayer, or bonds.
        let turns: std::collections::BTreeSet<u64> = s
            .events
            .iter()
            .map(|e| e.turn)
            .chain(self.turns.iter().map(|r| r.turn))
            .collect();
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
                                .find(|a| a.name == e.actor && a.action == e.action)
                        }) {
                            ui.label(
                                RichText::new(format!(
                                    "{} · social {} · fidelity {} · grounded {} · intensity {}",
                                    if a.accepted { "Accepted" } else { "Rejected" },
                                    humanize(&a.social),
                                    fmt(a.fidelity),
                                    fmt(a.grounded),
                                    fmt(a.intensity)
                                ))
                                .weak()
                                .small(),
                            );
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
                            ui.label(&a.say);
                            if let Some(note) = &a.note {
                                ui.label(RichText::new(note).weak().small());
                            }
                        }
                    }
                }
            });
    }

    fn draw_inspector(&mut self, ui: &mut egui::Ui) {
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
                    let death = a
                        .death
                        .as_ref()
                        .map(|d| format!("{} at turn {}", d.cause, d.turn))
                        .unwrap_or_else(|| "dead".into());
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
                            ("Health", a.health as f32, 10.0, a.health < 5),
                            ("Energy", a.energy as f32, 10.0, a.energy <= 2),
                            ("Warmth", a.warmth as f32, 4.0, a.warmth <= 1),
                            ("Hunger", a.hunger as f32, 3.0, a.hunger >= 2),
                            ("Thirst", a.thirst as f32, 3.0, a.thirst >= 2),
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
                                let names: std::collections::BTreeSet<&str> = a
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
                                    ui.label(
                                        relation
                                            .map(|r| humanize(r.kinship.id()))
                                            .unwrap_or_else(|| "Unrecorded".into()),
                                    );
                                    ui.label(format!("{:.1}", a.trust_of(name)));
                                    for value in [
                                        relation.map(|r| r.affinity),
                                        relation.map(|r| r.respect),
                                        relation.map(|r| r.grudge),
                                    ] {
                                        ui.label(
                                            value
                                                .map(|v| format!("{v:.1}"))
                                                .unwrap_or_else(|| "—".into()),
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
                                belief
                                    .source
                                    .as_deref()
                                    .map(|n| format!("told by {n}"))
                                    .unwrap_or_else(|| "observed directly".into())
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
                                        .map(|t| t.to_string())
                                        .unwrap_or_else(|| "—".into()),
                                    fact.expired_at
                                        .map(|t| t.to_string())
                                        .unwrap_or_else(|| "—".into())
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
    let galley = painter.layout_no_wrap(text.to_string(), font, Color32::from_gray(150));
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
    v.map(|x| format!("{x:.2}")).unwrap_or_else(|| "—".into())
}

#[allow(dead_code)]
fn _assert_state_send_sync() {
    fn check<T: Send + Sync>() {}
    check::<AppState>();
    check::<Arc<AppState>>();
}

fn humanize(id: &str) -> String {
    let text = id.replace('_', " ").replace("go:", "Go to ");
    let mut chars = text.chars();
    chars
        .next()
        .map(|c| c.to_uppercase().collect::<String>() + chars.as_str())
        .unwrap_or_default()
}

fn inventory_text(food: u32, water: u32, wood: u32, herbs: u32, tools: u32, coin: u32) -> String {
    format!(
        "Food {food} · Water {water} · Wood {wood} · Herbs {herbs} · Tools {tools} · Coin {coin}"
    )
}

fn memory_kind(kind: society_sim::MemoryKind) -> &'static str {
    use society_sim::MemoryKind::*;
    match kind {
        Own => "Own",
        Witnessed => "Witnessed",
        Reflection => "Reflection",
        Scar => "Scar",
        Tradition => "Tradition",
        Milestone => "Milestone",
    }
}

fn event_style(kind: EventKind) -> (&'static str, Color32) {
    use EventKind::*;
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
        let voice = Arc::new(society_server::Voice::new(Default::default()));
        let save_path = std::path::PathBuf::from("/unused-ui-test-save.json");
        let engine = Arc::new(society_server::Engine::new(
            judge.clone(),
            voice.clone(),
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
            8100,
            "world",
            Some("Ada"),
            false,
            false,
            false,
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
                    Tab::Feed => app.draw_feed(ui),
                    Tab::Inspector => app.draw_inspector(ui),
                });
            },
        );
        fn texts(shape: &egui::epaint::Shape, out: &mut String) {
            match shape {
                egui::epaint::Shape::Text(t) => {
                    out.push_str(&t.galley.job.text);
                    out.push('\n');
                }
                egui::epaint::Shape::Vec(shapes) => {
                    for s in shapes {
                        texts(s, out);
                    }
                }
                _ => {}
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
                fidelity: None,
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
}
