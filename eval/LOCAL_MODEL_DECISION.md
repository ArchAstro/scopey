# Local model decision — 2026-08-03

## Decision

Use a warm, loopback-only llama.cpp server with Qwen3.5 9B Q4_K_M as the first
local Scopey summarizer. Keep cloud Codex as the compatibility fallback. Do not
ship a text-diffusion model as the default yet.

This decision is based on Scopey's checked-in 12-case, 22-turn component corpus:

| Configuration | Turns | Required | Forbidden | Format | Errors | Median |
|---|---:|---:|---:|---:|---:|---:|
| no Scopey | 22 | 77.7% | 100% | 0% | 0% | — |
| current Codex | 22 | 100% | 100% | 100% | 0% | 5.70s |
| next Codex | 22 | 100% | 100% | 100% | 0% | 4.97s |
| current Claude | 22 | 95.7% | 95.5% | 95.5% | 0% | 7.28s |
| next Claude | 22 | 98.9% | 100% | 90.9% | 0% | 17.52s |
| LLaDA-MoE Q4 diffusion | 6 adversarial | 78.3% | 50% | 100%* | 0% | 7.72s |
| Qwen3 4B Q4 local | 22 | 95.7% | 95.5% | 100% | 0% | 0.65s |
| Qwen3.5 9B Q4 local | 66 | 100% | 100% | 100% | 0% | 1.39s |

`*` LLaDA formatting required deterministic duplicate/trailing-marker cleanup.
Dream 7B was not promoted to the adversarial slice: a correct one-case result
took 9.5 seconds at 32 steps, while 16 steps took 5.1 seconds and lost an
explicit requirement.

Transition taxonomy is reported separately from active-scope correctness. Both
the selected local model and the cloud reference sometimes use a coarser label
while producing the correct scope body consumed by Scopey.

## Runtime choice

Use llama.cpp because it provides one embeddable C/C++ runtime and release
artifacts across macOS, Linux, and Windows, with Metal, CUDA, HIP, Vulkan, SYCL,
and CPU backends. Its OpenAI-compatible server also isolates model execution
from Scopey's Rust process and lets a single warm model serve background jobs.

Do not use `model_command` for the product path. It invokes `sh -c`, is not a
native Windows abstraction, cannot securely own a long-lived server, and makes
process cleanup ambiguous.

`diffuse-cpp` is promising research for CPU text diffusion because it implements
adaptive entropy exit and inter-step caching. It is not the packaging choice:
it currently lacks an integrated tokenizer, GPU support, and release artifacts.
The llama.cpp diffusion CLI is also an example target omitted from official
release archives. Either choice would make Scopey own more cross-platform build
and distribution work than the winning autoregressive server path.

Primary references:

- <https://github.com/ggml-org/llama.cpp>
- <https://github.com/iafiscal1212/diffuse-cpp>
- <https://huggingface.co/Qwen/Qwen3.5-9B>
- <https://huggingface.co/bartowski/Qwen_Qwen3.5-9B-GGUF>

## Product lifecycle contract

Local inference must be opt-in and must never download gigabytes from a hook.
The explicit command surface should be:

```text
scopey models local install [--model qwen3.5-9b-q4]
scopey models local status
scopey models local enable
scopey models local disable
scopey models local remove [--model-only | --all]
```

Install behavior:

1. Resolve a signed Scopey manifest by OS, architecture, and acceleration tier.
2. Download to a `.part` file with resume support.
3. Verify byte size and SHA-256 before an atomic rename.
4. Store a manifest containing source revision, license, hashes, and installed
   paths.
5. Run an isolated health completion before enabling the local runner.
6. Leave the existing cloud runner configured as fallback unless the user
   explicitly disables fallback.

All owned artifacts must stay below these roots:

```text
~/.scopey/models/<model-id>/
~/.scopey/runtimes/llama.cpp/<runtime-version>/<target-triple>/
~/.scopey/run/local-model/
```

Disable stops Scopey from selecting local inference and terminates only the
server whose PID, start token, executable path, and random API key Scopey owns.
It preserves downloaded files. Remove first performs that identity-checked stop,
then removes the selected manifest-listed directories. `--all` removes only the
three local-model roots above, never the broader `~/.scopey` directory.

## Server safety and availability

The managed server must:

- bind to `127.0.0.1` (and optionally `::1`) only;
- disable the Web UI and agent/tool surfaces;
- use a random API key stored with user-only permissions;
- use one slot initially so ordering and memory use are predictable;
- lazily start on the first summarize/judge job and idle-exit after a bounded
  period;
- verify PID identity before stop/remove and recover stale run metadata;
- capture bounded logs under the Scopey run directory;
- fall back to the configured cloud runner on startup, health, timeout, parse,
  or memory failure.

The runtime manifest should initially cover macOS arm64/x86_64, Linux
x86_64/arm64, and Windows x86_64. CPU is the compatibility baseline; Metal is
the default on Apple silicon. CUDA/Vulkan variants can be optional downloads so
the base install stays small and removal remains unambiguous.

## Remaining promotion work

The component choice is complete, but product default promotion still requires:

- a larger adversarial corpus and non-lexical judge review;
- end-to-end hook trajectories for no Scopey, current, and next;
- measurements on Windows and Linux reference machines;
- peak memory, cold-start, idle-exit, and crash-recovery measurements;
- license/notice review and reproducible release manifests;
- implementation of the explicit lifecycle commands above.
