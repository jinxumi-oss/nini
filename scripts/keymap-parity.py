#!/usr/bin/env python3
"""键位语义层对照：Pi 0.85.1 的键位表 ↔ nini 的键位表。

# 为什么要有这个脚本

`scripts/tui-oracle.py` 那一层（屏幕级）在**当前不可用**：
`docs/invariant-audit.md` §5.4 写得很直白——提取器会把表头框线
`├───┼───┤` 配成键名，110 列下 Pi 侧只提到 11 个键（真实约 25 个），
而且 `capture-pane -p` 天然丢颜色与光标。

所以对照换一条路：**从源码取真相，不从屏幕取**。

* Pi 侧：`references/pi-0.85.1/src/ui/keybindings.ts` 的 `KEYBINDINGS`
  （它 `...TUI_KEYBINDINGS` 展开 `src/tui/keybindings.ts`），加上
  `src/ui/interactive-mode.ts` 的 `handleHotkeysCommand`——那是**用户看到的
  那张表**（键由 `getAppKeyDisplay` 在运行期插值，所以描述是硬编码的）。
* nini 侧：`crates/nini-tui/src/keys.rs` 的 `default_keymap()`（键 → 动作）
  与 `crates/nini-tui/src/commands/dispatch.rs` 的 `action_label()`（描述）。

# 比什么（为什么不是「键」）

Pi 的同一个键在不同上下文里是不同动作（`Ctrl+T` 是「显示/隐藏 thinking 块」，
`Home` 是「行首」），而 nini 刻意换过几个键（`Ctrl+-` → `Ctrl+/`，
`Alt+Left` → `Ctrl+Left`）。**按键比对必然误报**，所以主轴是
**binding id → nini 的 KeyAction**，键只作为「是否刻意不同」的附加检查。

# 契约在哪

`scripts/keymap-parity.json` 是这张表的**唯一数据源**（与台账
`scripts/parity-gaps.json` 同一个规矩：不手工维护计数，只引用脚本输出）。
它有五段：

| 段 | 作用 |
|---|---|
| `pi_classes` | Pi 的 binding id → **语义类**（`cursor` / `interrupt` / …） |
| `nini_classes` | nini 的 `KeyAction` → 语义类 |
| `map` | binding id ↔ nini 动作的配对 + 键是否应当相同 |
| `divergences` | **已知**的语义冲突（Pi 与 nini 对同一个动作的理解不同）——每条带理由 |
| `exempt` | Pi 有而 nini **整体不做**的能力（按 glob 豁免）——每条带理由 |

# 判红的三件事

1. **有 id 没登记**：Pi 新增了一个 binding id，契约里没有它 → 退出码 1。
   （这是它作为门禁项的主要价值：Pi 升级后新增的键位**必须**有人裁决。）
2. **配对的动作不存在**：`map` 指向的 nini `KeyAction` 在 `keys.rs` 里没有 → 退出码 1。
3. **语义类对不上**：配对双方的语义类不同，且不在 `divergences` 里 → 退出码 1。
   （`Ctrl+T`、`Ctrl+U`、`Home`/`End`、`Esc`、`Ctrl+C` 就在这里被逮住。）

键的差异**不判红**，但 `map` 里 `keys: "differ"` 的行必须写理由——没有理由的
键差异就是一次无声的分叉。

# 用法

    python3 scripts/keymap-parity.py           # 人读报告，退出码即判定
    python3 scripts/keymap-parity.py --json    # 机读

退出码：0 = 通过；1 = 有违规。
"""

from __future__ import annotations

import argparse
import json
import os
import re
import sys
from pathlib import Path

# 根目录可被环境变量改写：自测要把仓库**复制一份**再改坏它，让 oracle 对着
# 坏副本退出非 0（对着真仓库改来改去不是自测，是破坏）。
ROOT = Path(os.environ.get("NINI_KEYMAP_ROOT") or Path(__file__).resolve().parent.parent)
PI = ROOT / "references" / "pi-0.85.1" / "src"
NINI = ROOT / "crates" / "nini-tui" / "src"
CONTRACT = ROOT / "scripts" / "keymap-parity.json"

# Pi 的键名 → 显示名（`keyDisplayText` 不在快照里，这张表是它的近似；
# 只影响**报告里的可读性**，不影响任何判定）。
KEY_DISPLAY = {
    "ctrl": "Ctrl",
    "shift": "Shift",
    "alt": "Alt",
    "up": "Up",
    "down": "Down",
    "left": "Left",
    "right": "Right",
    "pageUp": "PageUp",
    "pageDown": "PageDown",
    "home": "Home",
    "end": "End",
    "enter": "Enter",
    "escape": "Escape",
    "tab": "Tab",
    "backspace": "Backspace",
    "delete": "Delete",
    "space": "Space",
}


def read(path: Path) -> str:
    return path.read_text(encoding="utf-8")


# ---------------------------------------------------------------------------
# Pi 侧：两张定义表 + 用户可见的那张表
# ---------------------------------------------------------------------------


def _object_body(src: str, const: str) -> str:
    """取 `export const X = { ... }` 的花括号体（按深度配平，不靠正则猜）。"""
    i = src.index(f"export const {const} = {{")
    i = src.index("{", i + len(const))
    depth = 0
    for j in range(i, len(src)):
        if src[j] == "{":
            depth += 1
        elif src[j] == "}":
            depth -= 1
            if depth == 0:
                return src[i : j + 1]
    raise ValueError(f"unbalanced braces for {const}")


def _literal_keys(raw: str) -> list[str]:
    raw = raw.strip()
    if raw.startswith("["):
        return re.findall(r'"([^"]+)"', raw)
    if raw.startswith('"'):
        return [raw.strip('"')]
    return []


def _platform_keys(expr: str) -> list[str]:
    """三元式/平台分支 → **Linux** 上的默认键。

    平台必须钉死：Pi 的键位表带 `process.platform === "win32"` 与
    `windowsKeybindings` 分支，拿 macOS 的键和 nini 比是**另一件事**。
    """
    # process.platform === "win32" ? A : B
    m = re.search(
        r'process\.platform\s*===\s*"win32"\s*\?\s*(\[[^\]]*\]|"[^"]*")\s*:\s*(\[[^\]]*\]|"[^"]*")',
        expr,
    )
    if m:
        return _literal_keys(m.group(2))
    # windowsKeybindings ? A : B
    m = re.search(
        r"windowsKeybindings\s*\?\s*(\[[^\]]*\]|\"[^\"]*\")\s*:\s*(\[[^\]]*\]|\"[^\"]*\")",
        expr,
    )
    if m:
        return _literal_keys(m.group(2))
    # process.platform === "darwin" ? A : B
    m = re.search(
        r'process\.platform\s*===\s*"darwin"\s*\?\s*(\[[^\]]*\]|"[^"]*")\s*:\s*(\[[^\]]*\]|"[^"]*")',
        expr,
    )
    if m:
        return _literal_keys(m.group(2))
    return []


def parse_defs(path: Path, const: str) -> dict[str, dict]:
    """解析 `{ defaultKeys, description }` 形状的定义表。"""
    src = read(path)
    body = _object_body(src, const)
    out: dict[str, dict] = {}
    for m in re.finditer(r'"([\w.]+)":\s*\{', body):
        bid = m.group(1)
        i = m.end() - 1
        depth = 0
        for j in range(i, len(body)):
            if body[j] == "{":
                depth += 1
            elif body[j] == "}":
                depth -= 1
                if depth == 0:
                    break
        chunk = body[i + 1 : j]
        dk = re.search(
            r"defaultKeys:\s*(.+?)(?:,\s*description:|\s*\}\s*$)", chunk, re.S
        )
        ds = re.search(r'description:\s*"([^"]*)"', chunk)
        raw = dk.group(1).strip() if dk else "[]"
        if raw.startswith("[") or raw.startswith('"'):
            keys = _literal_keys(raw)
            expr = None
        else:
            keys = []
            expr = raw
        out[bid] = {
            "keys": keys,
            "expr": expr,
            "desc": ds.group(1) if ds else None,
        }
    return out


def pi_definitions() -> dict[str, dict]:
    tui = parse_defs(PI / "tui" / "keybindings.ts", "TUI_KEYBINDINGS")
    app = parse_defs(PI / "ui" / "keybindings.ts", "KEYBINDINGS")
    merged = dict(tui)
    for k, v in app.items():
        merged.setdefault(k, v)
    for v in merged.values():
        if v["expr"]:
            v["keys"] = _platform_keys(v["expr"])
    return merged


def key_label(key_id: str) -> str:
    parts = key_id.split("+")
    return "+".join(KEY_DISPLAY.get(p, p) for p in parts)


def canon_key(label: str) -> str:
    """键名归一：Pi 的 key id 是小写（`ctrl+shift+p`），nini 的是显示名
    （`Shift+Ctrl+P`）；`Escape` 与 `Esc` 是同一个键。**只影响比较**，
    报告里仍然打原始写法。"""
    parts = [p.lower() for p in label.split("+")]
    parts = ["esc" if p == "escape" else p for p in parts]
    return "+".join(parts)


def pi_hotkeys_rows(defs: dict[str, dict]) -> list[dict]:
    """用户看到的那张表（`handleHotkeysCommand` 的 markdown 模板）。"""
    src = read(PI / "ui" / "interactive-mode.ts")
    body = re.search(
        r"private handleHotkeysCommand\(\): void \{(.*?)\n\t\tlet hotkeys = `", src, re.S
    ).group(1)
    varmap = {
        m.group(1): m.group(2)
        for m in re.finditer(
            r'const (\w+) = this\.get(?:Editor|App)KeyDisplay\("([^"]+)"\)', body
        )
    }
    tmpl = re.search(r"let hotkeys = `(.*?)\n`;", src, re.S).group(1).replace("\\`", "`")
    rows: list[dict] = []
    for line in tmpl.split("\n"):
        line = line.strip()
        if not line.startswith("|"):
            continue
        cells = [c.strip() for c in line.strip("|").split("|")]
        if len(cells) < 2:
            continue
        key_cell, desc = cells[0], cells[1]
        if re.fullmatch(r"[-: ]+", key_cell) or key_cell.lower() == "key":
            continue
        vars_ = re.findall(r"\$\{(\w+)\}", key_cell)
        if not vars_:
            m = re.fullmatch(r"`([^`]+)`", key_cell)
            if m:
                rows.append({"key": m.group(1), "desc": desc, "pi_id": None})
            continue
        for v in vars_:
            bid = varmap.get(v)
            keys = defs.get(bid, {}).get("keys", []) if bid else []
            rows.append(
                {
                    "key": key_label(keys[0]) if keys else "?",
                    "desc": desc,
                    "pi_id": bid,
                }
            )
    return rows


# ---------------------------------------------------------------------------
# nini 侧：键位表与描述
# ---------------------------------------------------------------------------


def nini_keymap() -> list[dict]:
    src = read(NINI / "keys.rs")
    rows: list[dict] = []
    for m in re.finditer(
        r"KeyBinding\s*\{\s*key:\s*(.*?),\s*action:\s*(\w+),?\s*\}", src, re.S
    ):
        rows.append({"key": rust_key_label(m.group(1)), "action": m.group(2)})
    return rows


def rust_key_label(expr: str) -> str:
    mods: list[str] = []
    if "SHIFT" in expr:
        mods.append("Shift")
    if re.search(r"\bctrl\b|KeyModifiers::CTRL", expr):
        mods.append("Ctrl")
    if "ALT" in expr:
        mods.append("Alt")
    mods = [m for m in ("Shift", "Ctrl", "Alt") if m in mods]
    if "Key::enter()" in expr:
        base = "Enter"
    elif "Key::esc()" in expr:
        base = "Esc"
    elif "Key::backspace()" in expr:
        base = "Backspace"
    else:
        mc = re.search(r"KeyCode::(\w+)", expr)
        base = mc.group(1) if mc else "?"
        if base == "Char":
            cm = re.search(r"KeyCode::Char\('(.)'\)", expr)
            base = cm.group(1).upper() if cm else "?"
        elif base == "F":
            fm = re.search(r"KeyCode::F\((\d+)\)", expr)
            base = "F" + (fm.group(1) if fm else "?")
        else:
            base = {
                "Left": "Left",
                "Right": "Right",
                "Up": "Up",
                "Down": "Down",
                "Home": "Home",
                "End": "End",
                "PageUp": "PageUp",
                "PageDown": "PageDown",
                "Tab": "Tab",
                "Delete": "Delete",
            }.get(base, base)
    return "+".join(mods + [base])


def nini_descriptions() -> dict[str, str]:
    src = read(NINI / "commands" / "dispatch.rs")
    m = re.search(
        r"fn action_label\(a: crate::keys::KeyAction\) -> &'static str \{(.*?)\n    \}",
        src,
        re.S,
    )
    if not m:
        return {}
    return {
        mm.group(1): mm.group(2)
        for mm in re.finditer(r"(\w+)\s*=>\s*\"([^\"]*)\"", m.group(1))
    }


def nini_actions() -> set[str]:
    src = read(NINI / "keys.rs")
    m = re.search(r"pub enum KeyAction \{(.*?)\n\}", src, re.S)
    if not m:
        return set()
    return set(re.findall(r"^\s*(\w+),", m.group(1), re.M))


# ---------------------------------------------------------------------------
# 判定
# ---------------------------------------------------------------------------


def glob_match(glob: str, value: str) -> bool:
    return re.fullmatch(glob.replace("*", ".*"), value) is not None


def check(contract: dict, defs: dict, hotkeys: list[dict]) -> dict:
    errors: list[str] = []
    notes: list[str] = []

    pi_classes: dict[str, str] = contract.get("pi_classes", {})
    nini_classes: dict[str, str] = contract.get("nini_classes", {})
    mapping: dict[str, dict] = {m["pi"]: m for m in contract.get("map", [])}
    divergences = {d["pi"]: d for d in contract.get("divergences", [])}
    exempts = contract.get("exempt", [])

    nini_action_set = nini_actions()
    nini_rows = nini_keymap()
    nini_keys_for_action: dict[str, list[str]] = {}
    for r in nini_rows:
        nini_keys_for_action.setdefault(r["action"], []).append(r["key"])

    # 1. 每个有默认键的 Pi id 都必须被登记（map 或 exempt）。
    unmapped: list[str] = []
    for bid, v in sorted(defs.items()):
        if not v["keys"]:
            continue
        if bid in mapping:
            continue
        if any(glob_match(e["glob"], bid) for e in exempts):
            continue
        unmapped.append(bid)
    for bid in unmapped:
        errors.append(
            f"Pi 的 binding id `{bid}` 没有登记：既不在 map 里，也不在任何 exempt 里——"
            f"Pi 新增/改名了一个键位，必须有人裁决它对 nini 意味着什么"
        )

    # 2. map 指向的 nini 动作必须存在；键的期望必须写清。
    for bid, m in mapping.items():
        action = m.get("nini")
        if action is None:
            if not (m.get("note") or "").strip():
                errors.append(
                    f"map 里的 `{bid}` 没有 nini 对应动作，也没有写理由——"
                    f"「Pi 有、nini 没有」必须说明为什么"
                )
            continue
        if action not in nini_action_set:
            errors.append(
                f"map 里的 `{bid}` → nini 动作 `{action}` 在 keys.rs 里不存在"
            )
            continue
        if action not in nini_classes:
            errors.append(
                f"nini 动作 `{action}` 没有在 nini_classes 里登记语义类"
            )
        keys_mode = m.get("keys")
        if keys_mode not in ("same", "differ"):
            errors.append(f"map 里的 `{bid}` 的 keys 必须是 same 或 differ")
        elif keys_mode == "differ" and not (m.get("note") or "").strip():
            errors.append(
                f"map 里的 `{bid}` 标了 keys=differ 却没有写理由——"
                f"没有理由的键差异就是一次无声的分叉"
            )
        elif keys_mode == "same":
            # Pi 的一个动作常带**多个**键（`cursorWordLeft` = Alt+Left / Ctrl+Left /
            # Alt+b），所以判据是「nini 的键落在 Pi 的键集合里」，不是「等于主键」。
            pi_keys = {canon_key(key_label(k)) for k in defs.get(bid, {}).get("keys", [])}
            nini_keys = {canon_key(k) for k in nini_keys_for_action.get(action, [])}
            if pi_keys and nini_keys and not (pi_keys & nini_keys):
                errors.append(
                    f"map 里的 `{bid}` 标了 keys=same，但 nini 的 "
                    f"{sorted(nini_keys)} 与 Pi 的 {sorted(pi_keys)} 没有交集"
                )

    # 3. 语义类必须一致 —— 除非登记为已知冲突。
    conflicts: list[dict] = []
    for bid, m in mapping.items():
        action = m.get("nini")
        if action is None:
            continue
        pc = pi_classes.get(bid)
        nc_set = nini_classes.get(action, [])
        entry_class = m.get("class")
        if pc is None:
            errors.append(f"Pi 的 `{bid}` 没有在 pi_classes 里登记语义类")
            continue
        if not entry_class:
            errors.append(f"map 里的 `{bid}` 没有写 class（这个配对的语义类是什么？）")
            continue
        if entry_class not in nc_set:
            errors.append(
                f"map 里的 `{bid}` 声明语义类 `{entry_class}`，但 nini 动作 "
                f"`{action}` 的语义类是 {sorted(nc_set)}"
            )
            continue
        if pc != entry_class:
            d = divergences.get(bid)
            if not d:
                errors.append(
                    f"`{bid}`（Pi: {pc}）与 nini 的 `{action}`（{entry_class}）语义类不同，"
                    f"且不在 divergences 里——这是语义分叉，必须登记"
                )
            else:
                conflicts.append(
                    {
                        "pi": bid,
                        "nini": action,
                        "pi_class": pc,
                        "nini_class": entry_class,
                        "reason": d.get("reason", ""),
                    }
                )
    for bid in sorted(divergences):
        if bid not in mapping:
            errors.append(
                f"divergences 里的 `{bid}` 在 map 里没有对应行（登记了一个不存在的配对）"
            )

    # 4. map 里的 Pi id 必须真的存在（写错了 id 也要红）。
    for bid in mapping:
        if bid not in defs:
            errors.append(f"map 里的 `{bid}` 不是 Pi 的 binding id（拼错了？）")

    # 5. 只统计**有默认键**的 id：无默认键的（平台专有）不算差距。
    total = sum(1 for v in defs.values() if v["keys"])
    covered = sum(
        1
        for bid, v in defs.items()
        if v["keys"]
        and (
            bid in mapping
            or any(glob_match(e["glob"], bid) for e in exempts)
        )
    )
    return {
        "errors": errors,
        "notes": notes,
        "conflicts": conflicts,
        "pi_total": total,
        "pi_covered": covered,
        "pi_unmapped": unmapped,
        "hotkeys_rows": len(hotkeys),
        "nini_rows": len(nini_rows),
        "mapped": len(mapping),
        "exempted": sum(
            1
            for bid, v in defs.items()
            if v["keys"]
            and bid not in mapping
            and any(glob_match(e["glob"], bid) for e in exempts)
        ),
    }


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--json", action="store_true", help="机读输出")
    args = ap.parse_args()

    if not CONTRACT.exists():
        print(f"缺少契约文件 {CONTRACT}", file=sys.stderr)
        return 2
    contract = json.loads(CONTRACT.read_text(encoding="utf-8"))
    defs = pi_definitions()
    hotkeys = pi_hotkeys_rows(defs)
    result = check(contract, defs, hotkeys)

    if args.json:
        print(json.dumps(result, ensure_ascii=False, indent=2))
        return 1 if result["errors"] else 0

    print("键位语义层对照（Pi 0.85.1 ↔ nini）")
    print(
        f"  Pi 有默认键的 binding id：{result['pi_total']}；"
        f"已登记 {result['pi_covered']}（map {result['mapped']} + exempt {result['exempted']}）"
    )
    print(
        f"  Pi 用户可见的 /hotkeys 行：{result['hotkeys_rows']}；"
        f"nini 键位行：{result['nini_rows']}"
    )
    print()

    if result["conflicts"]:
        print("已知语义冲突（已登记，不判红）：")
        for c in result["conflicts"]:
            print(
                f"  {c['pi']:<34} Pi={c['pi_class']:<18} nini={c['nini_class']:<18} {c['reason']}"
            )
        print()

    if result["errors"]:
        print(f"违规 {len(result['errors'])} 条：")
        for e in result["errors"]:
            print(f"  ✗ {e}")
        return 1

    print("✓ 键位对照通过：Pi 的每个键位 id 都有裁决，配对的语义类一致。")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
