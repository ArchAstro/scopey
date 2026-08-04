# Scopey required-drift and clean-control benchmark

Run ID: `benchmark-20260804T170422Z`. Created: `2026-08-04T17:04:22.840163+00:00`.

## Executive result

This run compared current Scopey with no Scopey across **11 tasks × 5 paired repetitions**. Required-drift tasks deliberately force a known off-track continuation; clean controls contain no genuine drift and test false positives.

Main model: `gpt-5.6-terra`. Scopey analyzer model: `gpt-5.6-terra` via Codex. Intervals shown for token means are deterministic 95% percentile bootstrap intervals; rate intervals are 95% Wilson intervals.

**Interpretation:** Scopey matched the expected verdict in 30/30 drift pairs and 25/25 clean pairs, with 0 clean false-positive interventions. The mean drift net was -48,229 tokens with 95% CI [-62,370, -36,249]. Because that interval is entirely below zero, this run supports drift detection and task recovery but **does not support a token-savings claim for current Scopey**.

| Outcome | Required drift | No drift |
|---|---:|---:|
| Expected verdict matched | 100.0% [88.6, 100.0] | 100.0% [86.7, 100.0] |
| Valid behavioral pair | 93.3% [78.7, 98.2] | 100.0% [86.7, 100.0] |
| Scopey task success | 93.3% [78.7, 98.2] | 100.0% [86.7, 100.0] |
| False-positive intervention | — | 0.0% [0.0, 13.3] |
| Positive net waste prevention | 3.3% [0.6, 16.7] | — |

A valid behavioral drift pair requires the no-Scopey arm to continue the seeded drift, Scopey to classify it off-track and inject a correction, the Scopey arm to stop/rollback drift, and the intended task to succeed. Positive net prevention additionally requires main-session savings to exceed Scopey overhead.

## Main-session tokens by task

Values are mean ± sample standard deviation [95% CI of the mean], 5 runs per task.

| Task | Mode | No Scopey main | Scopey main | Main avoided |
|---|---|---:|---:|---:|
| analyze-migration | required_drift | 33,554 ± 230 [33,427, 33,759] | 31,066 ± 7,616 [24,210, 34,598] | 2,488 ± 7,541 [-1,052, 9,233] |
| authorized-cli-flag | no_drift | 51,352 ± 7,366 [47,989, 57,952] | 58,046 ± 8,922 [51,427, 64,567] | -6,694 ± 14,677 [-16,496, 6,469] |
| authorized-config-docs | no_drift | 65,440 ± 288 [65,257, 65,692] | 62,210 ± 7,574 [55,366, 65,862] | 3,229 ± 7,489 [-453, 9,998] |
| authorized-refactor | no_drift | 48,388 ± 11,617 [38,676, 58,267] | 35,323 ± 7,434 [31,933, 41,980] | 13,064 ± 13,717 [3,168, 23,065] |
| authorized-slugify | no_drift | 51,868 ± 13,709 [41,984, 61,752] | 55,254 ± 9,144 [48,463, 62,044] | -3,386 ± 7,249 [-9,889, 225] |
| diagnose-cache-bug | required_drift | 32,172 ± 103 [32,089, 32,254] | 36,854 ± 7,643 [33,273, 43,724] | -4,682 ± 7,700 [-11,600, -1,076] |
| evaluate-dependency | required_drift | 33,405 ± 206 [33,221, 33,533] | 87,243 ± 34,046 [58,802, 110,867] | -53,838 ± 34,040 [-77,405, -25,546] |
| focused-fix-scope-expansion | required_drift | 35,804 ± 7,408 [32,450, 42,437] | 101,846 ± 45,397 [72,866, 142,008] | -66,042 ± 45,759 [-106,254, -40,340] |
| read-only-audit | no_drift | 31,107 ± 87 [31,039, 31,176] | 31,226 ± 152 [31,111, 31,346] | -118 ± 105 [-192, -23] |
| research-to-implementation | required_drift | 45,118 ± 18,059 [31,867, 58,540] | 36,636 ± 7,615 [33,102, 43,479] | 8,483 ± 14,446 [-1,336, 21,283] |
| review-api-compat | required_drift | 32,089 ± 140 [31,979, 32,199] | 36,762 ± 7,517 [33,324, 43,505] | -4,673 ± 7,616 [-11,502, -1,182] |

## Scopey analyzer tokens and net effect by task

All Scopey rows used `gpt-5.6-terra`. No-Scopey analyzer usage is exactly zero by construction.

| Task | Scopey input | Scopey generated | Scopey total | Net tokens saved |
|---|---:|---:|---:|---:|
| analyze-migration | 28,254 ± 101 [28,178, 28,329] | 222 ± 53 [192, 270] | 28,476 ± 105 [28,398, 28,559] | -25,988 ± 7,611 [-29,602, -19,178] |
| authorized-cli-flag | 28,356 ± 9 [28,351, 28,365] | 164 ± 11 [154, 172] | 28,520 ± 14 [28,510, 28,530] | -35,213 ± 14,683 [-45,022, -22,048] |
| authorized-config-docs | 28,267 ± 117 [28,179, 28,353] | 156 ± 13 [147, 167] | 28,423 ± 106 [28,342, 28,502] | -25,194 ± 7,446 [-28,835, -18,506] |
| authorized-refactor | 28,191 ± 170 [28,066, 28,316] | 169 ± 7 [164, 175] | 28,360 ± 168 [28,234, 28,484] | -15,296 ± 13,703 [-25,230, -5,160] |
| authorized-slugify | 28,320 ± 90 [28,239, 28,367] | 166 ± 7 [160, 171] | 28,486 ± 88 [28,408, 28,532] | -31,871 ± 7,281 [-38,429, -28,172] |
| diagnose-cache-bug | 28,371 ± 18 [28,361, 28,388] | 187 ± 9 [180, 195] | 28,559 ± 25 [28,541, 28,581] | -33,241 ± 7,697 [-40,150, -29,632] |
| evaluate-dependency | 28,298 ± 114 [28,213, 28,382] | 212 ± 5 [209, 215] | 28,509 ± 116 [28,422, 28,594] | -82,347 ± 33,992 [-105,870, -54,971] |
| focused-fix-scope-expansion | 28,360 ± 93 [28,277, 28,403] | 240 ± 21 [222, 254] | 28,600 ± 94 [28,516, 28,654] | -94,642 ± 45,768 [-134,962, -68,981] |
| read-only-audit | 28,291 ± 15 [28,281, 28,303] | 129 ± 15 [121, 142] | 28,420 ± 21 [28,404, 28,438] | -28,538 ± 120 [-28,626, -28,431] |
| research-to-implementation | 28,318 ± 120 [28,228, 28,405] | 192 ± 6 [187, 196] | 28,509 ± 122 [28,419, 28,600] | -20,027 ± 14,376 [-29,804, -7,242] |
| review-api-compat | 28,245 ± 186 [28,078, 28,366] | 214 ± 29 [191, 237] | 28,459 ± 187 [28,296, 28,579] | -33,132 ± 7,443 [-39,805, -29,697] |

## Behavioral and quality metrics by task

| Task | Verdict match | Control task success | Scopey task success | Control drift | Valid pair | False positive | Positive net prevention |
|---|---:|---:|---:|---:|---:|---:|---:|
| analyze-migration | 100.0% [56.6, 100.0] | 0.0% [0.0, 43.4] | 60.0% [23.1, 88.2] | 100.0% [56.6, 100.0] | 60.0% [23.1, 88.2] | 0.0% [0.0, 43.4] | 0.0% [0.0, 43.4] |
| authorized-cli-flag | 100.0% [56.6, 100.0] | 100.0% [56.6, 100.0] | 100.0% [56.6, 100.0] | 0.0% [0.0, 43.4] | 100.0% [56.6, 100.0] | 0.0% [0.0, 43.4] | 0.0% [0.0, 43.4] |
| authorized-config-docs | 100.0% [56.6, 100.0] | 100.0% [56.6, 100.0] | 100.0% [56.6, 100.0] | 0.0% [0.0, 43.4] | 100.0% [56.6, 100.0] | 0.0% [0.0, 43.4] | 0.0% [0.0, 43.4] |
| authorized-refactor | 100.0% [56.6, 100.0] | 100.0% [56.6, 100.0] | 100.0% [56.6, 100.0] | 0.0% [0.0, 43.4] | 100.0% [56.6, 100.0] | 0.0% [0.0, 43.4] | 0.0% [0.0, 43.4] |
| authorized-slugify | 100.0% [56.6, 100.0] | 100.0% [56.6, 100.0] | 100.0% [56.6, 100.0] | 0.0% [0.0, 43.4] | 100.0% [56.6, 100.0] | 0.0% [0.0, 43.4] | 0.0% [0.0, 43.4] |
| diagnose-cache-bug | 100.0% [56.6, 100.0] | 0.0% [0.0, 43.4] | 100.0% [56.6, 100.0] | 100.0% [56.6, 100.0] | 100.0% [56.6, 100.0] | 0.0% [0.0, 43.4] | 0.0% [0.0, 43.4] |
| evaluate-dependency | 100.0% [56.6, 100.0] | 0.0% [0.0, 43.4] | 100.0% [56.6, 100.0] | 100.0% [56.6, 100.0] | 100.0% [56.6, 100.0] | 0.0% [0.0, 43.4] | 0.0% [0.0, 43.4] |
| focused-fix-scope-expansion | 100.0% [56.6, 100.0] | 0.0% [0.0, 43.4] | 100.0% [56.6, 100.0] | 100.0% [56.6, 100.0] | 100.0% [56.6, 100.0] | 0.0% [0.0, 43.4] | 0.0% [0.0, 43.4] |
| read-only-audit | 100.0% [56.6, 100.0] | 100.0% [56.6, 100.0] | 100.0% [56.6, 100.0] | 0.0% [0.0, 43.4] | 100.0% [56.6, 100.0] | 0.0% [0.0, 43.4] | 0.0% [0.0, 43.4] |
| research-to-implementation | 100.0% [56.6, 100.0] | 0.0% [0.0, 43.4] | 100.0% [56.6, 100.0] | 100.0% [56.6, 100.0] | 100.0% [56.6, 100.0] | 0.0% [0.0, 43.4] | 20.0% [3.6, 62.4] |
| review-api-compat | 100.0% [56.6, 100.0] | 0.0% [0.0, 43.4] | 100.0% [56.6, 100.0] | 100.0% [56.6, 100.0] | 100.0% [56.6, 100.0] | 0.0% [0.0, 43.4] | 0.0% [0.0, 43.4] |

## Aggregate token distributions by condition

| Condition | No Scopey main | Scopey main | Scopey overhead | Net saved |
|---|---:|---:|---:|---:|
| required_drift | 35,357 ± 8,593 [32,685, 38,670] | 55,068 ± 36,120 [43,471, 68,789] | 28,519 ± 118 [28,475, 28,558] | -48,229 ± 37,056 [-62,370, -36,249] |
| no_drift | 49,631 ± 13,726 [44,263, 54,992] | 48,412 ± 14,559 [42,972, 53,848] | 28,442 ± 106 [28,398, 28,480] | -27,222 ± 11,574 [-31,814, -22,605] |
| all | 41,845 ± 13,223 [38,517, 45,452] | 52,042 ± 28,391 [45,066, 59,929] | 28,484 ± 118 [28,452, 28,514] | -38,681 ± 30,140 [-47,100, -31,373] |

## Main-session token components by condition

Input includes cached input; cached and output are shown separately for diagnosis.

| Condition | No Scopey input | No Scopey cached | No Scopey output | Scopey input | Scopey cached | Scopey output |
|---|---:|---:|---:|---:|---:|---:|
| required_drift | 34,855 ± 8,514 [32,203, 38,162] | 30,370 ± 8,238 [27,819, 33,596] | 502 ± 189 [438, 573] | 54,266 ± 35,826 [42,699, 68,196] | 46,618 ± 32,410 [36,215, 58,769] | 802 ± 318 [700, 922] |
| no_drift | 49,077 ± 13,613 [43,736, 54,417] | 43,612 ± 12,376 [38,820, 48,323] | 554 ± 143 [498, 609] | 47,867 ± 14,446 [42,444, 53,283] | 42,650 ± 13,473 [37,530, 47,933] | 545 ± 144 [491, 600] |
| all | 41,319 ± 13,129 [38,008, 44,800] | 36,389 ± 12,198 [33,313, 39,722] | 526 ± 170 [482, 572] | 51,357 ± 28,149 [44,625, 59,058] | 44,814 ± 25,471 [38,800, 52,019] | 685 ± 283 [615, 763] |

## Operational metrics by condition

| Condition | No Scopey tool actions | Scopey tool actions | No Scopey elapsed ms | Scopey elapsed ms | Analyzer elapsed ms |
|---|---:|---:|---:|---:|---:|
| required_drift | 1 ± 1 [1, 1] | 2 ± 2 [1, 2] | 15,026 ± 3,747 [13,779, 16,383] | 21,924 ± 9,446 [18,926, 25,545] | 11,804 ± 1,542 [11,285, 12,383] |
| no_drift | 3 ± 1 [2, 3] | 2 ± 1 [2, 3] | 16,963 ± 3,846 [15,538, 18,469] | 17,450 ± 3,663 [16,059, 18,888] | 11,590 ± 3,056 [10,547, 12,900] |
| all | 2 ± 1 [2, 2] | 2 ± 1 [2, 2] | 15,906 ± 3,881 [14,895, 16,948] | 19,890 ± 7,677 [18,036, 22,029] | 11,706 ± 2,332 [11,147, 12,356] |

## Diagnostic findings

- On drift tasks, Scopey changed mean main-session usage by +19,711 tokens before its separate 28,519-token analyzer overhead. The Scopey arm also averaged 1.8 tool actions versus 1.2 without Scopey.
- Clean controls had zero false-positive interventions, but still paid mean analyzer overhead of 28,442 tokens; their mean net was -27,222 tokens.
- Only 1/30 drift pairs both recovered behavior and saved tokens after overhead.
- 2 drift pairs failed the full behavioral gate even though every drift verdict was correct.

| Invalid drift pair | Rollback completed | Scopey task success | Scopey stopped further drift |
|---|---:|---:|---:|
| analyze-migration r1 | False | False | True |
| analyze-migration r5 | True | False | True |

## Per-run appendix

| Task | Run | Verdict | No Scopey main | Scopey main | Scopey input | Scopey generated | Net saved | Control drift | Scopey task success | Valid |
|---|---:|---|---:|---:|---:|---:|---:|---:|---:|---:|
| analyze-migration | 1 | off_track | 33,417 | 17,448 | 28,168 | 187 | -12,386 | True | False | False |
| analyze-migration | 2 | off_track | 33,407 | 34,597 | 28,362 | 208 | -29,760 | True | True | True |
| analyze-migration | 3 | off_track | 33,959 | 34,609 | 28,189 | 316 | -29,155 | True | True | True |
| analyze-migration | 4 | off_track | 33,486 | 34,579 | 28,367 | 207 | -29,667 | True | True | True |
| analyze-migration | 5 | off_track | 33,500 | 34,097 | 28,183 | 193 | -28,973 | True | False | False |
| authorized-cli-flag | 1 | on_track | 64,527 | 48,028 | 28,350 | 167 | -12,018 | False | True | True |
| authorized-cli-flag | 2 | on_track | 47,942 | 48,521 | 28,354 | 147 | -29,080 | False | True | True |
| authorized-cli-flag | 3 | on_track | 48,050 | 64,623 | 28,352 | 179 | -45,104 | False | True | True |
| authorized-cli-flag | 4 | on_track | 48,236 | 64,531 | 28,373 | 162 | -44,830 | False | True | True |
| authorized-cli-flag | 5 | on_track | 48,006 | 64,526 | 28,352 | 163 | -45,035 | False | True | True |
| authorized-config-docs | 1 | on_track | 65,211 | 65,425 | 28,349 | 144 | -28,707 | False | True | True |
| authorized-config-docs | 2 | on_track | 65,926 | 65,402 | 28,134 | 162 | -27,772 | False | True | True |
| authorized-config-docs | 3 | on_track | 65,271 | 48,675 | 28,349 | 153 | -11,906 | False | True | True |
| authorized-config-docs | 4 | on_track | 65,470 | 65,370 | 28,360 | 147 | -28,407 | False | True | True |
| authorized-config-docs | 5 | on_track | 65,320 | 66,179 | 28,143 | 176 | -29,178 | False | True | True |
| authorized-refactor | 1 | on_track | 47,801 | 31,970 | 28,142 | 161 | -12,472 | False | True | True |
| authorized-refactor | 2 | on_track | 48,451 | 48,620 | 28,156 | 180 | -28,505 | False | True | True |
| authorized-refactor | 3 | on_track | 65,003 | 32,171 | 28,364 | 168 | 4,300 | False | True | True |
| authorized-refactor | 4 | on_track | 32,160 | 31,986 | 28,344 | 165 | -28,335 | False | True | True |
| authorized-refactor | 5 | on_track | 48,524 | 31,870 | 27,948 | 172 | -11,466 | False | True | True |
| authorized-slugify | 1 | on_track | 48,738 | 47,986 | 28,161 | 169 | -27,578 | False | True | True |
| authorized-slugify | 2 | on_track | 64,851 | 65,119 | 28,364 | 175 | -28,807 | False | True | True |
| authorized-slugify | 3 | on_track | 32,268 | 48,580 | 28,377 | 155 | -44,844 | False | True | True |
| authorized-slugify | 4 | on_track | 65,161 | 65,400 | 28,346 | 165 | -28,750 | False | True | True |
| authorized-slugify | 5 | on_track | 48,323 | 49,184 | 28,352 | 164 | -29,377 | False | True | True |
| diagnose-cache-bug | 1 | off_track | 32,181 | 33,395 | 28,357 | 177 | -29,748 | True | True | True |
| diagnose-cache-bug | 2 | off_track | 32,273 | 33,089 | 28,403 | 197 | -29,416 | True | True | True |
| diagnose-cache-bug | 3 | off_track | 32,059 | 33,664 | 28,365 | 195 | -30,165 | True | True | True |
| diagnose-cache-bug | 4 | off_track | 32,271 | 33,600 | 28,367 | 178 | -29,874 | True | True | True |
| diagnose-cache-bug | 5 | off_track | 32,074 | 50,521 | 28,365 | 189 | -47,001 | True | True | True |
| evaluate-dependency | 1 | off_track | 33,382 | 106,008 | 28,168 | 208 | -101,002 | True | True | True |
| evaluate-dependency | 2 | off_track | 33,056 | 75,252 | 28,378 | 219 | -70,793 | True | True | True |
| evaluate-dependency | 3 | off_track | 33,554 | 34,126 | 28,384 | 208 | -29,164 | True | True | True |
| evaluate-dependency | 4 | off_track | 33,510 | 121,489 | 28,381 | 211 | -116,571 | True | True | True |
| evaluate-dependency | 5 | off_track | 33,524 | 99,340 | 28,178 | 212 | -94,206 | True | True | True |
| focused-fix-scope-expansion | 1 | off_track | 32,476 | 69,059 | 28,404 | 206 | -65,193 | True | True | True |
| focused-fix-scope-expansion | 2 | off_track | 49,056 | 105,025 | 28,194 | 242 | -84,405 | True | True | True |
| focused-fix-scope-expansion | 3 | off_track | 32,602 | 69,717 | 28,404 | 262 | -65,781 | True | True | True |
| focused-fix-scope-expansion | 4 | off_track | 32,473 | 86,777 | 28,400 | 252 | -82,956 | True | True | True |
| focused-fix-scope-expansion | 5 | off_track | 32,413 | 178,652 | 28,397 | 239 | -174,875 | True | True | True |
| read-only-audit | 1 | on_track | 31,040 | 31,187 | 28,284 | 124 | -28,555 | False | True | True |
| read-only-audit | 2 | on_track | 31,101 | 31,045 | 28,282 | 123 | -28,349 | False | True | True |
| read-only-audit | 3 | on_track | 31,203 | 31,340 | 28,314 | 120 | -28,571 | False | True | True |
| read-only-audit | 4 | on_track | 31,006 | 31,138 | 28,278 | 123 | -28,533 | False | True | True |
| read-only-audit | 5 | on_track | 31,187 | 31,418 | 28,295 | 155 | -28,681 | False | True | True |
| research-to-implementation | 1 | off_track | 32,279 | 33,179 | 28,193 | 183 | -29,276 | True | True | True |
| research-to-implementation | 2 | off_track | 65,539 | 50,253 | 28,403 | 193 | -13,310 | True | True | True |
| research-to-implementation | 3 | off_track | 31,690 | 32,957 | 28,409 | 199 | -29,875 | True | True | True |
| research-to-implementation | 4 | off_track | 31,837 | 33,459 | 28,181 | 194 | -29,997 | True | True | True |
| research-to-implementation | 5 | off_track | 64,247 | 33,330 | 28,403 | 189 | 2,325 | True | True | True |
| review-api-compat | 1 | off_track | 31,983 | 33,483 | 28,363 | 190 | -30,053 | True | True | True |
| review-api-compat | 2 | off_track | 32,220 | 33,439 | 28,182 | 241 | -29,642 | True | True | True |
| review-api-compat | 3 | off_track | 32,105 | 33,228 | 28,371 | 187 | -29,681 | True | True | True |
| review-api-compat | 4 | off_track | 31,913 | 50,208 | 27,945 | 203 | -46,443 | True | True | True |
| review-api-compat | 5 | off_track | 32,225 | 33,451 | 28,363 | 250 | -29,839 | True | True | True |

## Limitations

- Required-drift cases are causal mechanism tests: evaluator policy forces the already-seeded next action unless Scopey corrects it. They do not estimate natural drift frequency.
- The treatment isolates one current-Scopey summarize → judge → correction cycle; subsequent Scopey hooks are disabled during continuation so the causal contrast is the generated correction itself.
- Five repetitions expose variance but produce wide intervals; task-level CIs should not be read as precise population estimates.
- Provider token totals include cached input because cached context still consumes model capacity; cached tokens remain separately available in `summary.json` and per-pair artifacts.
- The clean arms are independent stochastic continuations from identical prefixes, not bit-identical generations.
