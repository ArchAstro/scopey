# Paired agent evaluator pilot — 2026-08-04

## Question

Does Scopey prevent wasted main-agent tokens by keeping the agent on task, after
including Scopey's analyzer input and generated output?

## Measurement rule

A shorter run is not automatically a saving. A pair is token-eligible only when
the Scopey arm completes the task, preserves the deterministic repository
oracle, and satisfies the intervention oracle. It is evidence of **prevented
waste** only when the control has more off-track trajectory actions or a worse
scope outcome, the Scopey arm preserves quality, and the paired token delta is
still positive after Scopey overhead.

Main-agent totals come from Codex provider counters. Current-Codex analyzer
totals are also provider-reported and split into analyzer input and generated
output. The isolated runner uses a disposable `CODEX_HOME`/`HOME` containing
only auth and Scopey hooks, so personal plugins and Team Room behavior do not
enter either arm.

## Calibration findings

The first pilot was invalid and is excluded. It exposed four harness defects:

1. Git porcelain parsing dropped the first character of changed paths.
2. Python bytecode directories polluted changed-file assertions.
3. resumed agents could not write `.git`, so the commit oracle failed.
4. nested agents inherited personal plugin/skill behavior, adding unrelated
   tool calls and very large context overhead.

After those fixes, a second full pilot found another evaluator-specific
contaminant: the current-Codex adapter appended its own “do not use tools” guard
after Scopey's prompt. The scope analyzer copied that guard into active scope,
causing a false correction. That run is also excluded from variant conclusions;
the guard now precedes the prompt and is explicitly marked as non-user metadata.

## Valid post-isolation pilot pairs

These are single repetitions, not benchmark estimates:

| Variant | Task | Control main | Scopey main | Scopey overhead | Quality-gated net | Corrections | Prevented waste? |
|---|---|---:|---:|---:|---:|---:|---|
| current-codex | unrelated verification chase | 58,704 | 58,341 | 28,693 | −28,330 | 0 | no; neither arm drifted |
| current-codex | inferred constraint / stale correction | 372,586 | 177,547 | 71,541 | +123,498 | 0 | no; neither arm drifted |
| current-codex | positive on-track control | 107,895 | 123,282 | 57,293 | −72,680 | 0 | no; neither arm drifted |

All three pairs completed successfully and preserved scope. The second row proves the
prompt-epoch scenario can complete without a false correction, but its positive
delta is not evidence of drift prevention: the control also remained on task.
The control for this two-turn task varied substantially across calibration runs,
so one repetition cannot support an efficiency claim.

Summed mechanically, the three pilot rows are +22,488 net tokens (+4.2% of
control main tokens), but that headline is misleading: only one of three pairs
is positive, the median pair is −28,330 tokens, and the positive total is
dominated by the volatile two-turn control. This is why the evaluator reports
per-task rows and medians rather than relying on an aggregate mean.

## Current answer

There is **no observed evidence yet that Scopey prevents wasted tokens by
keeping the model on track**. The one task intended to expose an unrelated-test
chase did not make the isolated control drift. Scopey added substantial overhead
on that pair. The stale-correction task preserved behavior and happened to be
token-positive in one clean repetition, but no drift was prevented and variance
is high.

Before making a product claim, finish and pilot the remaining four archetypes,
reject or redesign tasks whose control never exposes the intended drift
opportunity, freeze the corpus, and run at least three matched repetitions per
task and variant. The evaluator now reports per-pair rows, per-variant/task
aggregates, medians, and a deterministic bootstrap interval once at least three
quality-eligible pairs exist.
