/**
 * scopey — OpenCode plugin
 *
 * Installed by `scopey setup` into ~/.config/opencode/plugins/scopey.js
 * Loaded automatically on OpenCode startup.
 *
 * Maps OpenCode plugin hooks → `scopey hook <event>` with normalized JSON.
 */
import { spawnSync } from "node:child_process";

function scopeyBin() {
  return process.env.SCOPEY_BIN || "scopey";
}

function callScopey(hook, payload) {
  if (process.env.SCOPEY_INTERNAL === "1" || process.env.SCOPEY_HOOKS_DISABLED === "1") {
    return "";
  }
  const body = { harness: "opencode", ...payload };
  const r = spawnSync(scopeyBin(), ["hook", hook], {
    input: JSON.stringify(body),
    encoding: "utf8",
    env: {
      ...process.env,
      SCOPEY_HARNESS: "opencode",
      SCOPEY_SESSION_ID: String(body.session_id || ""),
      OPENCODE_SESSION_ID: String(body.session_id || ""),
    },
    timeout: 20_000,
  });
  if (r.error) {
    console.error(`[scopey/opencode] spawn error: ${r.error.message}`);
    return "";
  }
  if (r.status !== 0 && r.stderr) {
    console.error(`[scopey/opencode] ${hook}: ${String(r.stderr).slice(0, 400)}`);
  }
  return (r.stdout || "").trim();
}

export const ScopeyPlugin = async ({ directory, worktree }) => {
  const cwd = directory || worktree || process.cwd();
  // OpenCode subagents (task tool) run as child sessions whose session
  // object carries a parent id. Scope belongs to the top-level conversation,
  // so child-session events never reach scopey. The plugin instance lives for
  // the whole OpenCode process, so this set covers the process lifetime; the
  // scopey binary additionally drops anything tagged with a parent id.
  const childSessions = new Set();

  return {
    event: async ({ event }) => {
      const type = event?.type || "";
      const props = event?.properties || event || {};
      const info = props.info || props.session || props;
      const sid =
        info.id ||
        props.sessionID ||
        props.sessionId ||
        props.id ||
        process.env.OPENCODE_SESSION_ID ||
        `opencode-${process.pid}`;
      const parentID =
        info.parentID || info.parentId || info.parent_id || props.parentID;

      if (parentID) {
        childSessions.add(sid);
        return; // subagent/child session: scopey stays out entirely
      }
      if (childSessions.has(sid)) {
        return;
      }

      if (type === "session.created" || type === "session.updated") {
        callScopey("session-start", {
          session_id: sid,
          cwd,
          hook_event_name: "SessionStart",
          source: type,
        });
      }

      if (type === "session.idle") {
        callScopey("stop", {
          session_id: sid,
          cwd,
          hook_event_name: "Stop",
          source: "session.idle",
        });
      }
    },

    "tool.execute.after": async (input, _output) => {
      const sid =
        input?.sessionID ||
        input?.sessionId ||
        process.env.OPENCODE_SESSION_ID ||
        `opencode-${process.pid}`;
      if (childSessions.has(sid)) {
        return; // tool ran inside a subagent/child session
      }
      callScopey("post-tool", {
        session_id: sid,
        cwd,
        hook_event_name: "PostToolUse",
        tool_name: input?.tool,
        tool_input: input?.args || input,
      });
    },
  };
};

// Default export for loaders that expect one.
export default ScopeyPlugin;
