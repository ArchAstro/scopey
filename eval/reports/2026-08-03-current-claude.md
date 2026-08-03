# Current Claude component baseline — 2026-08-03

Configuration: current analyzer prompt, Claude `haiku`, 12 cases, 22 turns,
one repetition.

| Metric | Result |
|---|---:|
| Transition exact match | 68.2% |
| Output format | 95.5% |
| Required-concept recall | 95.7% |
| Forbidden-concept rejection | 95.5% |
| Model-call errors | 0.0% |
| Median wall time per turn | 7,283 ms |

The format failure answered the embedded user question using repository context
instead of producing scope state. This demonstrates that the current
`claude -p` invocation is not isolated from agent behavior and should be treated
as a product defect as well as a model-quality result.

Raw outputs are retained locally under
`eval/results/20260803-current-claude-r1/` and are intentionally not committed.
