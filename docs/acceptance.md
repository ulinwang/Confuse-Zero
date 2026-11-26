# Confuse-Zero acceptance rules

Confuse is an Apple Silicon macOS application for one macOS user and multiple isolated projects. It reuses the Electron desktop shell from Confuse and bundles the Rust backend as a child process of the application. The release candidate is a local-install DMG. Langfuse v4.88.0 is the behavior and API baseline. The earlier Confuse desktop at commit 043c7f90338654a14d7bb81e9cf4101c48e5851a is the Agent behavior reference.

## What counts as passing

- Use [the matrix](acceptance.tsv) as the release checklist. Each row needs a named automated check or reproducible real-device check, expected results, and evidence from the candidate commit. A screenshot cannot replace a data/API assertion. A compiled build cannot replace a visual or lifecycle check.
- Use synthetic conversations and isolated Agent roots for capture, failure injection, and uninstall tests. Real-device detection may inspect installed executables, app bundle identifiers, package metadata, and directory metadata. Release evidence contains only synthetic data.
- Compare user-visible flows with the upstream open-source pages and compare status, authentication, defaults, filters, pagination, and JSON fields with the pinned [Fern contracts](https://github.com/langfuse/langfuse/tree/v4.88.0/fern/apis/server/definition). The desktop equivalents of organization membership, scheduled export, and notifications are macOS user ownership, local folders, and the Confuse notification center.
- The packaged Electron application and Rust backend run with Docker, separately installed Node.js, Python, and service daemons absent. Run the bundled app from Finder with its usual limited PATH. Run official SDK compatibility checks separately using Python Langfuse 5.2.0 and JavaScript Langfuse 6.1.1.
- Load only packaged Electron front-end assets, with context isolation, sandboxing and an explicit preload bridge. Verify Electron quits its own Rust process, both listeners close, and the next start resumes work.
- Capture the window on a physical Apple Silicon Mac. Inspect default and minimum window sizes in light and dark appearance. Record installer checksum, installed size, cold start to interactive window, and idle resident memory without Agent jobs running. Record both the result and its measurement conditions.

## Agent fixtures

| Source | Minimum fixture coverage |
| --- | --- |
| Kimi Code | Current workspace/Agent Wire stream, incremental turns and tool events, custom root. |
| Codex | Desktop app bundle and CLI paths, linked session and turn IDs, plain and Zstandard rollout, archived rename, interrupted final line. |
| Claude Code | CLI executable, project transcripts, subagent/tool linkage, token and cache usage, custom profile. |
| DeepSeek Harness | Use the Harness package or bundle identifier to identify dsh. Include current versioned JSONL, Zstandard frames, torn final frame and restored append. |
| Pi | Official package/installer identity, fnm and restricted Finder PATH, same-name command, default and configured directories, branched JSONL, resumed append, tool results and token use. |

Use a fixed capture boundary. Importing with history disabled yields zero existing messages. Restarting an enabled adapter imports eligible records written after the last committed cursor, including records written while Confuse was closed. Importing or replaying the same eligible records leaves both trace counts and usage totals unchanged. An unavailable cursor or malformed record is reported with counts, without dumping log contents into diagnostics.

## Topics fixtures

Use separate synthetic cohorts for all four built-in facets, a fixed clock, and deterministic summary, embedding, and naming responses. Feed both an Agent-derived generation and real SDK spans to the same Topics workflow. Verify secondary filtering in the tracing table and open the linked trace. At least one test checks the embedding dimensions, insufficient sample state, provider error and recovery, caching across a refresh, and deletion cleanup. Test the live daily schedule, the default five-minute readiness delay, and a fixed-range snapshot. Verify project isolation in the published topic results and read-only API/MCP tools. The three model connections use the normal project endpoint configuration.

## Release procedure

Merge this specification before application changes. Every implementation PR runs the relevant matrix cases and the repository checks. Each PR references matrix IDs. Record real-device outcomes, installation and removal evidence, fixture/SDK test output and screenshots against the last release candidate SHA. Update matrix evidence with failed or unverified rows explicitly. Declare the first release accepted after every row passes.

Primary references

- [Pinned Langfuse source and license](https://github.com/langfuse/langfuse/tree/v4.88.0)
- [Langfuse OpenTelemetry contract](https://langfuse.com/integrations/native/opentelemetry)
- [Topics behavior](https://langfuse.com/docs/observability/features/topics)
- [Pi 1.6.0 session format](https://github.com/earendil-works/pi/blob/v1.6.0/packages/coding-agent/docs/session-format.md)
- [Earlier Confuse Agent adapters](https://github.com/ulinwang/Confuse/tree/043c7f90338654a14d7bb81e9cf4101c48e5851a/desktop/src/supervisor)
