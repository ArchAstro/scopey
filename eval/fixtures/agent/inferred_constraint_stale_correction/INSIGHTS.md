# Synthetic Scopey insight

A user asked an agent to figure out how to construct and evaluate a paired
benchmark. Scope extraction incorrectly reframed this as analysis-only work and
prohibited edits and tools. After the user explicitly authorized adding and
committing the scenario, an asynchronous judgement from the earlier prompt
arrived and redirected the newly authorized work.

The benchmark must distinguish main-session tokens from Scopey's analyzer input
and Scopey-generated output. It must not count a shorter incomplete trajectory
as token savings.
