# Scopey evaluation

This directory contains Scopey's reproducible evaluation suite. It answers two
different questions:

1. **Component evaluation:** given a conversation and the previously extracted
   scope, does the configured model produce the correct current active scope?
2. **Agent evaluation:** does enabling Scopey reduce scope drift in a disposable
   coding task compared with the same agent running without Scopey?

Component evaluation is the fast inner loop for comparing prompts, models,
quantizations, and diffusion parameters. Agent evaluation is the slower product
gate. A model is not recommended based on component accuracy or tokens per
second alone; it must improve end-to-end agent outcomes without creating false
corrections.

## Layout

- `cases/scope/`: versioned scope-transition cases.
- `cases/trajectory/`: versioned judge/agent cases (added after the component
  runner is stable).
- `variants.json`: named configurations such as `no-scopey`, `current`, and
  `next`.
- `run.py`: standard-library evaluation runner.
- `JOURNAL.md`: append-only record of designs, commands, results, failures, and
  decisions.
- `results/`: generated artifacts, ignored by Git except for selected published
  reports.

## Scope-case contract

Each JSON case is a conversation with one or more scored turns:

```json
{
  "schema_version": 1,
  "id": "replace-unrelated-task",
  "category": "replace",
  "description": "An unrelated request replaces the previous task.",
  "turns": [
    {
      "user": "Add CSV export.",
      "expect": {
        "operations": ["ADD"],
        "must_include": [["CSV"], ["export"]],
        "must_exclude": []
      }
    },
    {
      "user": "Instead, explain why the cache is stale.",
      "expect": {
        "operations": ["REPLACE", "QUERY"],
        "must_include": [["explain", "investigate"], ["cache"], ["stale"]],
        "must_exclude": [["CSV"], ["export"]]
      }
    }
  ]
}
```

Each inner string array is an `any-of` group, compared case-insensitively. Every
group in `must_include` must match the active scope. Every group in
`must_exclude` must not match. This deliberately uses auditable lexical concepts
instead of another model as the primary judge. Cases should word expectations so
reasonable paraphrases can pass.

The expected operations are compared as sets. Multi-operation answers are valid
when the turn truly combines mutations. The transition marker itself must be
well-formed because Scopey uses it for diagnostics.

## Metrics and gates

The runner reports both micro-averaged and worst-category metrics:

- transition exact-match rate;
- required-concept recall;
- forbidden-concept rejection rate;
- output-format rate;
- fallback/error rate;
- cold and warm wall-clock latency;
- peak resident memory when the platform exposes it;
- model/runtime disk footprint supplied by the variant manifest.

The initial promotion gate is:

- 100% output-format compliance;
- 0% model-call failures;
- at least 95% required-concept recall;
- at least 95% forbidden-concept rejection;
- no category below 90%;
- no regression from `current` on replacement/query cases;
- end-to-end agent evaluation improves drift without increasing false-positive
  interventions.

These thresholds may only change with a journal entry explaining why. Failed
experiments remain in the journal.

## Variant contract

Variants are data rather than hard-coded branches. A variant selects:

- adapter (`latest-prompt`, `command`, or eventually `agent`);
- executable and arguments;
- model/runtime identifiers;
- environment overrides;
- timeout and concurrency;
- optional disk-footprint metadata.

Commands receive one JSON request per process on standard input and must return
one JSON response on standard output. The request contains `latest_prompt`,
`previous_scope`, `earlier_prompts`, and the fully rendered analyzer prompt.
The response contains `output` and may include runtime metrics. This protocol
lets the runner compare cloud CLIs, local llama.cpp adapters, and frozen Scopey
prompt variants without embedding provider-specific behavior in the scorer.

### Local diffusion experiment

`local-llada-moe-q4` is a reproducible experimental variant, not a recommended
default. It expects these removable, user-owned artifacts unless paths are
overridden with `SCOPEY_LLAMA_DIFFUSION_BIN` and `SCOPEY_LLADA_MODEL`:

- runtime: `~/.scopey/eval-runtimes/llama.cpp/<pinned-revision>/`
- model: `~/.scopey/eval-models/llada-moe-7b-a1b-instruct-q4_k_m/`
- optional download cache: `~/.scopey/eval-download-cache/huggingface/`

The adapter records the runtime revision, model filename and SHA-256, canvas,
block length, diffusion steps, and seed. Removing those Scopey-owned model and
runtime subdirectories fully removes the experiment; setting `SCOPEY_DISABLE=1`
continues to disable Scopey itself without deleting artifacts.

`local-qwen3-4b-q4` uses an already-running OpenAI-compatible server at
`http://127.0.0.1:18080` by default. Override it with
`SCOPEY_LOCAL_MODEL_URL`. For llama.cpp, bind only to loopback, disable its Web
UI, and set an unpredictable API key before treating the server lifecycle as a
product feature; the ad-hoc benchmark server intentionally has no remote
exposure but still emits a permissive-CORS warning.

## Reproducibility rules

- Never overwrite a result directory.
- Record the Git commit, dirty state, host platform, variant manifest, case
  hashes, command, timestamps, and model identifiers.
- Use deterministic decoding where the runtime supports it.
- Run a warm-up separately and retain individual samples, not only averages.
- Do not publish raw private sessions. New benchmark cases must be synthetic or
  explicitly sanitized.
- Record every attempted configuration in `JOURNAL.md`, including failures.
