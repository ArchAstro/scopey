# Isolated next-Claude experiment — 2026-08-03

Configuration: compact next analyzer prompt, Claude `haiku`, safe mode, tools
disabled, customizations disabled, 12 cases, 22 turns, one repetition.

| Metric | Result |
|---|---:|
| Transition exact match | 81.8% |
| Output format | 90.9% |
| Required-concept recall | 98.9% |
| Forbidden-concept rejection | 100.0% |
| Model-call errors | 0.0% |
| Median wall time per turn | 17,525 ms |

Isolation fixed the direct-answer failure observed in the current Claude path,
but median latency was roughly 2.4× worse and duplicated transition labels caused
two format failures. This configuration is not a production recommendation.
