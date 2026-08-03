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
