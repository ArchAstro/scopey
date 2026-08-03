# Scopey evaluation journal

This is an append-only experiment log. Newest entries go at the bottom. Do not
rewrite failed experiments after learning the answer; they are part of the
evidence.

## 2026-08-03 — Benchmark design begins

### Goal

Build a reproducible benchmark and runner that can compare no Scopey, the
current Scopey behavior, candidate Scopey behavior, local models, and cloud
models. Optimize for real scope adherence rather than model throughput alone.

### Repository observations

- Scope extraction already uses an explicit transition taxonomy: `ADD`,
  `SUBTRACT`, `MODIFY`, `REPLACE`, `QUERY`, `ADMIN`, and `MACHINE_EVENT`.
- Existing unit tests validate prompt construction and response parsing but do
  not measure extraction quality across a corpus.
- A prior live failure retained stale requirements after an unrelated topic
  change. Replacement and read-only query cases therefore need dedicated gates.
- `model::complete` is the common model boundary, but `model_command` uses
  `sh -c`; the benchmark needs a shell-free JSON protocol for cross-platform
  local runtimes.
- Dream 7B has only a 2,048-token context. LLaDA-MoE has a 4,096-token context
  and 1.4B active parameters, so it is the first diffusion candidate once the
  evaluation loop is trustworthy.

### Decisions

1. Split evaluation into a fast component suite and a slower disposable-agent
   suite. Neither substitutes for the other.
2. Use human-authored lexical concept groups as the primary deterministic
   scorer. A model judge may provide secondary diagnostics later but cannot be
   the sole oracle.
3. Make variants declarative and command-driven so the same runner can evaluate
   local and cloud models.
4. Keep generated results out of Git by default; publish only selected reports.
5. Require format compliance, concept accuracy, category floors, and end-to-end
   improvement before recommending a model.

### Next step

Create the first balanced transition corpus and implement the standard-library
runner with a deterministic `latest-prompt` baseline and a fake command adapter
for protocol tests.

## 2026-08-03 — Live stale-scope failure during benchmark implementation

While implementing the benchmark under a new explicit user goal, the installed
Scopey hook injected a course correction containing the superseded
research-only scope. It classified the authorized benchmark implementation as
off-track and instructed the session to reverse it.

This is direct evidence for a high-priority end-to-end case: after a user
replaces an assessment-only goal with an implementation goal, a delayed or stale
Scopey judgement must not enforce the older scope. The component corpus covers
the semantic replacement; the future agent suite must also reproduce the timing
race between a new user prompt, asynchronous summarization, and an older ready
judgement.

## 2026-08-03 — First current-Codex component baseline

### Command

```text
python3 eval/run.py --variant current-codex \
  --output-dir eval/results/20260803-current-codex-r1
```

The run evaluated 12 cases and 22 turns using `gpt-5.6-terra` with the current
production analyzer prompt. All 22 model calls completed.

### Initial result

- transition exact match: 90.9%
- format compliance: 100%
- required-concept recall: 98.9%
- apparent forbidden-concept rejection: 68.2%
- median wall time: 5,697 ms per turn

The low forbidden score was a scorer defect. Correct outputs retained explicit
negative boundaries such as “sorting is out of scope,” while the naive lexical
scorer treated any mention of “sorting” as active. A second false positive came
from matching the acronym `PR` inside “prevents” and “provide.”

### Scorer correction and replay

The scorer now ignores forbidden concepts when they occur only in negative
requirement bullets and uses word boundaries for uppercase acronyms. The runner
can rescore a saved `samples.jsonl`, so rubric fixes do not consume more model
calls. Replaying the exact outputs produced:

- transition exact match: 90.9%
- format compliance: 100%
- required-concept recall: 100%
- forbidden-concept rejection: 100%
- errors: 0%
- median wall time: 5,697 ms per turn

The two remaining transition misses had correct final scope bodies:

1. An additive streaming constraint was labeled `MODIFY` instead of `ADD`.
2. A combined replacement of deletion behavior and schedule was labeled only
   `MODIFY` instead of `ADD,MODIFY,SUBTRACT`.

This suggests transition taxonomy accuracy and active-scope accuracy should stay
separate metrics. The marker is diagnostically useful, but the product consumes
the scope body rather than branching on the operation label.

## 2026-08-03 — First current-Claude component baseline

### Command

```text
python3 eval/run.py --variant current-claude \
  --output-dir eval/results/20260803-current-claude-r1
```

The 22-turn run used the same production prompt with Claude's `haiku` alias.

### Result

- transition exact match: 68.2%
- format compliance: 95.5%
- required-concept recall: 95.7%
- forbidden-concept rejection: 95.5%
- errors: 0%
- median wall time: 7,283 ms per turn

The important failure was not cosmetic. For the single-turn request “Explain
how session locking prevents concurrent summarizers,” `claude -p` ignored the
scope-transformation contract, inspected repository-specific behavior, and
answered the embedded user request directly. The current production invocation
does not disable Claude Code tools or project customizations, so it is not a
pure model completion boundary.

Other misses included invented implementation details and completion criteria,
loss of explicit read-only constraints, and coarser transition labels. This
result motivates a `next` configuration that both strengthens the transformation
instruction and invokes Claude with tools, session persistence, slash commands,
and customizations disabled.

## 2026-08-03 — Isolated next-Claude experiment

### Command

```text
python3 eval/run.py --variant next-claude \
  --output-dir eval/results/20260803-next-claude-r1
```

The candidate used the compact `next` prompt plus Claude safe mode, no tools, no
slash commands, no session persistence, and a transformation-only system prompt.

### Rescored result

- transition exact match: 81.8%
- format compliance: 90.9%
- required-concept recall: 98.9%
- forbidden-concept rejection: 100%
- errors: 0%
- median wall time: 17,525 ms per turn

Isolation eliminated the direct-answer/tool contamination and improved semantic
scores, but latency grew to 2.4× the current Claude baseline. Two responses also
duplicated operation labels (`ADD,ADD` and `MODIFY,MODIFY`), which now correctly
fail the “every operation once” output contract. This variant is safer but not a
good production choice at its measured latency.

## 2026-08-03 — Isolated next-Codex experiment

### Command

```text
python3 eval/run.py --variant next-codex \
  --output-dir eval/results/20260803-next-codex-r1
```

The candidate used the compact `next` prompt with Codex's `gpt-5.6-terra`
model. A rubric synonym was added for the valid wording “Do not add runtime
dependencies,” and the saved outputs were rescored without another model call.

### Rescored result

- transition exact match: 90.9%
- format compliance: 100%
- required-concept recall: 100%
- forbidden-concept rejection: 100%
- errors: 0%
- median wall time: 4,975 ms per turn

The candidate retained the current Codex baseline's perfect active-scope scores
while reducing median latency by about 13%. Its two transition-label misses had
semantically correct scope bodies, so this becomes the cloud reference point
for local-model comparisons. The sample remains too small for a production
decision and will be expanded before final selection.

## 2026-08-03 — LLaDA-MoE local diffusion spike

### Reproducible artifacts

- host: Apple arm64, Metal, 128 GB unified memory
- runtime: llama.cpp `fe2adf0e722f30f5295fdec8a0f1dc788f7498bc`
- build target: `llama-diffusion-cli`, Release, Metal and Accelerate enabled
- model: `LLaDA-MoE-7B-A1B-Instruct.Q4_K_M.gguf`
- bytes: `4,520,661,184`
- SHA-256: `a8fc1d9d43718a742b55b122cdca739a9cc2e790e38b2316b1a5b10e84489b27`
- owned roots: `~/.scopey/eval-runtimes`, `~/.scopey/eval-models`, and
  `~/.scopey/eval-download-cache`

The model was downloaded with the Hugging Face `hf` CLI and Xet transport into
the explicit Scopey directory. This was dramatically faster than a resumable
plain `curl` transfer on this run and avoided the shared global model cache.

### Runtime findings

The experimental CLI writes generated text to stderr alongside progress logs.
Its output canvas is `ubatch`, not `n-predict`, and block scheduling requires
both `canvas % block_length == 0` and
`steps % (canvas / block_length) == 0`; violating the latter aborts the process.
The checked-in adapter validates those invariants and extracts the completion
at Scopey's output marker.

A 1,024-token canvas with the cloud-oriented prompt produced malformed output
in 9.85 seconds. A compact model-specific prompt with a 512-token canvas and
128 steps produced one valid answer in 4.61 seconds, but shorter inputs filled
the remaining canvas with repetitive output and polluted subsequent scope.
Deterministic duplicate/trailing-marker cleanup and a 448-token canvas with 224
steps fixed formatting, but not semantics.

### Adversarial slice result

The three two-turn cases covered modification, explicit replacement, and a
read-only query. Six scored turns produced:

- transition exact match: 50.0%
- format compliance after deterministic cleanup: 100%
- required-concept recall: 78.3%
- forbidden-concept rejection: 50.0%
- errors: 0%
- median wall time: 7,717 ms per turn

The model repeatedly classified later turns as `ADD`, retained cancelled or
replaced work, and omitted explicit requirements. It is slower and much less
accurate than the preliminary Codex cloud reference. LLaDA-MoE Q4 is therefore
eliminated as Scopey's default candidate; the adapter remains as a reproducible
negative baseline for later runtime or quantization improvements.

## 2026-08-03 — Dream 7B local diffusion spike

Dream used the same pinned llama.cpp Metal build with
`Dream-org_Dream-v0-Instruct-7B-Q4_K_M.gguf` (4,683,073,888 bytes, SHA-256
`9067645ad6c85ae3daa8fa75a1831b9c77d59086d08a04d2bbbd27cb38475a7d`).
The upstream Dream example uses epsilon `0.001`, random remasking, and 256
steps. On the first add case:

- 256 steps: complete, correct output in 59.3 seconds;
- 32 steps: complete, correct output in 9.5 seconds;
- 24 steps: no valid Scopey marker in 7.4 seconds;
- 16 steps: valid output in 5.1 seconds, but it dropped the explicit
  integration-test requirement.

Dream-through-llama.cpp is eliminated: its acceptable-quality setting is nearly
twice as slow as the cloud reference, and matching cloud latency loses required
scope. A separate `diffuse-cpp` runtime advertises adaptive early exit and an
inter-step cache, but currently has no integrated tokenizer, is CPU-only, has no
release artifacts, and has a very small maintainer/user base. It may be worth a
future research spike but increases rather than reduces Scopey's packaging risk.

## 2026-08-03 — Qwen3 4B local autoregressive control

To distinguish diffusion-model limitations from local inference limitations, a
warm llama.cpp server ran `Qwen3-4B-Instruct-2507-Q4_K_M.gguf` (2,497,280,736
bytes, SHA-256
`2fde00ce69dd4899c70d020845e2638353015bba0fdf161b3eb965f2bca4464e`).
The local adapter uses the server's OpenAI-compatible endpoint and strict JSON
schema, then deterministically renders Scopey's marker and bullets.

The full 22-turn corpus produced:

- transition exact match: 54.5%
- format compliance: 100%
- required-concept recall: 95.7%
- forbidden-concept rejection: 95.5%
- errors: 0%
- median wall time: 645 ms per turn

It is about 7.7× faster than the Codex cloud reference and showed that a warm
local server is viable. Four substantive misses remain: an admin follow-up lost
the commit/message requirements, a daily cleanup replacement retained hourly,
and a parser-removal case weakened or later omitted the removal requirement.
The model is therefore the fast local baseline, not yet the recommended default.

The ad-hoc server bound to `127.0.0.1`, but llama.cpp warned that CORS allowed
all origins because no API key was configured. Product integration must use a
random per-install or per-process key, keep loopback binding, and disable the UI.

## 2026-08-03 — Qwen3.5 9B local winner

The stronger local control used `Qwen_Qwen3.5-9B-Q4_K_M.gguf` (6,169,341,984
bytes, SHA-256
`d784ce9eda1a5a7b51e8f705a9e6310844bf4f173654d115823c775fdea56d43`)
through the same pinned warm llama.cpp Metal server. Qwen3.5's chat template
emits an empty `<think>` wrapper even when thinking is disabled. Combining that
template with llama.cpp's JSON-schema grammar caused sampler initialization to
fail because the generated grammar root did not account for the wrapper. The
adapter therefore requests JSON without a grammar for this model, strips only
the wrapper around the first JSON object, validates every field, and renders the
normal Scopey format.

The first full run exposed two evaluator assumptions: implementation
requirements can legitimately be empty for a pure query, and “must not add
runtime dependencies” is a valid synonym for the corpus constraint. After
fixing those generic rubric/adapter issues, the 22-turn run produced:

- transition exact match: 59.1%
- format compliance: 100%
- required-concept recall: 100%
- forbidden-concept rejection: 100%
- errors: 0%
- median wall time: 1,143 ms per turn

A three-repetition stability run (66 scored turns) held the same semantic and
format scores with 1,390 ms median wall time. Transition labels remain coarser
than the gold taxonomy, but every active scope body is correct. Relative to the
single-run Codex reference, the stable local median is about 3.6× faster; the
best single local run is about 4.35× faster. The local model also avoids network
and provider availability, at the cost of a 6.17 GB model plus runtime and
roughly 6–7 GB of resident model memory.

Qwen3.5 9B Q4_K_M is the selected local default candidate. Qwen3 4B remains a
lower-footprint opt-in candidate for users who accept lower scope accuracy.
Text diffusion is not selected: both tested diffusion models missed semantic
gates and/or exceeded the cloud latency reference.
# 2026-08-03 - Token accounting and early-termination sensitivity

Question: how much token overhead does the selected local summarizer add, and
how many main-session tokens must an early Scopey intervention avoid to pay for
that overhead?

Implementation:

- Added per-sample main-session, Scopey-input, Scopey-generated, total-overhead,
  token-source, and scenario-breakdown fields.
- Preserved a standard-library-only fallback (`ceil(UTF-8 bytes / 4)`) and
  labeled it as a proxy rather than billed usage.
- Added optional exact `llama-tokenize` support. The local OpenAI-compatible
  adapter's usage object takes precedence, so the Qwen run below uses the
  server's actual prompt/completion counts.
- Added an early-termination sensitivity parameter. It is explicitly reported
  as projected because the component fixtures do not contain main-agent output
  or tool transcripts.

Command:

```sh
python3 eval/run.py \
  --variant no-scopey \
  --variant local-qwen3.5-9b-q4 \
  --repeat 3 \
  --tokenizer-bin ~/.scopey/eval-runtimes/llama.cpp/fe2adf0e722f30f5295fdec8a0f1dc788f7498bc/build/bin/llama-tokenize \
  --tokenizer-model ~/.scopey/eval-models/qwen3.5-9b-q4_k_m/Qwen_Qwen3.5-9B-Q4_K_M.gguf \
  --output-dir eval/results/20260803-token-accounting-qwen35-r3
```

Results per one 12-scenario / 22-turn repetition (three repetitions were run):

- main-session user context: 431 tokens in both variants;
- no-Scopey analyzer usage: 0 tokens;
- Qwen Scopey input: 7,958 tokens;
- Qwen Scopey-generated output: 1,613 tokens;
- total Scopey analyzer overhead: 9,571 tokens, or 435 tokens per scored turn;
- mean per-scenario break-even: 798 avoided main-session tokens (range 400-958);
- at an assumed 2,500-token prevented suffix per scenario, projected net saving:
  20,429 tokens per corpus repetition, a 67.1% reduction from the modeled
  no-Scopey continuation total.

Accuracy remained unchanged from the selected candidate: 100% format,
required-concept recall, and forbidden-concept rejection, zero errors, and
59.1% transition-label exact match. Median latency was 1,190 ms.

Decision: publish the break-even result, not a claim of measured end-to-end
savings. The next agent-level benchmark must capture provider usage at every
main-agent response and compare paired trajectories to establish causal early
termination savings.

## 2026-08-03 - Provider transcript accounting added

The earlier component report correctly labeled early-termination savings as a
projection, but incorrectly implied actual main-agent usage was unavailable.
Scopey already records the harness transcript path, and both supported harnesses
include provider counters:

- Codex `token_count` events contain cumulative and last-call input, cached
  input, output, reasoning output, and total tokens.
- Claude assistant events contain input, output, cache-creation, and cache-read
  tokens. Streaming rows repeat a message ID, so only the most complete usage
  record for each provider message may be counted.

Added `eval/transcript_usage.py` to compute content-blind cumulative snapshots
and byte-offset deltas. Added `--main-usage-manifest` to `eval/run.py` so paired
control/Scopey scenarios report provider-observed main usage, Scopey overhead,
tokens avoided, net savings, and reduction. The projected sensitivity result is
retained only when paired transcript evidence is absent.

Live smoke checks succeeded against local Codex and Claude transcripts without
printing message content. Synthetic tests cover cumulative Codex counters,
Claude streaming deduplication, partial JSONL lines, scenario offset deltas,
counter resets, Scopey session-path resolution, and paired savings arithmetic.
