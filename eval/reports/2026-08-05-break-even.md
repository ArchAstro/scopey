# Scopey cost break-even synthesis

Generated: `2026-08-05T03:46:43.894326+00:00`.
Benchmark summary: `eval/results/v2-benchmark-20260805T025357Z/summary.json`. Session analytics: `eval/reports/2026-08-04-real-sessions-48h-v2.json`.

## Production side (observed window)

- Sessions: 23; judge calls: 220; corrections: 19.
- Scopey analyzer tokens (as accounted by the analytics tool): 7,241,247; main-session tokens: 826,294,493.
- **Fleet overhead per correction: 381,118 raw analyzer tokens** (every session's overhead divided by the corrections it bought).

## Benchmark side (per caught drift)

- Conditional on the control actually continuing drift (4 pairs; unforced continuation rate 7%):
  - main tokens avoided per catch: mean 27,909, median 24,670 (before analyzer overhead);
  - net per benchmark pair: raw -11,212, price-weighted -7,382 (weights {'cached_weight': 0.1, 'output_weight': 8.0, 'analyzer_token_weight': 1.0}).

## Break-even

- Crediting every production correction with the benchmark's mean catch value: 27,909 saved vs 381,118 spent per catch → **ratio 0.07** (>1 means raw-token break-even).
- Corrections would need to prevent at least 381,118 main-session tokens each for Scopey to break even in raw tokens at the observed correction rate.

## Assumptions and caveats

- Raw tokens from different models are added as if fungible; analyzer models are typically cheaper per token than main-session models, and cached main-session reads are cheaper still. Use the weighted figures and your own price sheet before treating the ratio as dollars.
- Production corrections are assumed comparable to benchmark catches; the analytics cannot verify each correction was correct or count the tokens it actually prevented.
- Value beyond tokens (avoided wrong work, review time, trust) is out of scope here and may dominate the decision.
