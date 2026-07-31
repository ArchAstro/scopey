# Security policy

## Supported versions

Until Scopey publishes versioned releases, security fixes are made on the
default branch only. After releases begin, this file will list supported release
lines explicitly.

## Reporting a vulnerability

Please use GitHub's private vulnerability reporting flow in the repository's
Security tab. Do not open a public issue for a suspected vulnerability.

Include the affected command or hook, impact, reproduction steps, and any
suggested mitigation. Remove credentials, real prompts, transcripts, and other
sensitive data from the report. Maintainers will acknowledge the report and
coordinate disclosure after assessing its impact.

## Security-relevant behavior

Scopey:

- reads coding-agent event payloads and may store prompts and tool summaries
  under `~/.scopey/`;
- modifies supported harness hook configuration during `scopey setup`;
- invokes a locally installed agent CLI for scope extraction and judging;
- may execute a user-configured `model_command` through the system shell; and
- sends desktop or Herdr notifications when configured.

Treat Scopey's configuration and data directory as sensitive. Only use a
`model_command` from a configuration you trust.
