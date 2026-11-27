# Langfuse renderer sources

This directory mirrors the imported paths from Langfuse v4.88.0 at commit `9eea2e60a1222936d78a28a0f0b2cafe690fc2a5`. The imported files retain the upstream MIT notice in [LICENSE](LICENSE). [Upstream source](https://github.com/langfuse/langfuse/tree/9eea2e60a1222936d78a28a0f0b2cafe690fc2a5)

- `web/src/components/nav` and `web/src/components/ui` provide the sidebar and table elements.
- `web/src/components/design-system` provides the data table, code sections, timeline, dialogs, and shared controls.
- `web/src/features/topics` provides the facet and view tabs, treemap, and interactive scatter canvas.
- `web/src/styles/globals.css` provides the design tokens and Tailwind styles. `packages/icons/src/create-icon.tsx` provides the icon factory.

The nearby `langfuse-bridge` directory adapts client navigation, appearance, clipboard access, and Topics data to Electron. The build maps the upstream analytics hooks to local no-ops. Confuse pages send requests through the existing Electron preload to Rust.
