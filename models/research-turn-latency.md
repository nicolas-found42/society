# Turn latency: where the time goes and what actually shortens it

Measured on this machine, 2026-10-19, against a running `society --headless`.
Every number below came from a live turn or a direct `/api/chat` call; nothing
is estimated. The question was "how can we speed up the rounds?"

## Where a turn's time goes

One committed turn is strictly sequential at three points:

1. **Intent** — one batched OpenRouter Decisions call. Measured 164–394 ms.
2. **Voice** — one `/api/chat` per living villager, concurrent under the AIMD
   semaphore. Wall time is the *max* line, not the sum.
3. **Gate** — one batched Decisions call, and it cannot start until every line
   exists because it judges the spoken text. Measured 223–438 ms.

Measured across nine turns (cloud voice, `think: low`):

| Stage | Mean | Share of turn |
|---|---|---|
| Intent | 304 ms | ~6% |
| Gate | 307 ms | ~6% |
| Voice (max line) | 4365 ms | ~88% |

Judgment is not the problem and cannot be overlapped without changing what the
gate judges. **Voice is the whole cost.**

## Voice cost is queueing, not generation

Instrumented through a logging proxy in front of the daemon, one real turn
(`/tmp/proxy.log`):

| Model | App-observed wall | Model `total_duration` | Ratio |
|---|---|---|---|
| `beam:cloud` | 2032 ms | 757 ms | 2.7× |
| `beam:cloud` | 3150 ms | 801 ms | 3.9× |
| `beam:cloud` | 5313 ms | 2084 ms | 2.5× |
| `deepseek-v4.1-flash:cloud` | 3059 ms | 826 ms | 3.7× |

The model reports finishing in 0.76–2.08 s while the client waits 2.0–5.3 s.
The difference is the account's admission control: the response headers declare
`X-Ratelimit-Max-Concurrent: 3`, `X-Ratelimit-Queue-Limit: 10`. Nine lines
therefore run in three serialized waves, and each wave also pays the daemon's
queueing.

Direct concurrency scaling on `beam:cloud`, same card, `think: low`:

| Concurrent calls | Wall | Max call | Errors |
|---|---|---|---|
| 1 | 0.48 s | 0.48 s | 0 |
| 3 | 0.80 s | 0.66 s | 0 |
| 6 | 2.41 s | 0.92 s | 0 |
| 9 | 3.12 s | 1.00 s | 0 |
| 12 | 5.15 s | 1.01 s | 5 (429) |

Wall time grows ~6× from 1 to 12 calls while the slowest individual call stays
near 1 s. That is the signature of a fixed server-side concurrency window, not
of slow generation. No client setting can exceed it.

## Levers, measured on the two axes that matter

The earlier comparison (local at `think:false` vs cloud at `think:low`) changed
two variables at once. Running the missing cell separates them:

| Configuration | Turn wall time | Voice max | Basis |
|---|---|---|---|
| Cloud (`beam`/`deepseek`), `think: low` | **4.3–6.2 s** | 3.3–4.4 s | 10 live turns |
| Local `qwen3:0.6b`, `think: false` | **2.1–2.2 s** | 1.5–1.8 s | 5 live turns |
| Local `qwen3:0.6b`, `think: true` | **11.2–12.5 s** | 3.0–11.5 s | 4 live turns |

**Locality is not the lever; reasoning is.** At equal reasoning (on), local is
2–2.5× *slower* than cloud. The fast local result comes from reasoning being
off, which `qwen3:0.6b` supports (`/api/show` values `[false, true]`) and
`beam:cloud` does not (it ignores `false` and reasons anyway).

No cloud model in the tested set gives both good lines and reasoning off:
`beam` ignores `false` (1593 reasoning chars, 430 eval tokens);
`deepseek-v4.1-flash` honors it (0 reasoning chars, 25 eval tokens).

| Lever | Effect | Evidence |
|---|---|---|
| Reasoning off, where the model allows it | **4.3–6.2 s → 2.1–2.2 s** (local, `qwen3:0.6b`) | 5 live turns; also 0.09–0.16 s per line warm, 21–40 tokens |
| Prompt instruction to reason briefly | **None** | reasoning 799→851 chars, eval 236→241 tokens |
| `num_predict` | Cost guardrail | Completed lines end `done_reason: stop` |
| Streaming | None on total | Gate needs the complete line |
| Concurrency / `--permits` | None | Account caps at 3 concurrent regardless of client permits |
| Batching villagers into one voice call | Untested structural lever | One wave instead of three, but the gate scores each line; A/B would be needed |

## Residual: a budget can starve the answer

One proxied line finished `done_reason: length` with `eval_count: 768` and
`content: ""` — every token went to reasoning, and the turn aborted. Observed
reasoning use at `think: low` reaches 630 tokens, so the default budget is now
1024. A second failure mode surfaced the same way: a reply whose JSON lacked
`say` was committed as a silent villager; it is now an error so the retry runs
and the failing villager is named.

## Recommendation

Two configurations are defensible, and the choice is reasoning, not locality:

- **Rate**: local `qwen3:0.6b` with `SOCIETY_ACTOR_THINK=false`. Measured
  2.1–2.2 s per turn, no account queue, no 429 path, no per-call billing. This
  is only fast because a 0.6B with reasoning off generates ~30 tokens per line.
- **Quality**: cloud `beam`/`deepseek` at `think: low`, 4.3–6.2 s per turn. The
  lines are noticeably better. Reasoning cannot be disabled on `beam`, and
  enabling it on the local model makes local the slow option (11–12 s).

Do not describe the local result as "the local model is faster": at equal
reasoning settings it is 2–2.5× slower. The speed comes from reasoning off,
which only some models allow.

## Sources

- Ollama chat API schema (`think` accepts boolean or model level name;
  `num_predict` is a generation cap; `keep_alive` is server-side):
  https://docs.ollama.com/api/chat
- Ollama OpenAPI specification: https://github.com/ollama/ollama/blob/main/docs/openapi.yaml
- Per-model think levels and defaults (`/api/show`):
  `curl -s localhost:11434/api/show -d '{"model":"beam:cloud"}'`
- Cloud concurrency limits: response headers
  `X-Ratelimit-Max-Concurrent`, `X-Ratelimit-Queue-Limit`,
  `Retry-After` on 429.
