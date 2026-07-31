/**
 * scopey — Pi coding agent extension
 *
 * Installed by `scopey setup` into ~/.pi/agent/extensions/scopey.ts
 * Reloads with Pi restart or /reload.
 *
 * Maps Pi lifecycle events → `scopey hook <event>` with Claude-compatible JSON.
 * Sets SCOPEY_HARNESS=pi and never runs when SCOPEY_INTERNAL=1.
 */
import { spawnSync } from "node:child_process";
import type { ExtensionAPI } from "@mariozechner/pi-coding-agent";

function scopeyBin(): string {
  return process.env.SCOPEY_BIN || "scopey";
}

function sessionId(ctx: any): string {
  try {
    const f = ctx?.sessionManager?.getSessionFile?.();
    if (typeof f === "string" && f.length) {
      // Use basename without extension as stable-ish id
      const base = f.split("/").pop() || f;
      return base.replace(/\.(jsonl?|pi)$/i, "") || base;
    }
  } catch {
    /* ignore */
  }
  return process.env.SCOPEY_SESSION_ID || `pi-${process.pid}`;
}

function callScopey(
  hook: "session-start" | "user-prompt" | "post-tool" | "stop",
  payload: Record<string, unknown>,
): string {
  if (process.env.SCOPEY_INTERNAL === "1" || process.env.SCOPEY_HOOKS_DISABLED === "1") {
    return "";
  }
  const body = {
    harness: "pi",
    ...payload,
  };
  const r = spawnSync(scopeyBin(), ["hook", hook], {
    input: JSON.stringify(body),
    encoding: "utf8",
    env: {
      ...process.env,
      SCOPEY_HARNESS: "pi",
      SCOPEY_SESSION_ID: String(body.session_id || ""),
    },
    timeout: 20_000,
  });
  if (r.error) {
    console.error(`[scopey/pi] spawn error: ${r.error.message}`);
    return "";
  }
  if (r.status !== 0 && r.stderr) {
    console.error(`[scopey/pi] ${hook}: ${r.stderr.slice(0, 400)}`);
  }
  return (r.stdout || "").trim();
}

function parseAdditionalContext(stdout: string): string | undefined {
  if (!stdout) return undefined;
  try {
    const j = JSON.parse(stdout);
    return (
      j?.hookSpecificOutput?.additionalContext ||
      j?.additionalContext ||
      undefined
    );
  } catch {
    return undefined;
  }
}

export default function scopeyExtension(pi: ExtensionAPI): void {
  pi.on("session_start", async (event, ctx) => {
    const sid = sessionId(ctx);
    callScopey("session-start", {
      session_id: sid,
      cwd: event.cwd || process.cwd(),
      hook_event_name: "SessionStart",
      source: event.reason || "startup",
      transcript_path: ctx.sessionManager?.getSessionFile?.() || null,
    });
  });

  pi.on("before_agent_start", async (event, ctx) => {
    const sid = sessionId(ctx);
    const prompt = event.prompt || "";
    const stdout = callScopey("user-prompt", {
      session_id: sid,
      cwd: event.systemPromptOptions?.cwd || process.cwd(),
      hook_event_name: "UserPromptSubmit",
      prompt,
      transcript_path: ctx.sessionManager?.getSessionFile?.() || null,
    });
    const extra = parseAdditionalContext(stdout);
    if (extra) {
      return {
        message: {
          customType: "scopey",
          content: extra,
          display: false,
        },
      };
    }
  });

  // Count each completed tool as one post-tool event (scopey batches/throttles internally).
  pi.on("tool_result", async (event, ctx) => {
    const sid = sessionId(ctx);
    callScopey("post-tool", {
      session_id: sid,
      cwd: process.cwd(),
      hook_event_name: "PostToolUse",
      tool_name: event.toolName,
      tool_input: event.input,
      transcript_path: ctx.sessionManager?.getSessionFile?.() || null,
    });
  });

  pi.on("agent_end", async (_event, ctx) => {
    const sid = sessionId(ctx);
    const stdout = callScopey("stop", {
      session_id: sid,
      cwd: process.cwd(),
      hook_event_name: "Stop",
      transcript_path: ctx.sessionManager?.getSessionFile?.() || null,
    });
    const extra = parseAdditionalContext(stdout);
    if (extra) {
      try {
        ctx.ui?.notify?.(`scopey: ${extra.slice(0, 120)}`, "warning");
      } catch {
        /* ignore */
      }
    }
  });
}
