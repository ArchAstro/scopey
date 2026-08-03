# Local LLaDA-MoE Q4 diffusion spike — 2026-08-03

Configuration:

- prompt: `eval/prompts/local.txt`
- runtime: llama.cpp `fe2adf0e722f30f5295fdec8a0f1dc788f7498bc`
- model: `LLaDA-MoE-7B-A1B-Instruct.Q4_K_M.gguf`
- model SHA-256: `a8fc1d9d43718a742b55b122cdca739a9cc2e790e38b2316b1a5b10e84489b27`
- diffusion: confidence algorithm, 448-token canvas, block length 32, 224
  steps, temperature 0, seed 42
- hardware: Apple arm64 with Metal, 128 GB unified memory
- cases: 3 adversarial, two-turn cases
- scored turns: 6

| Metric | Result |
|---|---:|
| Transition exact match | 50.0% |
| Output format | 100.0% |
| Required-concept recall | 78.3% |
| Forbidden-concept rejection | 50.0% |
| Model-call errors | 0.0% |
| Median wall time per turn | 7,717 ms |

The model handled simple first-turn additions but failed the state transitions
that justify Scopey: modification, replacement, and query preservation. Fixed
canvas generation also required deterministic cleanup of duplicated bullets and
multiple markers. It is eliminated as the default candidate because it is both
slower and materially less accurate than the preliminary Codex reference.

Raw outputs are retained locally under
`eval/results/20260803-local-llada-adversarial-r2/` and are intentionally not
committed.
