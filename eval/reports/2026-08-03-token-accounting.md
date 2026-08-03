# Scopey token accounting - Qwen3.5 9B Q4

The controlled run compared `no-scopey` and `local-qwen3.5-9b-q4` over the
same 12 scenarios, 22 scored turns, and three repetitions. Qwen analyzer usage
is reported by llama.cpp; main-session context is counted with the same Qwen
tokenizer. Results below are per corpus repetition.

| Bucket | No Scopey | Scopey + Qwen |
|---|---:|---:|
| Main-session user context | 431 | 431 |
| Scopey analyzer input | 0 | 7,958 |
| Scopey-generated output | 0 | 1,613 |
| Scopey total overhead | 0 | 9,571 |
| Observed component total | 431 | 10,002 |

This component corpus does not contain main-agent generations or tool results,
so it cannot measure tokens actually prevented by a course correction. The
break-even calculation asks how long an unwanted future suffix would need to
be before terminating it offsets Scopey's 9,571-token analyzer overhead.

| Avoided main tokens per scenario | Net corpus tokens saved | Reduction vs modeled no-Scopey continuation |
|---:|---:|---:|
| 0 | -9,571 | -2,220.6% |
| 250 | -6,571 | -191.5% |
| 500 | -3,571 | -55.5% |
| 800 | 29 | 0.2% |
| 1,000 | 2,429 | 19.5% |
| 2,500 | 20,429 | 67.1% |
| 5,000 | 50,429 | 83.4% |

Average break-even is 798 avoided tokens per scenario. Individual scenario
break-even points range from 400 tokens for the one-turn explanation case to
958 tokens for the two-turn cross-platform constraint case.

The full scenario table and limitations are in
`eval/paper/scopey-token-accounting.tex`; the rendered paper is
`output/pdf/scopey-token-accounting.pdf`.
