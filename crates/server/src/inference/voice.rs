//! Small-model voice. One job: turn a decided action into one spoken line.
//!
//! Constrained by measurement:
//! * `format` is a flat JSON Schema with one field, `say`: the line is the only
//!   thing the model answers. It used to carry a `why` clause in front of the
//!   answer; measured, 45 of 45 were prompt fragments, and the engine read one
//!   as the villager's goal (see `Voice::schema`).
//! * Reasoning is per role at its lowest enabled level (`think: "low"`), not
//!   `false`: beam has no `false` in its think values and ignores the flag,
//!   spending the whole budget on reasoning and returning empty content. The
//!   768-token budget covers reasoning plus the JSON.
//! * The schema is echoed in the prompt; llama.cpp does not inject it.

use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

const DEFAULT_BASE: &str = "http://127.0.0.1:11434";

/// A cloud `Retry-After` is advisory and can be longer than a turn may wait;
/// past this the line is reported rather than held.
const MAX_BACKOFF: Duration = Duration::from_secs(10);

#[derive(Debug, Clone)]
pub struct VoiceConfig {
    pub base_url: String,
    /// Model for ordinary villagers.
    pub actor_model: String,
    /// Model for rare, consequential narration.
    pub elder_model: String,
    /// Reasoning level for the actor model. `low` is the floor both cloud
    /// models accept; reasoning stays on, just bounded.
    pub actor_think: Think,
    /// Reasoning level for the elder model.
    pub elder_think: Think,
    pub timeout: Duration,
    pub max_tokens: u32,
}

/// How hard the model thinks before answering.
///
/// The daemon accepts a boolean or one of the model's `/api/show` level names,
/// and rejects the quoted spellings (`"false"`, `"true"` are both HTTP 400).
/// Keeping the boolean case distinct is what lets `qwen3:0.6b` — whose only
/// values are `[false, true]` — be configured by name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Think {
    Off,
    On,
    Level(String),
}

impl Think {
    #[must_use]
    pub fn parse(raw: &str) -> Self {
        match raw.trim() {
            "false" => Self::Off,
            "true" => Self::On,
            other => Self::Level(other.to_owned()),
        }
    }

    fn as_json(&self) -> Value {
        match self {
            Self::Off => json!(false),
            Self::On => json!(true),
            Self::Level(l) => json!(l),
        }
    }
}

impl Default for Think {
    fn default() -> Self {
        Self::parse("low")
    }
}

impl Default for VoiceConfig {
    fn default() -> Self {
        Self {
            base_url: std::env::var("OLLAMA_HOST")
                .map_or_else(|_| DEFAULT_BASE.to_owned(), |h| normalize_host(&h)),
            actor_model: std::env::var("SOCIETY_ACTOR_MODEL")
                .unwrap_or_else(|_| "hf.co/LiquidAI/LFM2.5-350M-GGUF:Q4_K_M".into()),
            elder_model: std::env::var("SOCIETY_ELDER_MODEL")
                .unwrap_or_else(|_| "hf.co/LiquidAI/LFM2.5-350M-GGUF:Q4_K_M".into()),
            actor_think: std::env::var("SOCIETY_ACTOR_THINK")
                .map_or_else(|_| Think::parse("false"), |v| Think::parse(&v)),
            elder_think: std::env::var("SOCIETY_ELDER_THINK")
                .map_or_else(|_| Think::parse("false"), |v| Think::parse(&v)),
            timeout: Duration::from_secs(180),
            // Reasoning plus the JSON. Observed reasoning use at `think: low`
            // reached 630 tokens; a budget near that starves the answer, which
            // arrives empty with `done_reason: length`.
            max_tokens: 1024,
        }
    }
}

/// Ollama accepts `host:port`; reqwest needs a scheme.
fn normalize_host(h: &str) -> String {
    if h.starts_with("http://") || h.starts_with("https://") {
        h.trim_end_matches('/').to_owned()
    } else {
        format!("http://{}", h.trim_end_matches('/'))
    }
}

#[derive(Debug)]
pub enum VoiceError {
    Transport(String),
    /// The backend refused the request (cloud concurrency cap). Carries the
    /// server's `Retry-After`, because retrying sooner only deepens the burst.
    RateLimited {
        retry_after: Duration,
        body: String,
    },
    Status(u16, String),
    Malformed(String),
}

impl VoiceError {
    /// The one error worth backing off on rather than aborting the turn.
    #[must_use]
    pub const fn retry_after(&self) -> Option<Duration> {
        match self {
            Self::RateLimited { retry_after, .. } => Some(*retry_after),
            _ => None,
        }
    }
}

impl std::fmt::Display for VoiceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Transport(e) => write!(f, "ollama transport: {e}"),
            Self::RateLimited { retry_after, body } => write!(
                f,
                "ollama rate limited (retry after {}s): {body}",
                retry_after.as_secs()
            ),
            Self::Status(c, b) => write!(f, "ollama HTTP {c}: {b}"),
            Self::Malformed(e) => write!(f, "ollama response malformed: {e}"),
        }
    }
}

impl std::error::Error for VoiceError {}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Line {
    pub say: String,
    pub model: String,
    pub latency_ms: u64,
}

#[derive(Debug, Clone)]
pub struct Voice {
    cfg: VoiceConfig,
    client: reqwest::Client,
}

/// The three example lines the system prompt shows the model.
///
/// They are in the prompt, so the model can hand one back as dialogue: the
/// copy filter treats them as sources.
const EXAMPLE_LINES: [&str; 3] = [
    "Six measures, Cor. Count them out where everyone can hear.",
    "Lie still. I will find something, even if it is roots.",
    "Nothing. Not even mud.",
];

/// The example lines the system prompt shows the model.
///
/// The prompt is a copy source like the card: a line that quotes one of these
/// is the model reading its own instructions back, not a villager speaking.
#[must_use]
pub fn prompt_examples() -> Vec<String> {
    EXAMPLE_LINES
        .iter()
        .map(|line| (*line).to_owned())
        .collect()
}

impl Voice {
    /// # Panics
    ///
    /// Panics if the HTTP client cannot be built.
    #[must_use]
    pub fn new(cfg: VoiceConfig) -> Self {
        let client = reqwest::Client::builder()
            .timeout(cfg.timeout)
            .build()
            .expect("client builds");
        Self { cfg, client }
    }

    #[must_use]
    pub const fn config(&self) -> &VoiceConfig {
        &self.cfg
    }

    /// The single output schema. Flat, no `$ref`, and one field: the line.
    ///
    /// The schema used to ask for a `why` clause in front of `say`, on the
    /// theory that a small model writes a better line after naming its reason.
    /// Measured, the clause was never a reason: 45 of 45 were prompt fragments —
    /// a system-prompt example ("she will not leave him"), a card line
    /// ("Village atmosphere: spring (dawn) · clear (14 °C) · morale 2.50"), a
    /// label ("mood: determined", "person"). Worse, the engine preferred it as
    /// the villager's goal whenever it was between 8 and 72 characters, so a
    /// parroted fragment could become that villager's goal. The judgment layer
    /// already knows why the villager acts; the motive belongs there, and this
    /// model is asked only for the words.
    fn schema() -> Value {
        json!({
            "type": "object",
            "properties": {
                "say": {
                    "type": "string",
                    "description": "The words spoken aloud, one line"
                }
            },
            "required": ["say"]
        })
    }

    /// Render one line. `card` is built by `society-sim`; `decision` is already
    /// committed, so the model is never asked to choose.
    /// `speaker` only seeds the sampling temperature; it is never sent.
    /// `think` is the model's reasoning level, passed through to the daemon.
    ///
    /// # Errors
    ///
    /// Returns a `VoiceError` on transport failure, backpressure, or an empty line.
    pub async fn speak(
        &self,
        card: &str,
        model: &str,
        speaker: &str,
        think: &Think,
    ) -> Result<Line, VoiceError> {
        // A 429 is the only error that must not be retried immediately: the
        // cloud account caps concurrency, and each attempt before `Retry-After`
        // keeps the queue full. Two waits, then one last attempt.
        for _ in 0..2 {
            match self.speak_once(card, model, speaker, think, false).await {
                Ok(l) => return Ok(l),
                Err(VoiceError::RateLimited { retry_after, .. }) => {
                    // Jittered: nine lines share one account, and retrying in
                    // lockstep rebuilds the burst that caused the 429.
                    let jitter =
                        Duration::from_millis(speaker.bytes().map(u64::from).sum::<u64>() % 250);
                    tokio::time::sleep(retry_after.min(MAX_BACKOFF) + jitter).await;
                },
                // One retry: a truncated or empty JSON reply is transient far
                // more often than it is systematic.
                Err(_) => return self.speak_once(card, model, speaker, think, false).await,
            }
        }
        self.speak_once(card, model, speaker, think, false).await
    }

    /// Deterministic speech habit derived from the villager's name so each
    /// inhabitant has a recognizable cadence across the life of the village.
    #[must_use]
    pub fn vocal_cadence(speaker: &str) -> &'static str {
        const CADENCES: [&str; 8] = [
            "Speak plainly with concrete nouns (grain, mud, roof, salt, frost).",
            "Speak in short, measured clauses; weigh promises and debts carefully.",
            "Speak with earthy directness; notice weather, hands, and tools.",
            "Speak with quiet intensity; name the person you address or distrust.",
            "Speak practically; focus on who worked, who shared, and who held back.",
            "Speak with dry understatement; never waste words when hungry or cold.",
            "Speak with keen memory of kinship, old favors, and broken oaths.",
            "Speak sharply and observant; pay attention to who owns the ground you stand on.",
        ];
        let hash = speaker.bytes().fold(5381_u32, |acc, b| {
            acc.wrapping_mul(33).wrapping_add(u32::from(b))
        });
        CADENCES[(hash as usize) % CADENCES.len()]
    }

    fn system_prompt(speaker: &str, elder: bool) -> String {
        let role_clause = if elder {
            "You are an Elder of the settlement: keeper of village custom, shared memory, and hard precedent. Your words carry communal weight."
        } else {
            "You are a villager in a small, tight-knit settlement where everyone remembers favors, grudges, kinship, and scarcity."
        };
        let cadence = Self::vocal_cadence(speaker);
        let [e0, e1, e2] = EXAMPLE_LINES;
        format!(
            "{role_clause} Reply with JSON only, matching this schema exactly:\n\
            {{\"say\":\"<one line spoken aloud>\"}}\n\n\
            Examples of the required style:\n\
            Situation: Ada, market. Cor carries six, Ada carries two, nothing is owed.\n\
            {{\"say\":\"{e0}\"}}\n\n\
            Situation: Ada, hut. Rain. Bo is here, sick, carrying none.\n\
            {{\"say\":\"{e1}\"}}\n\n\
            Situation: Wren, well. Nobody here. The well is dry.\n\
            {{\"say\":\"{e2}\"}}\n\n\
            Vocal habit for {speaker}: {cadence}\n\n\
            Rules:\n\
            - `say` is speech only: no narration, no stage directions, no summary of the scene.\n\
            - One sentence, at most twenty words.\n\
            - If you hesitated between two actions, let that tension show in what you say.\n\
            - Never reuse a sentence from the examples above.\n\
            - Do not count food out loud unless you are making a demand or an offer.\n\
            - Say something new about this exact person in this exact place."
        )
    }

    async fn speak_once(
        &self,
        card: &str,
        model: &str,
        speaker: &str,
        think: &Think,
        elder: bool,
    ) -> Result<Line, VoiceError> {
        let system = Self::system_prompt(speaker, elder);
        let body = json!({
            "model": model,
            "stream": false,
            "think": think.as_json(),
            "format": Self::schema(),
            "options": {
                "temperature": Self::temperature_for(speaker, 0.95),
                "num_predict": self.cfg.max_tokens,
                "top_p": 0.95,
            },
            "messages": [
                {"role": "system", "content": system},
                {"role": "user", "content": card}
            ]
        });

        let started = std::time::Instant::now();
        let resp = self
            .client
            .post(format!("{}/api/chat", self.cfg.base_url))
            .json(&body)
            .send()
            .await
            .map_err(|e| VoiceError::Transport(e.to_string()))?;
        let status = resp.status();
        let retry_after = resp
            .headers()
            .get("retry-after")
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.trim().parse::<u64>().ok())
            .map(Duration::from_secs);
        let text = resp
            .text()
            .await
            .map_err(|e| VoiceError::Transport(e.to_string()))?;
        if status.as_u16() == 429 {
            return Err(VoiceError::RateLimited {
                retry_after: retry_after.unwrap_or(Duration::from_secs(5)),
                body: text.chars().take(200).collect(),
            });
        }
        if !status.is_success() {
            return Err(VoiceError::Status(
                status.as_u16(),
                text.chars().take(300).collect(),
            ));
        }
        let v: Value =
            serde_json::from_str(&text).map_err(|e| VoiceError::Malformed(e.to_string()))?;
        let content = v
            .get("message")
            .and_then(|m| m.get("content"))
            .and_then(|c| c.as_str())
            .ok_or_else(|| VoiceError::Malformed("no message.content".into()))?;
        let parsed: Value = serde_json::from_str(content)
            .map_err(|e| VoiceError::Malformed(format!("content not JSON: {e}")))?;
        let say = parsed
            .get("say")
            .and_then(|s| s.as_str())
            .unwrap_or("")
            .trim()
            .trim_matches('"')
            .to_owned();
        // A reply with no `say` is not a line. Returning "" here would commit a
        // mute villager and look like a working turn; surfacing it makes the
        // one retry run and, if that fails, names the villager that failed.
        if say.is_empty() {
            return Err(VoiceError::Malformed(format!("empty say in {content:?}")));
        }
        Ok(Line {
            say,
            model: model.to_owned(),
            latency_ms: started.elapsed().as_millis() as u64,
        })
    }

    /// # Errors
    ///
    /// Returns a `VoiceError` on transport failure, backpressure, or an empty line.
    pub async fn speak_actor(&self, card: &str, speaker: &str) -> Result<Line, VoiceError> {
        let m = self.cfg.actor_model.clone();
        let think = self.cfg.actor_think.clone();
        self.speak(card, &m, speaker, &think).await
    }

    /// Vary sampling per agent. Nine calls at one temperature converge on the
    /// same phrasing; the offset is derived from the name so runs stay
    /// reproducible.
    fn temperature_for(name: &str, base: f32) -> f32 {
        let h: u32 = name.bytes().map(u32::from).sum();
        ((h % 5) as f32).mul_add(0.05, base)
    }

    /// # Errors
    ///
    /// Returns a `VoiceError` on transport failure, backpressure, or an empty line.
    pub async fn speak_elder(&self, card: &str, speaker: &str) -> Result<Line, VoiceError> {
        let m = self.cfg.elder_model.clone();
        let think = self.cfg.elder_think.clone();
        for _ in 0..2 {
            match self.speak_once(card, &m, speaker, &think, true).await {
                Ok(l) => return Ok(l),
                Err(VoiceError::RateLimited { retry_after, .. }) => {
                    let jitter =
                        Duration::from_millis(speaker.bytes().map(u64::from).sum::<u64>() % 250);
                    tokio::time::sleep(retry_after.min(MAX_BACKOFF) + jitter).await;
                },
                Err(_) => return self.speak_once(card, &m, speaker, &think, true).await,
            }
        }
        self.speak_once(card, &m, speaker, &think, true).await
    }

    /// The distinct models a turn can call, in configuration order.
    #[must_use]
    pub fn configured_models(&self) -> Vec<String> {
        let mut out = vec![self.cfg.actor_model.clone()];
        if self.cfg.elder_model != self.cfg.actor_model {
            out.push(self.cfg.elder_model.clone());
        }
        out
    }

    /// Whether the daemon can resolve one model. `/api/show` answers for local
    /// tags and for cloud models (`name:cloud`) alike; `/api/tags` lists only
    /// pulled local ones, so it reports a cloud-only setup as empty.
    async fn resolves(&self, model: &str) -> Result<bool, VoiceError> {
        let resp = self
            .client
            .post(format!("{}/api/show", self.cfg.base_url))
            .json(&json!({ "model": model }))
            .send()
            .await
            .map_err(|e| VoiceError::Transport(e.to_string()))?;
        let status = resp.status();
        // Drain the body so the connection is reusable.
        let _ = resp.text().await;
        match status.as_u16() {
            200 => Ok(true),
            404 => Ok(false),
            c => Err(VoiceError::Status(c, String::new())),
        }
    }

    /// Liveness for the exact models a turn will call. The simulator does not
    /// require Ollama to be up, but the dev server reports this so a broken
    /// setup — a missing tag, a signed-out cloud session — is visible before a
    /// run.
    ///
    /// # Errors
    ///
    /// Returns a `VoiceError` if the daemon is unreachable.
    pub async fn probe(&self) -> Result<Vec<String>, VoiceError> {
        let mut out = Vec::new();
        for m in self.configured_models() {
            if self.resolves(&m).await? {
                out.push(m);
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};

    #[test]
    fn host_normalization_adds_scheme() {
        assert_eq!(normalize_host("127.0.0.1:11434"), "http://127.0.0.1:11434");
        assert_eq!(normalize_host("http://x:1/"), "http://x:1");
    }

    #[test]
    fn temperature_varies_by_speaker_not_model() {
        let a = Voice::temperature_for("Ada", 0.95);
        let b = Voice::temperature_for("Cor", 0.95);
        assert!((0.95..1.2).contains(&a) && (0.95..1.2).contains(&b));
        // Some pair of names must differ, or the jitter is inert.
        let many: std::collections::HashSet<String> = ["Ada", "Bo", "Cor", "Mira", "Tam"]
            .iter()
            .map(|n| format!("{:.2}", Voice::temperature_for(n, 0.95)))
            .collect();
        assert!(many.len() > 1, "temperature must vary across names");
    }

    #[test]
    fn the_schema_asks_for_the_line_and_nothing_else() {
        // The `why` clause is gone: measured, 45 of 45 were prompt fragments
        // and the engine read one as the villager's goal. The motive is the
        // judgment layer's answer; the voice model writes only the line.
        let s = Voice::schema();
        let props = s["properties"].as_object().unwrap();
        assert_eq!(props.len(), 1, "{s}");
        assert!(props.contains_key("say"), "{s}");
        assert_eq!(s["required"], json!(["say"]), "{s}");
    }

    #[tokio::test]
    async fn a_reply_that_carries_only_say_parses_into_a_line() {
        // The schema no longer has a second field, so a conforming reply has
        // exactly one property and must still become a line.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let body = json!({"message": {"content": r#"{"say":"The well is dry."}"#}}).to_string();
        std::thread::spawn(move || {
            let Ok((mut s, _)) = listener.accept() else {
                return;
            };
            let mut buf = [0_u8; 8192];
            let _ = s.read(&mut buf);
            let _ = write!(
                s,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{}",
                body.len(),
                body
            );
        });

        let v = Voice::new(VoiceConfig {
            base_url: format!("http://127.0.0.1:{port}"),
            actor_model: "beam:cloud".into(),
            elder_model: "x".into(),
            actor_think: Think::parse("low"),
            elder_think: Think::parse("low"),
            timeout: Duration::from_secs(2),
            max_tokens: 64,
        });
        let line = v
            .speak_once("card", "beam:cloud", "Ada", &Think::default(), false)
            .await
            .expect("a say-only reply is a line");
        assert_eq!(line.say, "The well is dry.");
        assert_eq!(line.model, "beam:cloud");
    }

    #[test]
    fn think_false_is_a_boolean_not_a_string() {
        // The daemon rejects the quoted spellings; both booleans must be sent
        // as JSON booleans. `qwen3:0.6b` accepts only `false`/`true`.
        assert_eq!(Think::parse("false"), Think::Off);
        assert_eq!(Think::Off.as_json(), json!(false));
        assert_eq!(Think::parse("true"), Think::On);
        assert_eq!(Think::On.as_json(), json!(true));
        assert_eq!(Think::parse("low").as_json(), json!("low"));
        assert_eq!(Think::parse(" low ").as_json(), json!("low"));
        assert_eq!(Think::default(), Think::Level("low".into()));
    }

    #[test]
    fn configured_models_are_the_distinct_models_a_turn_calls() {
        let cfg = VoiceConfig {
            base_url: "http://127.0.0.1:1".into(),
            actor_model: "beam:cloud".into(),
            elder_model: "deepseek-v4.1-flash:cloud".into(),
            actor_think: Think::parse("low"),
            elder_think: Think::parse("low"),
            timeout: Duration::from_secs(1),
            max_tokens: 8,
        };
        let v = Voice::new(cfg.clone());
        assert_eq!(
            v.configured_models(),
            vec!["beam:cloud", "deepseek-v4.1-flash:cloud"]
        );

        // One model for both roles must not be probed twice.
        let same = Voice::new(VoiceConfig {
            elder_model: cfg.actor_model.clone(),
            ..cfg
        });
        assert_eq!(same.configured_models(), vec!["beam:cloud"]);
    }

    /// A 429 must be classified as backpressure with the server's own delay,
    /// because that is what the engine keys its shed-on-429 on. Retrying
    /// without the delay is what turns one burst into a halt.
    #[tokio::test]
    async fn a_429_is_rate_limiting_with_the_advertised_retry_after() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for _ in 0..4 {
                let Ok((mut s, _)) = listener.accept() else {
                    return;
                };
                let mut buf = [0_u8; 4096];
                let _ = s.read(&mut buf);
                let _ = s.write_all(
                    b"HTTP/1.1 429 Too Many Requests\r\nRetry-After: 7\r\nContent-Length: 2\r\n\r\n{}",
                );
            }
        });

        let v = Voice::new(VoiceConfig {
            base_url: format!("http://127.0.0.1:{port}"),
            actor_model: "beam:cloud".into(),
            elder_model: "x".into(),
            actor_think: Think::parse("low"),
            elder_think: Think::parse("low"),
            timeout: Duration::from_secs(2),
            max_tokens: 8,
        });
        // Call the single attempt so the test does not sit through Retry-After.
        let err = v
            .speak_once("card", "beam:cloud", "Ada", &Think::default(), false)
            .await
            .unwrap_err();
        assert_eq!(err.retry_after(), Some(Duration::from_secs(7)));

        // A non-429 stays fatal for the turn; it must not be mistaken for
        // backpressure.
        let status_err = VoiceError::Status(500, "boom".into());
        assert_eq!(status_err.retry_after(), None);
    }

    /// The retry loop is the part that turns a 429 into a spoken line rather
    /// than a lost villager: first attempt rate-limited, second succeeds. A
    /// regression here would surface only under load, which is exactly when a
    /// village cannot afford it.
    #[tokio::test]
    async fn speak_recovers_after_a_rate_limit_and_returns_the_line() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for attempt in 0..2 {
                let Ok((mut s, _)) = listener.accept() else {
                    return;
                };
                let mut buf = [0_u8; 8192];
                let _ = s.read(&mut buf);
                if attempt == 0 {
                    let _ = s.write_all(
                        b"HTTP/1.1 429 Too Many Requests\r\nRetry-After: 0\r\nContent-Length: 2\r\n\r\n{}",
                    );
                } else {
                    let body = r#"{"message":{"content":"{\"say\":\"The well is dry.\"}"}}"#;
                    let _ = write!(
                        s,
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{}",
                        body.len(),
                        body
                    );
                }
            }
        });

        let v = Voice::new(VoiceConfig {
            base_url: format!("http://127.0.0.1:{port}"),
            actor_model: "beam:cloud".into(),
            elder_model: "x".into(),
            actor_think: Think::parse("low"),
            elder_think: Think::parse("low"),
            timeout: Duration::from_secs(2),
            max_tokens: 64,
        });
        let line = v
            .speak("card", "beam:cloud", "Ada", &Think::default())
            .await
            .expect("a rate-limited line is retried, not dropped");
        assert_eq!(line.say, "The well is dry.");
    }

    #[test]
    fn elder_and_actor_prompts_carry_distinct_framing_and_cadence() {
        let actor = Voice::system_prompt("Ada", false);
        let elder = Voice::system_prompt("Ada", true);
        assert!(elder.contains("Elder of the settlement"));
        assert!(!actor.contains("Elder of the settlement"));
        assert_eq!(Voice::vocal_cadence("Ada"), Voice::vocal_cadence("Ada"));
    }
}
