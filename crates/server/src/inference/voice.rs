//! Small-model voice. One job: turn a decided action into one spoken line.
//!
//! Constrained by measurement:
//! * `format` is a flat JSON Schema with `why` **before** `say`, because field
//!   order is what the structured-output literature actually fixes.
//! * `think: false`. With thinking on, qwen3:0.6b spends the whole budget on
//!   reasoning and returns no content at all.
//! * The schema is echoed in the prompt; llama.cpp does not inject it.

use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

const DEFAULT_BASE: &str = "http://127.0.0.1:11434";

#[derive(Debug, Clone)]
pub struct VoiceConfig {
    pub base_url: String,
    /// Model for ordinary villagers.
    pub actor_model: String,
    /// Model for rare, consequential narration.
    pub elder_model: String,
    pub timeout: Duration,
    pub max_tokens: u32,
}

impl Default for VoiceConfig {
    fn default() -> Self {
        Self {
            base_url: std::env::var("OLLAMA_HOST")
                .map(|h| normalize_host(&h))
                .unwrap_or_else(|_| DEFAULT_BASE.to_string()),
            actor_model: std::env::var("SOCIETY_ACTOR_MODEL")
                .unwrap_or_else(|_| "qwen3:0.6b".into()),
            elder_model: std::env::var("SOCIETY_ELDER_MODEL")
                .unwrap_or_else(|_| "gemma4:12b".into()),
            timeout: Duration::from_secs(180),
            max_tokens: 192,
        }
    }
}

/// Ollama accepts `host:port`; reqwest needs a scheme.
fn normalize_host(h: &str) -> String {
    if h.starts_with("http://") || h.starts_with("https://") {
        h.trim_end_matches('/').to_string()
    } else {
        format!("http://{}", h.trim_end_matches('/'))
    }
}

#[derive(Debug)]
pub enum VoiceError {
    Transport(String),
    Status(u16, String),
    Malformed(String),
}

impl std::fmt::Display for VoiceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            VoiceError::Transport(e) => write!(f, "ollama transport: {e}"),
            VoiceError::Status(c, b) => write!(f, "ollama HTTP {c}: {b}"),
            VoiceError::Malformed(e) => write!(f, "ollama response malformed: {e}"),
        }
    }
}

impl std::error::Error for VoiceError {}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Line {
    pub say: String,
    pub why: String,
    pub model: String,
    pub latency_ms: u64,
}

#[derive(Debug, Clone)]
pub struct Voice {
    cfg: VoiceConfig,
    client: reqwest::Client,
}

impl Voice {
    pub fn new(cfg: VoiceConfig) -> Self {
        let client = reqwest::Client::builder()
            .timeout(cfg.timeout)
            .build()
            .expect("client builds");
        Self { cfg, client }
    }

    pub fn config(&self) -> &VoiceConfig {
        &self.cfg
    }

    /// The single output schema. Flat, no `$ref`, reasoning field first.
    fn schema() -> Value {
        json!({
            "type": "object",
            "properties": {
                "why": {
                    "type": "string",
                    "description": "One short clause naming what makes this line fit the moment"
                },
                "say": {
                    "type": "string",
                    "description": "The words spoken aloud, one line"
                }
            },
            "required": ["why", "say"]
        })
    }

    /// Render one line. `card` is built by `society-sim`; `decision` is already
    /// committed, so the model is never asked to choose.
    /// `speaker` only seeds the sampling temperature; it is never sent.
    pub async fn speak(&self, card: &str, model: &str, speaker: &str) -> Result<Line, VoiceError> {
        // One retry: a truncated or empty JSON reply is transient far more
        // often than it is systematic.
        match self.speak_once(card, model, speaker).await {
            Ok(l) => Ok(l),
            Err(_) => self.speak_once(card, model, speaker).await,
        }
    }

    async fn speak_once(&self, card: &str, model: &str, speaker: &str) -> Result<Line, VoiceError> {
        let system = concat!(
            "You are a villager in a small settlement. Reply with JSON only, matching this schema exactly:\n",
            r#"{"why":"<one short clause>","say":"<one line spoken aloud>"}"#,
            "\n\nExamples of the required style:\n",
            "Situation: Ada, market. Cor carries six, Ada carries two, nothing is owed.\n",
            r#"{"why":"she wants the debt settled in public","say":"Six measures, Cor. Count them out where everyone can hear."}"#,
            "\n\nSituation: Ada, hut. Rain. Bo is here, sick, carrying none.\n",
            r#"{"why":"she will not leave him","say":"Lie still. I will find something, even if it is roots."}"#,
            "\n\nSituation: Wren, well. Nobody here. The well is dry.\n",
            r#"{"why":"she has nothing left to try here","say":"Nothing. Not even mud."}"#,
            "\n\nRules:\n",
            "- `say` is speech only: no narration, no stage directions, no summary of the scene.\n",
            "- One sentence, at most twenty words.\n",
            "- Never reuse a sentence from the examples above, or from your own memories.\n",
            "- Never repeat a line you have already said in this conversation.\n",
            "- Do not count food out loud unless you are making a demand or an offer.\n",
            "- Say something new about this exact person in this exact place."
        );
        let body = json!({
            "model": model,
            "stream": false,
            "think": false,
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
        let text = resp
            .text()
            .await
            .map_err(|e| VoiceError::Transport(e.to_string()))?;
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
            .to_string();
        let why = parsed
            .get("why")
            .and_then(|s| s.as_str())
            .unwrap_or("")
            .to_string();
        Ok(Line {
            say,
            why,
            model: model.to_string(),
            latency_ms: started.elapsed().as_millis() as u64,
        })
    }

    pub async fn speak_actor(&self, card: &str, speaker: &str) -> Result<Line, VoiceError> {
        let m = self.cfg.actor_model.clone();
        self.speak(card, &m, speaker).await
    }

    /// Vary sampling per agent. Nine calls at one temperature converge on the
    /// same phrasing; the offset is derived from the name so runs stay
    /// reproducible.
    fn temperature_for(name: &str, base: f32) -> f32 {
        let h: u32 = name.bytes().map(|b| b as u32).sum();
        base + (h % 5) as f32 * 0.05
    }

    pub async fn speak_elder(&self, card: &str, speaker: &str) -> Result<Line, VoiceError> {
        let m = self.cfg.elder_model.clone();
        self.speak(card, &m, speaker).await
    }

    /// Liveness only; the simulator does not require Ollama to be up, but the
    /// dev server reports it so a broken setup is visible before a run.
    pub async fn probe(&self) -> Result<Vec<String>, VoiceError> {
        let resp = self
            .client
            .get(format!("{}/api/tags", self.cfg.base_url))
            .send()
            .await
            .map_err(|e| VoiceError::Transport(e.to_string()))?;
        let v: Value = resp
            .json()
            .await
            .map_err(|e| VoiceError::Malformed(e.to_string()))?;
        Ok(v.get("models")
            .and_then(|m| m.as_array())
            .map(|ms| {
                ms.iter()
                    .filter_map(|m| m.get("name").and_then(|n| n.as_str()).map(str::to_string))
                    .collect()
            })
            .unwrap_or_default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn schema_puts_reasoning_before_the_answer() {
        let s = Voice::schema();
        let keys: Vec<&str> = s["properties"].as_object().unwrap().keys().map(|s| s.as_str()).collect();
        assert_eq!(keys, vec!["why", "say"]);
        assert_eq!(s["required"].as_array().unwrap().len(), 2);
    }
}
