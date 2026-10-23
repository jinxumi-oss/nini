# Fixtures

Session files used by `nini-conformance`. **Read this before trusting one.**

## `pi-0.85.1-session.jsonl` —— **当前对照物**

由 `scripts/pi-session-record.sh` 用**脚本化的 mock provider 驱动真 Pi**
（本机安装的 **0.85.1**）录制。

**只洗路径**（`cwd` → `/scrubbed/project`）；**id、parentId、时间戳、正文原样保留**。

它证明两件事，而旧 fixture 只能证明一件：

1. **行级 schema** —— Pi 写的每一行，nini 都要能解析（`session_parity_0_85_1.rs`
   钉住 `failed == 0`）；
2. **图结构** —— 父子链**全部可解析**（同文件的 `the_fixture_proves_the_graph_too`），
   所以它能测 `get_path`、压缩重建、分支选择这类**走树**的行为。

它还包含**完整的回合形状**：user → assistant(`toolCall`) → `toolResult` →
assistant(text) → user → assistant(text)。工具调用与工具结果**在 Pi 的会话里是留痕的**。

**复算**：

```console
bash scripts/pi-session-record.sh --out /tmp/rec.jsonl
```

时间戳与 id 每次录制都不同（那是真 Pi 写的），所以 CI 比的是**结构**
（条目类型与 role 序列），不是逐字节。

## `pi-0.84.2-session.jsonl` —— **历史证据**（不再是当前对照物）

> 目标已统一到 **0.85.1**（见 `references/TARGET.json`）。这个文件是
> **Pi 0.84.2** 写的，它证明的是「nini 也能读旧版本的会话」；
> **当前对照物**是上面的 `pi-0.85.1-session.jsonl`。

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
