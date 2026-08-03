# Current Codex component baseline — 2026-08-03

Configuration:

- prompt: `eval/prompts/current.txt`
- runner: Codex
- model: `gpt-5.6-terra`
- cases: 12
- scored turns: 22
- repetitions: 1

| Metric | Result |
|---|---:|
| Transition exact match | 90.9% |
| Output format | 100.0% |
| Required-concept recall | 100.0% |
| Forbidden-concept rejection | 100.0% |
| Model-call errors | 0.0% |
| Median wall time per turn | 5,697 ms |

Two answers used a coarser transition label than the gold taxonomy while still
producing the correct active scope. There were no substantive active-scope
failures in this small first corpus. This is a preliminary baseline, not a final
model recommendation: repeated runs, a larger adversarial corpus, local models,
judge classification, and end-to-end agent trials remain outstanding.

Raw outputs are retained locally under
`eval/results/20260803-current-codex-r1/` and are intentionally not committed.
