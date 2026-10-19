# Small voice models: candidates, primary-source facts, and measured app results

Question: which models are similar to `qwen3:0.6b`, preferably smaller and
faster, for the voice stage of this app — speed and correctness both matter?

Two halves. **Part 1** is measured here, in this app, on this machine. **Part 2**
is primary-source metadata for the candidates and neighbours, so the shortlist
is traceable rather than vibes.

All measurements are on macOS arm64 (Apple Silicon), Ollama 0.40.6, through the
real `/turn` endpoint with the real production prompt (1072-char system prompt,
~570-char cards, `format` JSON schema, `think: false`, `temperature` 0.95–1.15,
`top_p` 0.95, `num_predict` 1024). Each model ran 12 turns (108 lines) except
where noted. No warm-up turn was discarded; the first turn of each run is
included.

## Part 1 — measured

| Model | Params | Turns | Lines | Empty | Parrots an example | Discredited by gate | Mean fidelity | Mean grounding | Line median | Turn median |
|---|---|---|---|---|---|---|---|---|---|---|
| **`qwen3:0.6b`** (baseline) | 0.75B | 12 | 108 | 0 | **0%** | **21%** | **0.71** | **0.66** | 840 ms | **1456 ms** |
| `granite4:350m-h` | 0.34B | 12 | 108 | 0 | 0% | 34% | 0.60 | 0.64 | 992 ms | 1731 ms |
| `qwen2.5:0.5b` | 0.49B | 12 | 108 | 0 | 3% | 32% | 0.63 | 0.61 | 830 ms | 1430 ms |
| `smollm2:360m` | 0.36B | 12 | 108 | 0 | 14% | 35% | 0.65 | 0.62 | 964 ms | 1796 ms |
| `qwen3.5:0.8b` | 0.85B | 6 | 54 | 0 | 6% | 41% | 0.59 | 0.58 | 1052 ms | 2048 ms |
| `gemma3:270m` | 0.27B | 9 | 81 | 0 | **53%** | 48% | 0.56 | 0.59 | 957 ms | 1293 ms |
| `smollm2:135m` | 0.13B | 3 | 27 | 0 | 15% | 59% | 0.54 | 0.51 | 963 ms | 1514 ms |

**Column meanings.** *Parrots an example* counts a line that is verbatim one of
the three few-shot examples in the system prompt — a failure mode the prompt
explicitly forbids, and the one that makes a village feel broken. *Discredited*
is the app's own Jev gate marking a line as contradicting its decision or the
world; it is the closest thing here to a correctness measure, since a
discredited line cannot move trust. *Fidelity* and *grounding* are that same
gate's scores.

### What the numbers say

1. **Nothing is faster in a way that matters.** The spread in line latency
   (830–1052 ms) and turn time (1.29–2.05 s) is small; turn time is dominated by
   the judgment calls (~600 ms) plus the slowest of nine concurrent lines, not by
   model size. A 270M model does not buy a meaningfully faster round.
2. **Smaller is worse at the task, and the smaller the worse.** Discredited rate
   rises monotonically as parameters fall: 21% at 0.75B, 32–35% at ~0.35B, 48% at
   270M, 59% at 135M.
3. **`qwen3:0.6b` is the only model here that never parrots.** It is the
   baseline for a reason: 0% verbatim example reuse across 108 lines.
4. **`gemma3:270m` is disqualified in practice.** 53% of lines were a verbatim
   few-shot example; in one run the same sentence ("Six measures, Cor…") came
   back for three different villagers in a single turn.
5. **`qwen3.5:0.8b` is slower and worse than the older, smaller `qwen3:0.6b`,**
   despite a much stronger model card. Its latency is also unstable at the turn
   level: across 10 app turns, 8 had a median line near 1.0 s, but two turns
   stalled *as a whole* — 6 of 9 and 7 of 9 lines over 3 s (medians 6500 ms and
   12488 ms). Reproduced deterministically with nine concurrent requests and no
   app in the loop (2 of 10 rounds at 7.3 s and 13.1 s), and measured against
   `qwen3:0.6b` in the same harness: 3 of 8 rounds stalled for qwen3.5, 0 of 8
   for qwen3. See "Runner behaviour" below.
6. **`granite4:350m-h` is the best of the smaller models** on parroting (0%) but
   still misses the gate 34% of the time versus 21%.
7. **`qwen2.5:0.5b` is marginally faster** (1430 ms vs 1456 ms median turn) and
   clearly worse on fidelity (0.63 vs 0.71), with 3% parrot rate.

### Verdict

**Keep `qwen3:0.6b`.** There is no model in this class that is both smaller and
better for this task on this machine; the speed ceiling is the judgment layer,
not the voice model. If a smaller model is mandatory for footprint reasons,
`granite4:350m-h` (0.34B, 366 MB on disk versus qwen's 522 MB) is the only candidate
that does not parrot, at a real cost in gate pass rate.

If raw speed is the actual goal, the lever measured in
`models/research-turn-latency.md` is reasoning-off plus the local model, which
this baseline already uses — not a smaller model.

### Runner behaviour (why qwen3.5 stalls)

`qwen3.5:0.8b` is served by Ollama's **MLX** runner on this machine; every other
candidate here runs on **llama.cpp** (`ollama ps` shows the runner per model).
Nine concurrent requests per round, same prompt and schema, eight rounds each:

| Model | Round wall times | Rounds stalled (>3 s) | Truncated at `num_predict` | Unaccounted time (total − load − prompt − eval) |
|---|---|---|---|---|
| `qwen3:0.6b` (llama.cpp) | 1299–1601 ms | **0 / 8** | 0 / 72 | median 634 ms, max 1421 ms |
| `qwen3.5:0.8b` (MLX) | 1344–6743 ms | **3 / 8** | 3 / 72 | median 571 ms, **max 6314 ms** |

Two separate defects show up in qwen3.5's slow rounds:

1. **Non-zero `load_duration` on 90 of 90 requests** (median 7 ms, up to
   387 ms), versus `qwen3:0.6b` loading on 68 of 72. Some requests are paying to
   touch the model when they should not.
2. **Unaccounted time**: `total_duration` minus load, prompt-eval and eval
   leaves up to 12.5 s unaccounted inside the runner — scheduling/queueing wait,
   not generation. In the stalled rounds the eval phase itself is normal
   (110–210 ms), which is why the whole round stalls together while individual
   generations look fast.

The practical consequence: qwen3.5's ~1.0 s median line is real in the good
rounds, but its mean-plus-tail is 4–5× worse than qwen3:0.6b's. For a village
that must keep ticking, that tail is disqualifying regardless of median speed.

## Part 2 — primary-source metadata

Parameter counts are from `config.json` / the model card; license, HF creation
date, and download counts from the Hugging Face API (`/api/models/<id>`).
IFEval is quoted only where the model card states it, with the card link.

| Model | HF repo | Params | License | HF created | Reasoning mode | Ollama tag | IFEval (card) |
|---|---|---|---|---|---|---|---|
| Qwen3-0.6B | `Qwen/Qwen3-0.6B` | 0.6B | Apache-2.0 | 2025-04-27 | `[false, true]`, disable via `false` | `qwen3:0.6b` | not stated |
| Qwen2.5-0.5B-Instruct | `Qwen/Qwen2.5-0.5B-Instruct` | 0.49B | Apache-2.0 | 2024-09-16 | none | `qwen2.5:0.5b` | not stated |
| SmolLM2-360M-Instruct | `HuggingFaceTB/SmolLM2-360M-Instruct` | 0.36B | Apache-2.0 | 2024-10-31 | none | `smollm2:360m` | 41.0 |
| SmolLM2-135M-Instruct | `HuggingFaceTB/SmolLM2-135M-Instruct` | 0.135B | Apache-2.0 | 2024-10-31 | none | `smollm2:135m` | not stated |
| Granite 4.0-H-350M | `ibm-granite/granite-4.0-h-350m` | 0.34B | Apache-2.0 | 2025-10-07 | none | `granite4:350m-h` | 61.63 (strict, instruct) |
| Gemma 3 270M IT | `google/gemma-3-270m-it` | 0.27B | Gemma Terms of Use | 2025-07-30 | none | `gemma3:270m` | card gated (401) |
| Qwen3.5-0.8B | `Qwen/Qwen3.5-0.8B` | 0.8B | Apache-2.0 | 2026-02-28 | `[false, true]`; card IFEval 52.1 non-thinking | `qwen3.5:0.8b` | 52.1 non-thinking / 44.0 thinking |

Card-stated context lengths: Qwen3-0.6B 40,960; Qwen2.5-0.5B 32,768;
SmolLM2 8,192; Granite 4.0-H-350M 1,048,576; Gemma 3 270M 32,768;
Qwen3.5-0.8B 262,144.

### Baked-in sampling parameters (from `ollama show --modelfile`)

These persist as model defaults and interact with the app, which overrides only
`temperature`, `top_p`, and `num_predict`:

| Tag | Baked parameters |
|---|---|
| `qwen3:0.6b` | `temperature 0.6`, `top_k 20`, `top_p 0.95`, `repeat_penalty 1` |
| `qwen3.5:0.8b` | `temperature 1`, `top_k 20`, `top_p 0.95`, `min_p 0`, `presence_penalty 1.5`, `repeat_penalty 1` |
| `gemma3:270m` | `top_k 64`, `top_p 0.95` |
| `qwen2.5:0.5b`, `smollm2:360m`, `smollm2:135m`, `granite4:350m-h` | none |

So the app's temperature is not the only sampling control in play:
`qwen3.5:0.8b` still runs with `presence_penalty 1.5` and `qwen3:0.6b` with
`top_k 20`. A model comparison that claims to isolate the model must account for
these, which is part of why this document reports app-level outcomes rather than
inventing an equal-footing prompt harness.

### Sources

- Hugging Face model API for license, creation date, downloads:
  `https://huggingface.co/api/models/<repo>` (e.g.
  `https://huggingface.co/api/models/Qwen/Qwen3-0.6B`).
- Model cards, for parameter counts, context length, IFEval:
  `https://huggingface.co/Qwen/Qwen3-0.6B`,
  `https://huggingface.co/Qwen/Qwen2.5-0.5B-Instruct`,
  `https://huggingface.co/HuggingFaceTB/SmolLM2-360M-Instruct`,
  `https://huggingface.co/HuggingFaceTB/SmolLM2-135M-Instruct`,
  `https://huggingface.co/ibm-granite/granite-4.0-h-350m`,
  `https://huggingface.co/google/gemma-3-270m-it`,
  `https://huggingface.co/Qwen/Qwen3.5-0.8B`.
- Ollama tag existence verified against the registry manifest endpoint
  `https://registry.ollama.ai/v2/library/<model>/manifests/<tag>` (HTTP 200),
  and per-model facts from `POST http://127.0.0.1:11434/api/show`
  (`model_info`, `capabilities`, `thinking`, `template`).
- Ollama `think` semantics (boolean or model-defined level; must match
  `/api/show`): https://docs.ollama.com/api/chat

## UNVERIFIED / not settled

- **Gemma 3 270M's model card returned HTTP 401** without authentication, so its
  IFEval and any stated speed figures are unverified; only its license, date, and
  Ollama metadata are sourced here.
- **No card in this set states a tokens/sec figure measured on Apple Silicon.**
  Any speed claim in this document comes from my own measurements, not from a
  vendor.
- **Qwen3.5-0.8B's stalls are localised but not root-caused.** The measured
  facts: MLX runner, non-zero `load_duration` on every request, and up to 12.5 s
  of `total_duration` unaccounted for by load/prompt/eval. Which of reload,
  MLX scheduling, or the vision projector causes it is not established; a
  `keep_alive: -1` pin and an `ollama ps` trace during a stall would settle it.
- **Sub-0.05 fidelity gaps are noise.** Fidelity is one Jev-scored judgment per
  line, and each model was run once over 12 turns (54 for qwen3.5). Treat the
  parrot and discredited counts as the robust columns; do not read fid 0.65 vs
  0.66 as a ranking.
- **The parrot rate is prompt-specific.** It was measured against the current
  three-example system prompt; a different prompt would move it.
- **LFM2.5-350M** — superseded by the update below. It has no official
  Ollama library tag (hf.co route) and its license carries a commercial-revenue
  condition; it is now the default voice model.

## Update (speed round): the queue, not the model, was the ceiling

Later the same day the constraint changed: `qwen3:0.6b` is off the table, speed
first, correctness to be reworked afterwards. Re-measuring exposed a
confounder that invalidates the "speed is flat" reading above.

**Ollama served every earlier run with one parallel slot.** `ollama ps` shows
`llama-server … -np 1`. Nine concurrent voice lines therefore serialize, and
the ~640 ms of "unaccounted" per-request time in the qwen3.5 analysis above is
mostly queue wait, not MLX scheduling. Solo, one line is **72–240 ms**
end-to-end; nine concurrent take **1.2–2.0 s** wall for the slower models (LFM2.5: 650 ms)
— 9× the compute, exactly what one slot predicts. The `Runner behaviour` stall findings for qwen3.5
(6.5–13 s whole-round freezes versus 0 for qwen3 in the same harness) still
stand as an MLX-specific defect; but the small "unaccounted" numbers on every
model are the slot queue.

**Measured with the production prompt** (median of runs; `num_predict` 1024,
`think: false` where supported):

| Model | Solo wall | 9-way wall, np=1 | 9-way wall, np=8 |
|---|---|---|---|
| **LFM2.5-350M** (`hf.co/LiquidAI/…:Q4_K_M`) | **72 ms** | 650 ms | **618 ms** (np=8) |
| gemma3:270m | 107 ms | 1220 ms | 719 ms |
| smollm2:135m | 207 ms | 1968 ms | 803 ms |
| granite4:350m | 242 ms | — | 814 ms |
| qwen2.5:0.5b | 142 ms | — | 746 ms |
| granite4:350m-h | 165 ms | — | 1417 ms |
| qwen3:0.6b (old default) | 124–141 ms | 1463 ms | 1138 ms |
| llama3.2:1b | 216 ms | — | 1295 ms |
| Falcon-H1-0.5B | 724 ms | — | 4243 ms |

End-to-end through the real `/turn` endpoint, 8 turns each, both models on the
same `OLLAMA_NUM_PARALLEL=8` server:

| Model | Turn median | Line median | Discredited | Parrot |
|---|---|---|---|---|
| **LFM2.5-350M** | **744 ms** | 478 ms | 38% | 0 |
| qwen3:0.6b | 964 ms | 784 ms | 21% | 0 |

Decision: **wired LFM2.5-350M as the default voice model** (`actor_model` and
`elder_model` in `VoiceConfig::default()`, reasoning off). Smoke run with pure
defaults against `OLLAMA_NUM_PARALLEL=8`: 3 turns at 659/665/689 ms, 27/27
lines, `/health` reports the model configured. The turn floor is now the
judgment layer (~470 ms), not the voice model.

Caveats carried forward, deliberate under "correctness later":

- LFM2.5-350M discredited 38% of lines in the 8-turn run (qwen3:0.6b: 21%).
  It does not parrot. Correctness work is explicitly deferred.
- No official Ollama library tag; the default is an `hf.co` GGUF route
  (`Q4_K_M`, 354 MB). License has a commercial-revenue condition.
- `OLLAMA_NUM_PARALLEL=8` is a daemon setting; without it the same model runs
  ~2 s turns for slower models (qwen3:0.6b round: 1463→1138 ms, i.e.
  ~1.0 s turn at np=8); LFM2.5 measured 689 ms turns on the stock daemon,
  so the setting barely matters for it.
