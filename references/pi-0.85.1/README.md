# Pi 0.85.1 机制面快照

- 来源：`/tmp/pi-0.85.1/node_modules/@earendil-works/pi-coding-agent`（npm 包 `@earendil-works/pi-coding-agent@0.85.1`）
- 生成：`python3 scripts/pi-snapshot.py --version 0.85.1 --root /tmp/pi-0.85.1/node_modules/@earendil-works/pi-coding-agent --out references/pi-0.85.1`
- 校验：`python3 scripts/pi-snapshot.py --version 0.85.1 --root /tmp/pi-0.85.1/node_modules/@earendil-works/pi-coding-agent --out references/pi-0.85.1 --check`

`dist/` 是包里实际发布的 .js；`src/` 是从 `.js.map` 的 `sourcesContent` 逐字还原的 .ts；
`maps/` 是还原过程的证据。三个目录都在 `SNAPSHOT.lock` 的 sha256 锁里。

**这份快照是 nini 行为层对照物的唯一真相。** 行号引用必须能在
`dist/` 或 `src/` 里解析出来；`scripts/pi-oracle-lock.py --verify-refs` 会逐条检查，
解析不到就退出 1。
