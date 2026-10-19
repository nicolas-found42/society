//! Jev client. One HTTP call shape: `{model, state, questions}` in, typed
//! answers out. Every question asked here is a close-ended judgment; anything
//! deterministic is computed in `society-sim` instead.

use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

const DEFAULT_ENDPOINT: &str = "https://openrouter.ai/api/alpha/decisions";
const MODEL: &str = "typesafe/jev-1.14";

#[derive(Debug, Clone)]
pub struct JudgeConfig {
    pub endpoint: String,
    pub api_key: String,
    pub model: String,
    pub timeout: Duration,
}

impl JudgeConfig {
    /// # Errors
    ///
    /// Returns `JudgmentError::MissingKey` when `OPENROUTER_API_KEY` is unset.
    pub fn from_env() -> Result<Self, JudgmentError> {
        let api_key =
            std::env::var("OPENROUTER_API_KEY").map_err(|_missing| JudgmentError::MissingKey)?;
        let endpoint =
            std::env::var("SOCIETY_JEV_ENDPOINT").unwrap_or_else(|_| DEFAULT_ENDPOINT.to_owned());
        let model = std::env::var("SOCIETY_JEV_MODEL").unwrap_or_else(|_| MODEL.to_owned());
        Ok(Self {
            endpoint,
            api_key,
            model,
            timeout: Duration::from_secs(45),
        })
    }
}

#[derive(Debug)]
pub enum JudgmentError {
    MissingKey,
    Transport(String),
    Status(u16, String),
    Malformed(String),
}

impl std::fmt::Display for JudgmentError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingKey => write!(f, "OPENROUTER_API_KEY is not set"),
            Self::Transport(e) => write!(f, "judgment transport: {e}"),
            Self::Status(c, body) => write!(f, "judgment HTTP {c}: {body}"),
            Self::Malformed(e) => write!(f, "judgment response malformed: {e}"),
        }
    }
}

impl std::error::Error for JudgmentError {}

/// Typed answer, reduced to what the simulator consumes.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Judgment {
    Choice {
        choice: String,
        probabilities: Map<String, Value>,
        confidence: f32,
    },
    Noul {
        noul: f32,
    },
    Score {
        score: f32,
        confidence: f32,
    },
}

/// Captures an agent's inner hesitation when their runner-up action had nearly
/// the same probability as the chosen action.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Dilemma {
    pub chosen: String,
    pub alternative: String,
    pub chosen_prob: f32,
    pub alternative_prob: f32,
}

impl Judgment {
    #[must_use]
    pub fn choice(&self) -> Option<&str> {
        match self {
            Self::Choice { choice, .. } => Some(choice),
            _ => None,
        }
    }

    #[must_use]
    pub const fn noul(&self) -> Option<f32> {
        match self {
            Self::Noul { noul } => Some(*noul),
            _ => None,
        }
    }

    #[must_use]
    pub const fn score(&self) -> Option<f32> {
        match self {
            Self::Score { score, .. } => Some(*score),
            _ => None,
        }
    }

    #[must_use]
    pub const fn confidence(&self) -> Option<f32> {
        match self {
            Self::Choice { confidence, .. } | Self::Score { confidence, .. } => Some(*confidence),
            Self::Noul { .. } => None,
        }
    }

    /// Sorted `(option, probability)` pairs in descending order for a choice.
    #[must_use]
    pub fn top_alternatives(&self) -> Vec<(String, f32)> {
        match self {
            Self::Choice { probabilities, .. } => {
                let mut pairs: Vec<(String, f32)> = probabilities
                    .iter()
                    .filter_map(|(k, v)| v.as_f64().map(|p| (k.clone(), p as f32)))
                    .collect();
                pairs.sort_by(|a, b| {
                    b.1.partial_cmp(&a.1)
                        .unwrap_or(std::cmp::Ordering::Equal)
                        .then_with(|| a.0.cmp(&b.0))
                });
                pairs
            },
            _ => Vec::new(),
        }
    }

    /// Shannon entropy (in nats) of a choice distribution, measuring how torn
    /// the decision was across all legal affordances.
    #[must_use]
    pub fn entropy(&self) -> Option<f32> {
        match self {
            Self::Choice { probabilities, .. } => {
                let h: f32 = probabilities
                    .values()
                    .filter_map(|v| v.as_f64().map(|p| p as f32))
                    .filter(|&p| p > 1e-6)
                    .map(|p| -p * p.ln())
                    .sum();
                Some(h)
            },
            _ => None,
        }
    }

    /// Returns `Some(Dilemma)` when the runner-up choice is within `margin`
    /// probability of the winning choice and holds at least `0.20` mass.
    #[must_use]
    pub fn dilemma(&self, margin: f32) -> Option<Dilemma> {
        let chosen = self.choice()?;
        let ranked = self.top_alternatives();
        let (_, top_p) = ranked.first()?;
        let (alt, alt_p) = ranked.iter().find(|(k, _)| k != chosen)?;
        ((*top_p - *alt_p).abs() <= margin && *alt_p >= 0.20).then(|| Dilemma {
            chosen: chosen.to_owned(),
            alternative: alt.clone(),
            chosen_prob: *top_p,
            alternative_prob: *alt_p,
        })
    }
}

/// A completed call, with the accounting the observer UI shows.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Answer {
    pub answers: Map<String, Value>,
    pub model: Option<String>,
    pub cost: f64,
    pub input_tokens: u64,
    pub latency_ms: u64,
    /// Echo of the request, kept so a turn can be replayed exactly.
    pub state: Value,
    pub questions: Value,
}

impl Answer {
    #[must_use]
    pub fn get(&self, id: &str) -> Option<Judgment> {
        let v = self.answers.get(id)?;
        let ty = v.get("type")?.as_str()?;
        match ty {
            "choice" => Some(Judgment::Choice {
                choice: v.get("choice")?.as_str()?.to_owned(),
                probabilities: v
                    .get("probabilities")
                    .and_then(|p| p.as_object())
                    .cloned()
                    .unwrap_or_default(),
                confidence: v.get("confidence")?.as_f64()? as f32,
            }),
            "noul" => Some(Judgment::Noul {
                noul: v.get("noul")?.as_f64()? as f32,
            }),
            "score" => Some(Judgment::Score {
                score: v.get("score")?.as_f64()? as f32,
                confidence: v.get("confidence").and_then(Value::as_f64).unwrap_or(0.0) as f32,
            }),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Judge {
    cfg: JudgeConfig,
    client: reqwest::Client,
}

impl Judge {
    /// # Panics
    ///
    /// Panics if the HTTP client cannot be built.
    #[must_use]
    pub fn new(cfg: JudgeConfig) -> Self {
        let client = reqwest::Client::builder()
            .timeout(cfg.timeout)
            .build()
            .expect("client builds");
        Self { cfg, client }
    }

    /// # Errors
    ///
    /// Returns `JudgmentError::MissingKey` when `OPENROUTER_API_KEY` is unset.
    pub fn from_env() -> Result<Self, JudgmentError> {
        Ok(Self::new(JudgeConfig::from_env()?))
    }

    /// One batched request. All questions see the same state and are answered
    /// independently; this is the only call shape the simulator uses.
    ///
    /// # Errors
    ///
    /// Returns a `JudgmentError` on transport failure or an unusable response.
    pub async fn ask(
        &self,
        state: Value,
        questions: Map<String, Value>,
    ) -> Result<Answer, JudgmentError> {
        let body = json!({
            "model": self.cfg.model,
            "state": state,
            "questions": questions,
        });
        let started = std::time::Instant::now();
        let resp = self
            .client
            .post(&self.cfg.endpoint)
            .bearer_auth(&self.cfg.api_key)
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| JudgmentError::Transport(e.to_string()))?;

        let status = resp.status();
        let text = resp
            .text()
            .await
            .map_err(|e| JudgmentError::Transport(e.to_string()))?;
        if !status.is_success() {
            return Err(JudgmentError::Status(status.as_u16(), truncate(&text, 400)));
        }
        let v: Value =
            serde_json::from_str(&text).map_err(|e| JudgmentError::Malformed(e.to_string()))?;
        let answers = v
            .get("answers")
            .and_then(|a| a.as_object())
            .cloned()
            .ok_or_else(|| JudgmentError::Malformed("no answers object".into()))?;
        let usage = v.get("usage").cloned().unwrap_or(Value::Null);
        Ok(Answer {
            answers,
            model: v.get("model").and_then(|m| m.as_str()).map(str::to_owned),
            cost: usage.get("cost").and_then(Value::as_f64).unwrap_or(0.0),
            input_tokens: usage
                .get("input_tokens")
                .and_then(Value::as_u64)
                .unwrap_or(0),
            latency_ms: started.elapsed().as_millis() as u64,
            state: body["state"].clone(),
            questions: body["questions"].clone(),
        })
    }

    /// Prove the layer is reachable before the society starts. Cheap, one
    /// question, and a hard failure: there is no degraded mode.
    ///
    /// # Errors
    ///
    /// Returns a `JudgmentError` if the layer cannot answer the probe question.
    pub async fn probe(&self) -> Result<(), JudgmentError> {
        let mut qs = Map::new();
        qs.insert(
            "ok".into(),
            json!({
                "type": "noul",
                "instructions": "Is this a request to the judgment layer?"
            }),
        );
        let a = self.ask(json!("society probe"), qs).await?;
        if a.get("ok").and_then(|j| j.noul()).is_none() {
            return Err(JudgmentError::Malformed("probe answer missing".into()));
        }
        Ok(())
    }
}

fn truncate(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

/// Build a choice question from the exact action ids the simulator offers.
///
/// Order is left as given; measurement showed no ordering effect once the
/// state is well-formed, so the caller's order is the auditable one.
#[must_use]
pub fn action_question(instructions: &str, actions: &[String]) -> Value {
    let mut criteria = Map::new();
    for a in actions {
        criteria.insert(a.clone(), json!(a));
    }
    json!({ "type": "choice", "instructions": instructions, "criteria": criteria })
}

/// Build a choice question with explicit per-option descriptions.
#[must_use]
pub fn choice_question(instructions: &str, options: &[(&str, &str)]) -> Value {
    let mut criteria = Map::new();
    for (key, desc) in options {
        criteria.insert((*key).to_owned(), json!(desc));
    }
    json!({ "type": "choice", "instructions": instructions, "criteria": criteria })
}

/// Build a 5-point ordinal score question.
#[must_use]
pub fn score_question(instructions: &str, labels: [&str; 5]) -> Value {
    json!({ "type": "score", "instructions": instructions, "criteria": labels })
}

/// Build a boolean/credibility `noul` question.
#[must_use]
pub fn noul_question(instructions: &str) -> Value {
    json!({ "type": "noul", "instructions": instructions })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn action_question_enumerates_every_affordance() {
        let acts = vec![
            "speak".to_owned(),
            "take".to_owned(),
            "go:market".to_owned(),
        ];
        let q = action_question("Which?", &acts);
        let c = q["criteria"].as_object().unwrap();
        assert_eq!(c.len(), 3);
        for a in &acts {
            assert!(c.contains_key(a));
        }
    }

    #[test]
    fn parse_choice_answer() {
        let mut a = Map::new();
        a.insert(
            "i".into(),
            json!({"type":"choice","choice":"take","probabilities":{"take":0.8,"speak":0.2},"confidence":0.7}),
        );
        let ans = Answer {
            answers: a,
            model: None,
            cost: 0.0,
            input_tokens: 0,
            latency_ms: 0,
            state: Value::Null,
            questions: Value::Null,
        };
        match ans.get("i").unwrap() {
            Judgment::Choice {
                choice, confidence, ..
            } => {
                assert_eq!(choice, "take");
                assert!((confidence - 0.7).abs() < 1e-6);
            },
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn malformed_answer_is_not_a_silent_pass() {
        let mut a = Map::new();
        a.insert("i".into(), json!({"type":"noul","noul":"yes"}));
        let ans = Answer {
            answers: a,
            model: None,
            cost: 0.0,
            input_tokens: 0,
            latency_ms: 0,
            state: Value::Null,
            questions: Value::Null,
        };
        assert!(ans.get("i").is_none());
    }

    #[test]
    fn detects_dilemma_when_top_two_actions_are_close() {
        let mut probs = Map::new();
        probs.insert("give:Bo".into(), json!(0.44));
        probs.insert("eat".into(), json!(0.39));
        probs.insert("speak".into(), json!(0.17));
        let j = Judgment::Choice {
            choice: "give:Bo".into(),
            probabilities: probs,
            confidence: 0.42,
        };
        let d = j
            .dilemma(0.12)
            .expect("close split should surface a dilemma");
        assert_eq!(d.chosen, "give:Bo");
        assert_eq!(d.alternative, "eat");
        assert!(j.entropy().unwrap() > 0.9);
    }

    #[test]
    fn clear_favorite_produces_no_dilemma() {
        let mut probs = Map::new();
        probs.insert("eat".into(), json!(0.85));
        probs.insert("speak".into(), json!(0.15));
        let j = Judgment::Choice {
            choice: "eat".into(),
            probabilities: probs,
            confidence: 0.9,
        };
        assert!(j.dilemma(0.12).is_none());
    }
}
