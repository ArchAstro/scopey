#!/usr/bin/env python3
"""JSON-protocol adapter for a local OpenAI-compatible model server."""

from __future__ import annotations

import argparse
import json
import os
import sys
import time
import urllib.error
import urllib.request


OPERATIONS = ["ADD", "SUBTRACT", "MODIFY", "REPLACE", "QUERY", "ADMIN", "MACHINE_EVENT"]
RESPONSE_SCHEMA = {
    "type": "object",
    "properties": {
        "operations": {
            "type": "array",
            "items": {"type": "string", "enum": OPERATIONS},
            "minItems": 1,
            "uniqueItems": True,
        },
        "requirements": {
            "type": "array",
            "items": {"type": "string"},
            "minItems": 1,
            "maxItems": 15,
        },
        "queries": {
            "type": "array",
            "items": {"type": "string"},
            "maxItems": 5,
        },
        "boundaries": {
            "type": "array",
            "items": {"type": "string"},
            "maxItems": 5,
        },
    },
    "required": ["operations", "requirements", "queries", "boundaries"],
    "additionalProperties": False,
}


def render_scope(payload: dict[str, object]) -> str:
    operations = payload.get("operations")
    requirements = payload.get("requirements")
    queries = payload.get("queries", [])
    boundaries = payload.get("boundaries", [])
    if (
        not isinstance(operations, list)
        or not operations
        or any(operation not in OPERATIONS for operation in operations)
    ):
        raise ValueError("model returned invalid operations")
    if (
        not isinstance(requirements, list)
        or not requirements
        or len(requirements) > 15
        or any(not isinstance(item, str) or not item.strip() for item in requirements)
    ):
        raise ValueError("model returned invalid requirements")
    for name, values in (("queries", queries), ("boundaries", boundaries)):
        if not isinstance(values, list) or any(
            not isinstance(item, str) or not item.strip() for item in values
        ):
            raise ValueError(f"model returned invalid {name}")
    unique_operations = list(dict.fromkeys(operations))
    unique_items = list(
        dict.fromkeys(item.strip() for item in [*requirements, *queries, *boundaries])
    )
    bullets = "\n".join(f"- {item}" for item in unique_items)
    return f"<!-- scope-transition: {','.join(unique_operations)} -->\n{bullets}"


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--url",
        default=os.environ.get("SCOPEY_LOCAL_MODEL_URL", "http://127.0.0.1:18080"),
    )
    parser.add_argument("--model", default="local")
    parser.add_argument("--model-id", required=True)
    parser.add_argument("--timeout", type=float, default=30)
    parser.add_argument("--max-tokens", type=int, default=256)
    parser.add_argument("--seed", type=int, default=42)
    args = parser.parse_args()

    started = time.perf_counter()
    try:
        request = json.load(sys.stdin)
        body = {
            "model": args.model,
            "messages": [
                {
                    "role": "system",
                    "content": "You are a deterministic scope-state transformer. Preserve only explicitly requested requirements.",
                },
                {"role": "user", "content": request["rendered_prompt"]},
            ],
            "temperature": 0,
            "max_tokens": args.max_tokens,
            "seed": args.seed,
            "stream": False,
            "response_format": {
                "type": "json_schema",
                "json_schema": {
                    "name": "scope_transition",
                    "strict": True,
                    "schema": RESPONSE_SCHEMA,
                },
            },
        }
        http_request = urllib.request.Request(
            args.url.rstrip("/") + "/v1/chat/completions",
            data=json.dumps(body).encode("utf-8"),
            headers={"Content-Type": "application/json"},
        )
        with urllib.request.urlopen(http_request, timeout=args.timeout) as response:
            completion = json.load(response)
        content = completion["choices"][0]["message"]["content"]
        output = render_scope(json.loads(content))
        json.dump(
            {
                "output": output,
                "adapter_elapsed_ms": round((time.perf_counter() - started) * 1000, 3),
                "runner": "openai-compatible-local",
                "model": args.model_id,
                "server_fingerprint": completion.get("system_fingerprint"),
                "usage": completion.get("usage"),
                "timings": completion.get("timings"),
                "seed": args.seed,
            },
            sys.stdout,
        )
        return 0
    except Exception as exc:
        if isinstance(exc, urllib.error.HTTPError):
            detail = exc.read().decode("utf-8", errors="replace")[-1000:]
        else:
            detail = str(exc)
        json.dump(
            {
                "error": f"{type(exc).__name__}: {detail}",
                "adapter_elapsed_ms": round((time.perf_counter() - started) * 1000, 3),
                "runner": "openai-compatible-local",
                "model": args.model_id,
            },
            sys.stdout,
        )
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
