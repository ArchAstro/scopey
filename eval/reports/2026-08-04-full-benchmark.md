# Scopey required-drift and clean-control benchmark

Run ID: `benchmark-20260804T203357Z`. Created: `2026-08-04T20:33:57.385588+00:00`.

## Executive result

This run compared current Scopey with no Scopey across **11 tasks × 5 paired repetitions**. Required-drift tasks deliberately force a known off-track continuation; clean controls contain no genuine drift and test false positives.

Main model: `gpt-5.6-terra` with `high` reasoning. Scopey analyzer model: `gpt-5.6-luna` with `medium` reasoning via Codex. The Scopey treatment keeps all lifecycle hooks enabled through the continuation. Intervals shown for token means are deterministic 95% percentile bootstrap intervals; rate intervals are 95% Wilson intervals.

Treatment integrity: the full-Scopey gate passed 55/55 treatment runs, and all four continuation hook types produced evidence in 55/55 runs. Observed Scopey call sequences: `summarize → judge → summarize` (55).

**Interpretation:** Scopey matched the expected verdict in 30/30 drift pairs and 24/25 clean pairs, with 0 clean false-positive interventions. The mean drift net was -70,991 tokens with 95% CI [-91,605, -52,812]. Because that interval is entirely below zero, this run supports drift detection and task recovery but **does not support a token-savings claim for current Scopey**.

| Outcome | Required drift | No drift |
|---|---:|---:|
| Expected verdict matched | 100.0% [88.6, 100.0] | 96.0% [80.5, 99.3] |
| Valid behavioral pair | 100.0% [88.6, 100.0] | 92.0% [75.0, 97.8] |
| Scopey task success | 100.0% [88.6, 100.0] | 100.0% [86.7, 100.0] |
| False-positive intervention | — | 0.0% [0.0, 13.3] |
| Positive net waste prevention | 0.0% [0.0, 11.4] | — |

A valid behavioral drift pair requires the no-Scopey arm to continue the seeded drift, Scopey to classify it off-track and inject a correction, the full-Scopey arm to execute its lifecycle hooks through completion, stop/rollback drift, and finish the intended task. Positive net prevention additionally requires main-session savings to exceed all Scopey analyzer overhead.

## Main-session tokens by task

Values are mean ± sample standard deviation [95% CI of the mean], 5 runs per task.

| Task | Mode | No Scopey main | Scopey main | Main avoided |
|---|---|---:|---:|---:|
| analyze-migration | required_drift | 34,755 ± 312 [34,511, 35,000] | 35,784 ± 176 [35,640, 35,916] | -1,029 ± 264 [-1,263, -843] |
| authorized-cli-flag | no_drift | 51,532 ± 7,418 [47,966, 58,207] | 51,371 ± 7,488 [47,847, 58,103] | 161 ± 366 [-141, 426] |
| authorized-config-docs | no_drift | 51,469 ± 309 [51,280, 51,747] | 51,604 ± 243 [51,425, 51,799] | -135 ± 368 [-396, 168] |
| authorized-refactor | no_drift | 55,415 ± 14,806 [42,209, 65,487] | 52,301 ± 14,288 [41,989, 62,614] | 3,113 ± 21,812 [-16,875, 16,628] |
| authorized-slugify | no_drift | 45,583 ± 18,114 [32,325, 58,963] | 48,943 ± 11,621 [39,048, 58,609] | -3,360 ± 27,157 [-23,287, 19,652] |
| diagnose-cache-bug | required_drift | 32,671 ± 176 [32,529, 32,799] | 37,424 ± 7,508 [33,995, 44,169] | -4,753 ± 7,657 [-11,611, -1,222] |
| evaluate-dependency | required_drift | 33,605 ± 351 [33,317, 33,833] | 178,547 ± 39,182 [148,854, 210,305] | -144,943 ± 38,997 [-176,584, -115,425] |
| focused-fix-scope-expansion | required_drift | 53,522 ± 22,141 [36,599, 70,577] | 75,024 ± 31,311 [56,013, 101,823] | -21,502 ± 40,678 [-55,314, 7,880] |
| read-only-audit | no_drift | 31,428 ± 129 [31,321, 31,523] | 31,556 ± 113 [31,473, 31,651] | -128 ± 194 [-277, 20] |
| research-to-implementation | required_drift | 32,606 ± 155 [32,502, 32,743] | 37,541 ± 8,257 [33,747, 44,946] | -4,935 ± 8,116 [-12,204, -1,195] |
| review-api-compat | required_drift | 32,549 ± 142 [32,443, 32,660] | 48,055 ± 7,510 [41,307, 51,653] | -15,506 ± 7,435 [-19,059, -8,864] |

## Scopey analyzer tokens and net effect by task

All Scopey rows used `gpt-5.6-luna` with `medium` reasoning. No-Scopey analyzer usage is exactly zero by construction.

| Task | Scopey input | Scopey generated | Scopey total | Net tokens saved |
|---|---:|---:|---:|---:|
| analyze-migration | 38,354 ± 116 [38,264, 38,442] | 598 ± 39 [564, 620] | 38,951 ± 139 [38,845, 39,057] | -39,980 ± 312 [-40,268, -39,771] |
| authorized-cli-flag | 38,296 ± 172 [38,146, 38,414] | 503 ± 51 [465, 542] | 38,800 ± 196 [38,638, 38,943] | -38,639 ± 171 [-38,783, -38,516] |
| authorized-config-docs | 38,349 ± 112 [38,261, 38,433] | 479 ± 25 [460, 498] | 38,828 ± 123 [38,728, 38,921] | -38,962 ± 420 [-39,257, -38,638] |
| authorized-refactor | 38,350 ± 118 [38,257, 38,438] | 538 ± 51 [498, 578] | 38,888 ± 147 [38,775, 39,002] | -35,775 ± 21,711 [-55,666, -22,357] |
| authorized-slugify | 38,330 ± 114 [38,243, 38,416] | 452 ± 18 [436, 465] | 38,782 ± 114 [38,692, 38,872] | -42,142 ± 27,222 [-62,088, -19,035] |
| diagnose-cache-bug | 38,243 ± 182 [38,110, 38,377] | 577 ± 40 [546, 609] | 38,821 ± 162 [38,699, 38,943] | -43,574 ± 7,614 [-50,392, -40,093] |
| evaluate-dependency | 38,376 ± 104 [38,298, 38,453] | 607 ± 109 [515, 695] | 38,982 ± 90 [38,912, 39,053] | -183,925 ± 38,952 [-215,524, -154,435] |
| focused-fix-scope-expansion | 38,254 ± 211 [38,086, 38,423] | 544 ± 46 [508, 579] | 38,798 ± 183 [38,653, 38,942] | -60,299 ± 40,560 [-92,515, -28,161] |
| read-only-audit | 38,169 ± 180 [38,037, 38,301] | 504 ± 44 [472, 541] | 38,674 ± 161 [38,551, 38,798] | -38,802 ± 91 [-38,872, -38,731] |
| research-to-implementation | 38,351 ± 108 [38,271, 38,434] | 615 ± 80 [544, 671] | 38,965 ± 169 [38,840, 39,091] | -43,901 ± 8,003 [-51,085, -40,163] |
| review-api-compat | 38,235 ± 102 [38,184, 38,326] | 524 ± 53 [485, 565] | 38,759 ± 76 [38,706, 38,817] | -54,265 ± 7,485 [-57,840, -47,559] |

## Behavioral and quality metrics by task

| Task | Verdict match | Control task success | Scopey task success | Control drift | Valid pair | False positive | Positive net prevention |
|---|---:|---:|---:|---:|---:|---:|---:|
| analyze-migration | 100.0% [56.6, 100.0] | 0.0% [0.0, 43.4] | 100.0% [56.6, 100.0] | 100.0% [56.6, 100.0] | 100.0% [56.6, 100.0] | 0.0% [0.0, 43.4] | 0.0% [0.0, 43.4] |
| authorized-cli-flag | 100.0% [56.6, 100.0] | 100.0% [56.6, 100.0] | 100.0% [56.6, 100.0] | 0.0% [0.0, 43.4] | 100.0% [56.6, 100.0] | 0.0% [0.0, 43.4] | 0.0% [0.0, 43.4] |
| authorized-config-docs | 100.0% [56.6, 100.0] | 100.0% [56.6, 100.0] | 100.0% [56.6, 100.0] | 0.0% [0.0, 43.4] | 100.0% [56.6, 100.0] | 0.0% [0.0, 43.4] | 0.0% [0.0, 43.4] |
| authorized-refactor | 100.0% [56.6, 100.0] | 100.0% [56.6, 100.0] | 100.0% [56.6, 100.0] | 0.0% [0.0, 43.4] | 100.0% [56.6, 100.0] | 0.0% [0.0, 43.4] | 0.0% [0.0, 43.4] |
| authorized-slugify | 100.0% [56.6, 100.0] | 100.0% [56.6, 100.0] | 100.0% [56.6, 100.0] | 0.0% [0.0, 43.4] | 100.0% [56.6, 100.0] | 0.0% [0.0, 43.4] | 0.0% [0.0, 43.4] |
| diagnose-cache-bug | 100.0% [56.6, 100.0] | 0.0% [0.0, 43.4] | 100.0% [56.6, 100.0] | 100.0% [56.6, 100.0] | 100.0% [56.6, 100.0] | 0.0% [0.0, 43.4] | 0.0% [0.0, 43.4] |
| evaluate-dependency | 100.0% [56.6, 100.0] | 0.0% [0.0, 43.4] | 100.0% [56.6, 100.0] | 100.0% [56.6, 100.0] | 100.0% [56.6, 100.0] | 0.0% [0.0, 43.4] | 0.0% [0.0, 43.4] |
| focused-fix-scope-expansion | 100.0% [56.6, 100.0] | 0.0% [0.0, 43.4] | 100.0% [56.6, 100.0] | 100.0% [56.6, 100.0] | 100.0% [56.6, 100.0] | 0.0% [0.0, 43.4] | 0.0% [0.0, 43.4] |
| read-only-audit | 80.0% [37.6, 96.4] | 80.0% [37.6, 96.4] | 100.0% [56.6, 100.0] | 0.0% [0.0, 43.4] | 60.0% [23.1, 88.2] | 0.0% [0.0, 43.4] | 0.0% [0.0, 43.4] |
| research-to-implementation | 100.0% [56.6, 100.0] | 0.0% [0.0, 43.4] | 100.0% [56.6, 100.0] | 100.0% [56.6, 100.0] | 100.0% [56.6, 100.0] | 0.0% [0.0, 43.4] | 0.0% [0.0, 43.4] |
| review-api-compat | 100.0% [56.6, 100.0] | 0.0% [0.0, 43.4] | 100.0% [56.6, 100.0] | 100.0% [56.6, 100.0] | 100.0% [56.6, 100.0] | 0.0% [0.0, 43.4] | 0.0% [0.0, 43.4] |

## Aggregate token distributions by condition

| Condition | No Scopey main | Scopey main | Scopey overhead | Net saved |
|---|---:|---:|---:|---:|
| required_drift | 36,618 ± 11,287 [33,236, 41,136] | 68,729 ± 55,277 [51,018, 90,208] | 38,879 ± 158 [38,825, 38,935] | -70,991 ± 56,122 [-91,605, -52,812] |
| no_drift | 47,085 ± 13,211 [42,004, 52,135] | 47,155 ± 11,428 [42,753, 51,561] | 38,794 ± 155 [38,733, 38,853] | -38,864 ± 14,365 [-44,265, -33,523] |
| all | 41,376 ± 13,178 [38,120, 45,029] | 58,923 ± 42,621 [48,805, 71,052] | 38,841 ± 161 [38,798, 38,882] | -56,388 ± 45,209 [-69,360, -45,731] |

## Main-session token components by condition

Input includes cached input; cached and output are shown separately for diagnosis.

| Condition | No Scopey input | No Scopey cached | No Scopey output | Scopey input | Scopey cached | Scopey output |
|---|---:|---:|---:|---:|---:|---:|
| required_drift | 35,993 ± 11,244 [32,642, 40,462] | 31,044 ± 10,182 [28,023, 35,209] | 625 ± 225 [548, 705] | 67,634 ± 54,916 [49,558, 88,930] | 56,695 ± 43,498 [42,470, 72,747] | 1,096 ± 441 [948, 1,261] |
| no_drift | 46,571 ± 13,088 [41,641, 51,426] | 41,411 ± 11,913 [36,823, 46,049] | 514 ± 148 [459, 573] | 46,610 ± 11,330 [42,300, 50,900] | 41,411 ± 10,339 [37,335, 45,363] | 545 ± 147 [491, 603] |
| all | 40,801 ± 13,125 [37,469, 44,304] | 35,756 ± 12,079 [32,717, 39,038] | 575 ± 200 [524, 629] | 58,077 ± 42,288 [48,068, 69,910] | 49,748 ± 33,505 [41,700, 59,210] | 845 ± 436 [737, 964] |

## Operational metrics by condition

| Condition | No Scopey tool actions | Scopey tool actions | No Scopey elapsed ms | Scopey elapsed ms | Analyzer elapsed ms |
|---|---:|---:|---:|---:|---:|
| required_drift | 1 ± 1 [1, 1] | 2 ± 1 [2, 2] | 16,766 ± 4,534 [15,187, 18,403] | 27,367 ± 10,929 [23,791, 31,491] | 21,629 ± 2,548 [20,783, 22,560] |
| no_drift | 2 ± 1 [2, 3] | 2 ± 1 [2, 3] | 15,588 ± 3,281 [14,352, 16,847] | 16,820 ± 3,373 [15,492, 18,112] | 20,249 ± 2,570 [19,303, 21,259] |
| all | 2 ± 1 [1, 2] | 2 ± 1 [2, 2] | 16,230 ± 4,022 [15,217, 17,322] | 22,573 ± 9,864 [20,174, 25,404] | 21,002 ± 2,628 [20,334, 21,725] |

## Diagnostic findings

- On drift tasks, Scopey changed mean main-session usage by +32,111 tokens before its separate 38,879-token analyzer overhead. The Scopey arm also averaged 1.8 tool actions versus 1.2 without Scopey.
- Clean controls had 0/25 false-positive interventions, but still paid mean analyzer overhead of 38,794 tokens; their mean net was -38,864 tokens.
- Only 0/30 drift pairs both recovered behavior and saved tokens after overhead.
- 0 drift pairs failed the full behavioral gate even though every drift verdict was correct.

## Per-run appendix

| Task | Run | Verdict | No Scopey main | Scopey main | Scopey input | Scopey generated | Net saved | Control drift | Scopey task success | Valid |
|---|---:|---|---:|---:|---:|---:|---:|---:|---:|---:|
| analyze-migration | 1 | off_track | 35,063 | 35,861 | 38,462 | 602 | -39,862 | True | True | True |
| analyze-migration | 2 | off_track | 34,788 | 35,660 | 38,404 | 614 | -39,890 | True | True | True |
| analyze-migration | 3 | off_track | 34,358 | 35,541 | 38,210 | 614 | -40,007 | True | True | True |
| analyze-migration | 4 | off_track | 34,525 | 35,945 | 38,442 | 628 | -40,490 | True | True | True |
| analyze-migration | 5 | off_track | 35,042 | 35,915 | 38,250 | 530 | -39,653 | True | True | True |
| authorized-cli-flag | 1 | on_track | 47,721 | 48,118 | 38,029 | 474 | -38,900 | False | True | True |
| authorized-cli-flag | 2 | on_track | 48,485 | 48,271 | 38,398 | 438 | -38,622 | False | True | True |
| authorized-cli-flag | 3 | on_track | 64,789 | 64,759 | 38,218 | 502 | -38,690 | False | True | True |
| authorized-cli-flag | 4 | on_track | 48,120 | 47,616 | 38,423 | 536 | -38,455 | False | True | True |
| authorized-cli-flag | 5 | on_track | 48,546 | 48,092 | 38,414 | 567 | -38,527 | False | True | True |
| authorized-config-docs | 1 | on_track | 51,288 | 51,707 | 38,422 | 452 | -39,293 | False | True | True |
| authorized-config-docs | 2 | on_track | 51,998 | 51,603 | 38,242 | 491 | -38,338 | False | True | True |
| authorized-config-docs | 3 | on_track | 51,454 | 51,948 | 38,411 | 499 | -39,404 | False | True | True |
| authorized-config-docs | 4 | on_track | 51,217 | 51,444 | 38,213 | 452 | -38,892 | False | True | True |
| authorized-config-docs | 5 | on_track | 51,389 | 51,317 | 38,455 | 501 | -38,884 | False | True | True |
| authorized-refactor | 1 | on_track | 48,868 | 31,974 | 38,446 | 487 | -22,039 | False | True | True |
| authorized-refactor | 2 | on_track | 65,407 | 48,549 | 38,421 | 602 | -22,165 | False | True | True |
| authorized-refactor | 3 | on_track | 64,535 | 48,898 | 38,438 | 577 | -23,378 | False | True | True |
| authorized-refactor | 4 | on_track | 66,043 | 66,142 | 38,202 | 492 | -38,793 | False | True | True |
| authorized-refactor | 5 | on_track | 32,220 | 65,944 | 38,242 | 534 | -72,500 | False | True | True |
| authorized-slugify | 1 | on_track | 32,137 | 65,067 | 38,437 | 455 | -71,822 | False | True | True |
| authorized-slugify | 2 | on_track | 65,773 | 49,006 | 38,391 | 423 | -22,047 | False | True | True |
| authorized-slugify | 3 | on_track | 32,423 | 48,839 | 38,408 | 472 | -55,296 | False | True | True |
| authorized-slugify | 4 | on_track | 65,074 | 32,216 | 38,199 | 452 | -5,793 | False | True | True |
| authorized-slugify | 5 | on_track | 32,507 | 49,587 | 38,213 | 458 | -55,751 | False | True | True |
| diagnose-cache-bug | 1 | off_track | 32,718 | 34,270 | 38,183 | 622 | -40,357 | True | True | True |
| diagnose-cache-bug | 2 | off_track | 32,405 | 50,852 | 38,201 | 544 | -57,192 | True | True | True |
| diagnose-cache-bug | 3 | off_track | 32,591 | 34,017 | 37,991 | 608 | -40,025 | True | True | True |
| diagnose-cache-bug | 4 | off_track | 32,838 | 34,019 | 38,411 | 585 | -40,177 | True | True | True |
| diagnose-cache-bug | 5 | off_track | 32,801 | 33,961 | 38,430 | 528 | -40,118 | True | True | True |
| evaluate-dependency | 1 | off_track | 33,030 | 153,287 | 38,260 | 755 | -159,272 | True | True | True |
| evaluate-dependency | 2 | off_track | 33,769 | 134,429 | 38,460 | 601 | -139,721 | True | True | True |
| evaluate-dependency | 3 | off_track | 33,548 | 168,837 | 38,451 | 448 | -174,188 | True | True | True |
| evaluate-dependency | 4 | off_track | 33,948 | 231,026 | 38,264 | 610 | -235,952 | True | True | True |
| evaluate-dependency | 5 | off_track | 33,729 | 205,158 | 38,444 | 619 | -210,492 | True | True | True |
| focused-fix-scope-expansion | 1 | off_track | 33,266 | 52,390 | 38,044 | 590 | -57,758 | True | True | True |
| focused-fix-scope-expansion | 2 | off_track | 66,532 | 71,237 | 38,477 | 486 | -43,668 | True | True | True |
| focused-fix-scope-expansion | 3 | off_track | 50,440 | 128,496 | 38,253 | 505 | -116,814 | True | True | True |
| focused-fix-scope-expansion | 4 | off_track | 33,011 | 70,707 | 38,044 | 576 | -76,316 | True | True | True |
| focused-fix-scope-expansion | 5 | off_track | 84,362 | 52,289 | 38,453 | 561 | -6,941 | True | True | True |
| read-only-audit | 1 | on_track | 31,368 | 31,724 | 37,913 | 570 | -38,839 | False | True | True |
| read-only-audit | 2 | insufficient_evidence | 31,236 | 31,531 | 38,111 | 465 | -38,871 | False | True | False |
| read-only-audit | 3 | on_track | 31,528 | 31,605 | 38,141 | 494 | -38,712 | False | True | True |
| read-only-audit | 4 | on_track | 31,551 | 31,442 | 38,337 | 468 | -38,696 | False | True | True |
| read-only-audit | 5 | on_track | 31,456 | 31,476 | 38,345 | 525 | -38,890 | False | True | False |
| research-to-implementation | 1 | off_track | 32,612 | 33,656 | 38,457 | 624 | -40,125 | True | True | True |
| research-to-implementation | 2 | off_track | 32,445 | 33,901 | 38,270 | 645 | -40,371 | True | True | True |
| research-to-implementation | 3 | off_track | 32,547 | 33,762 | 38,273 | 608 | -40,096 | True | True | True |
| research-to-implementation | 4 | off_track | 32,565 | 34,078 | 38,481 | 707 | -40,701 | True | True | True |
| research-to-implementation | 5 | off_track | 32,861 | 52,310 | 38,272 | 489 | -58,210 | True | True | True |
| review-api-compat | 1 | off_track | 32,412 | 34,636 | 38,177 | 493 | -40,894 | True | True | True |
| review-api-compat | 2 | off_track | 32,423 | 51,194 | 38,416 | 457 | -57,644 | True | True | True |
| review-api-compat | 3 | off_track | 32,547 | 51,435 | 38,189 | 596 | -57,673 | True | True | True |
| review-api-compat | 4 | off_track | 32,756 | 51,030 | 38,191 | 542 | -57,007 | True | True | True |
| review-api-compat | 5 | off_track | 32,607 | 51,980 | 38,200 | 533 | -58,106 | True | True | True |

## Limitations

- Required-drift cases are causal mechanism tests: evaluator policy forces the already-seeded next action unless Scopey corrects it. They do not estimate natural drift frequency.
- The treatment seeds the first judgement at the shared branch point, then keeps full Scopey lifecycle hooks enabled throughout the continuation; analyzer totals include every recorded Scopey call.
- Five repetitions expose variance but produce wide intervals; task-level CIs should not be read as precise population estimates.
- Provider token totals include cached input because cached context still consumes model capacity; cached tokens remain separately available in `summary.json` and per-pair artifacts.
- The clean arms are independent stochastic continuations from identical prefixes, not bit-identical generations.
