# Fixtures

Session files used by `nini-conformance`. **Read this before trusting one.**

## `pi-0.84.2-session.jsonl`

Real Pi 0.84.2 output with values scrubbed: text bodies, paths and UUIDs
were replaced so the file carries no user data. **Every key name, enum
value and container shape is verbatim Pi.**

What it proves: that `nini` reads every line Pi writes without dropping
one. `session_parity.rs` pins `skipped == 0` against it, and a second test
in the same file checks the fixture still covers the whole schema — a
fixture that silently stops covering something is worse than no fixture.

What it does **not** prove: anything about the session *graph*. Scrubbing
replaced each entry id with an 8-character hash, and the replacement was
not carried through to `parentId` references. Of the 21 non-null parents,
**4 resolve**; the other 17 point at ids that appear nowhere in the file.
Only the first four entries form a chain.

So this file validates **schema coverage, line by line**. It cannot test
`get_path`, compaction reconstruction, branch selection, or anything else
that walks the tree. Those are tested against sessions built with real
parents — see `nini-tui/tests/p02_tree.rs`, which constructs that shape on
purpose rather than borrowing it from here.

The last line is not Pi output: a `custom` message with
`customType: "assistant"` and the body `<text>`, appended by hand so the
fixture covers the custom-message shape. Its full-length uuid gives it
away against the scrubbed 8-character ids.

**Do not repair the dangling parent links.** A reconstructed chain would be
our invention wearing Pi's name, and the next reader would trust it.

## `pi-0.84.2-fresh.jsonl`

A short, well-formed v3 session — and unlike the file above, its parent
chain is intact: `9ac83ce5 -> 08e517d8 -> bcd938af -> 6d5302e4`. It covers
`model_change`, `thinking_level_change` (with `thinkingLevel`), a user
message and an assistant message carrying a `thinking` block.

`session_fresh.rs` reads it as *"a session written by the installed Pi
minutes ago"*, so it doubles as a freshness check: if Pi's on-disk format
moves, this is the fixture that stops matching reality first.

## `pi-v4-unsupported.jsonl`

One line, a v4 header (`{"v":4,"kind":"header",...}`), which nini does not
support. `an_unreadable_format_is_skipped_not_fatal` pins that the reader
skips it and reports the count rather than failing the whole load.

It sat here unreferenced until P2, which is the same class of problem this
directory has had twice: a fixture that implies coverage no test is
actually checking.
