# Pi 0.84.2 behaviour-layer snapshot

The format layer is frozen in `../spec-v0.85.1/` (sha256 per file). This
directory freezes the **behaviour** layer for the Pi version nini's line-number
oracles were written against.

## Why

nini's oracle comments cite Pi internals by file and line
(`agent-loop.js:160`, `keybindings.ts:208`). Those citations were only
verifiable by hand, so every Pi upgrade silently moved them. This snapshot
plus `scripts/pi-oracle-lock.py` turns them into a machine check:

    python3 scripts/pi-oracle-lock.py --check         # snapshot bytes unchanged
    python3 scripts/pi-oracle-lock.py --verify-refs   # every citation re-resolved

## Contents

| path | what it is |
|---|---|
| `dist/*.js` | the compiled files as shipped by the npm package (`@earendil-works/pi-coding-agent` 0.84.2, and `@earendil-works/pi-agent-core` 0.84.2 for `agent-loop.js`) |
| `src/*.ts` | the original TypeScript sources, restored **verbatim** from the `sourcesContent` field of the matching `dist/*.js.map` |
| `maps/*.js.map` | the source maps themselves, kept so the `src/*.ts` restoration is reproducible and auditable |
| `SNAPSHOT.lock` | sha256 + byte size of every file above, plus the pi version and capture date |

## Citation conventions

* `*.js` citations refer to the **compiled** bundle → `dist/`.
* `*.ts` citations refer to the **TypeScript** source → `src/`.

The two do not have the same line numbering; a citation such as
`agent-loop.js:582` exists only in the `.ts` file. `scripts/pi-oracle-lock.py`
resolves each citation against the file that matches its extension and
reports `DRIFT` when the line number is past the end of that file.

## Source

    /home/jin/.nvm/versions/node/v25.9.0/lib/node_modules/@earendil-works/pi-coding-agent

Captured 2026-10-23. Do not edit these files by hand — regenerate the lock
instead, or the check will (correctly) fail.
