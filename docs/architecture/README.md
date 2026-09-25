# nini Architecture — Code Relationship Diagrams

Generated for v0.8.0-pre1 (2026-09-25).

## How to render

These are Mermaid `.mmd` files. Render with:

- VS Code: install the "Mermaid Preview" extension
- CLI: `npm i -g @mermaid-js/mermaid-cli && mmdc -i nini-crate-deps.mmd -o out.svg`
- Web: paste into https://mermaid.live/

## Diagrams

1. **`nini-crate-deps.mmd`** — 7 crates + their dependency edges
2. **`nini-tui-internals.mmd`** — nini-tui's 36 modules + import edges + LOC
3. **`nini-agent-flow.mmd`** — runtime sequence: user → TUI → Agent → Provider → Tool
4. **`nini-hook-system.mmd`** — AgentLoopHooks trait + 9 hook call sites in Agent::run
