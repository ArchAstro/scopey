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

