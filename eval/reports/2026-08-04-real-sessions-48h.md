# Scopey real-session analytics

Fixed window: `2026-08-02T18:44:19+00:00` to `2026-08-04T18:44:19+00:00` (48.0 hours).

## Result

Observed **23 real sessions**; 18 reached at least one judge call and 4 received at least one correction.

- Intervention prevalence among all observed sessions: **17.4%** (95% Wilson CI 7.0%–37.1%).
- Intervention prevalence among analyzed sessions: **22.2%** (95% Wilson CI 9.0%–45.2%).
- Correction events per completed judgement: **19/165 = 11.5%** (95% Wilson CI 7.5%–17.3%).
- Scopey overhead / main-session tokens, weighted: **0.6%** (sensitivity 0.4%–0.8%).
- Per-session overhead ratio: mean **14.5%**, median **1.1%**, SD 34.5%, range 0.2%–110.8%.

| Sessions | All | Analyzed | Intervened | No intervention | Main usage measurable |
|---|---:|---:|---:|---:|---:|
| Count | 23 | 18 | 4 | 19 | 23 |

| Aggregate | Main tokens | Scopey tokens | Scopey/main |
|---|---:|---:|---:|
| Window total | 826,294,493 | 5,227,207 | 0.6% |

Verdicts: `{"insufficient_evidence": 1, "off_track": 12, "on_track": 145, "warning": 7}`.

Main-token composition (provider logical usage): 809,968,327 cached input, 9,433,510 uncached input, 4,710,234 cache-write input, and 2,182,422 output. The overhead ratio is a token-volume ratio, not a dollar-cost estimate.

## Intervention versus non-intervention overhead

| Group | Sessions | Judge calls | Corrections | Main tokens | Scopey tokens | Weighted overhead/main | Median session ratio |
|---|---:|---:|---:|---:|---:|---:|---:|
| At least one intervention | 4 | 118 | 19 | 304,758,278 | 2,288,033 | 0.8% (0.7%–0.8%) | 0.7% |
| Analyzed, no intervention | 14 | 102 | 0 | 520,085,448 | 2,791,418 | 0.5% (0.3%–0.8%) | 1.0% |
| All sessions, no intervention | 19 | 102 | 0 | 521,536,215 | 2,939,174 | 0.6% (0.3%–0.9%) | 1.1% |

Analyzer accounting coverage: 12 provider-reported calls and 304 estimated calls; 120 of the estimates are started Claude jobs with no matchable usage artifact and therefore use the same-window median plus a zero-to-observed-max sensitivity range.

## Per-session privacy-safe rows

| Session | Harness | Main tokens | Summaries | Judges | Interventions | Scopey tokens | Overhead/main | Scopey model(s) |
|---|---|---:|---:|---:|---:|---:|---:|---|
| `154dc8f8d1ba` | claude | 22,453 | 1 | 0 | 0 | 23,580 | 105.0% | haiku |
| `171f58fb659c` | codex | 10,388,781 | 3 | 5 | 0 | 114,702 | 1.1% | gpt-5.6-terra |
| `1f41ac3b4a9a` | codex | 142,203,597 | 19 | 48 | 14 | 957,135 | 0.7% | gpt-5.6-terra |
| `3395c071f08c` | claude | 29,664,474 | 14 | 5 | 0 | 337,305 | 1.1% | claude-haiku-4-5-20251001, haiku |
| `49b302d90b89` | codex | 89,584,377 | 9 | 40 | 2 | 702,115 | 0.8% | gpt-5.6-terra |
| `5ae9119b9d0a` | claude | 22,523 | 1 | 0 | 0 | 24,945 | 110.8% | claude-haiku-4-5-20251001 |
| `5e1d8d614de1` | claude | 13,568,983 | 5 | 3 | 0 | 117,900 | 0.9% | haiku |
| `68a71a5f1db7` | claude | 4,438,462 | 2 | 1 | 0 | 47,160 | 1.1% | haiku |
| `8305cbe89a11` | claude | 30,643,290 | 3 | 10 | 0 | 70,740 | 0.2% | haiku |
| `85595dcbb0eb` | claude | 19,415,635 | 19 | 4 | 0 | 440,627 | 2.3% | claude-haiku-4-5-20251001, haiku |
| `8c163f01b09e` | codex | 63,127,230 | 5 | 23 | 1 | 400,559 | 0.6% | gpt-5.6-terra |
| `8d297d26306e` | claude | 246,466,377 | 24 | 19 | 0 | 565,920 | 0.2% | haiku |
| `9a5d63771dd6` | claude | 626,029 | 2 | 0 | 0 | 47,160 | 7.5% | haiku |
| `a0c9118da36c` | codex | 14,258,181 | 5 | 9 | 0 | 200,266 | 1.4% | gpt-5.6-terra |
| `b0d18eac7bb0` | codex | 4,785,558 | 1 | 2 | 0 | 42,816 | 0.9% | gpt-5.6-terra |
| `c0175027554b` | codex | 15,628,614 | 7 | 5 | 0 | 171,748 | 1.1% | gpt-5.6-terra |
| `c07f67cbceb1` | claude | 59,319,862 | 4 | 14 | 0 | 94,320 | 0.2% | haiku |
| `ca0eecb79692` | codex | 752,713 | 2 | 0 | 0 | 28,491 | 3.8% | gpt-5.6-terra |
| `d9e94a6b526c` | codex | 61,817,785 | 14 | 20 | 0 | 488,027 | 0.8% | gpt-5.6-terra |
| `e746acaf5e14` | codex | 846,052 | 1 | 1 | 0 | 28,578 | 3.4% | gpt-5.6-terra |
| `eb568e4c2569` | codex | 9,843,074 | 9 | 7 | 2 | 228,224 | 2.3% | gpt-5.6-terra |
| `f5c1ba881ec5` | codex | 8,843,394 | 1 | 4 | 0 | 71,309 | 0.8% | gpt-5.6-terra |
| `fa192aac9fef` | claude | 27,049 | 1 | 0 | 0 | 23,580 | 87.2% | haiku |

## Measurement contract

- Cohort: non-internal Scopey sessions with activity in the fixed window, an existing Claude/Codex transcript, and a non-test cwd. Test/eval/temp cwd patterns are excluded.
- Main tokens: provider-reported transcript usage inside the same fixed window. Cached input is included and separately retained in JSON.
- Intervention: a persisted Scopey injection with `kind=correction`; reminders are not interventions.
- Claude Scopey calls: exact provider usage when a print-mode transcript matches the job; unmatched starts use the same-window matched-call median, with zero and observed maximum as sensitivity bounds.
- Codex Scopey calls: historical estimate because production used ephemeral calls. Input is calibrated by kind from 55 `gpt-5.6-terra` calls per kind in `benchmark-20260804T170422Z`; generated tokens use persisted response characters / 4.
- The JSON artifact includes low/high sensitivity estimates, match coverage, exclusion reasons, calibration constants, and fixed timestamps.
- This is observational telemetry. It measures prevalence and overhead, but cannot establish whether each intervention was correct or how many counterfactual main-session tokens it prevented.

Reproduce with:

```bash
python3 eval/recent_session_analytics.py --since 2026-08-02T18:44:19+00:00 --until 2026-08-04T18:44:19+00:00 --json-out <result.json> --markdown-out <report.md>
```
