# Next Codex component baseline — 2026-08-03

Configuration:

- prompt: `eval/prompts/next.txt`
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
| Median wall time per turn | 4,975 ms |

Both transition misses had correct active-scope bodies: one additive follow-up
was classified as `MODIFY`, and one read-only query included an unnecessary
`MODIFY`. The compact candidate is about 13% faster than the current Codex
prompt on this first corpus and serves as the cloud reference for local-model
experiments.

Raw outputs are retained locally under
`eval/results/20260803-next-codex-r1/` and are intentionally not committed.
