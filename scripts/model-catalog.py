#!/usr/bin/env python3
"""Report on nini's model catalog and, optionally, refresh it from an
upstream model-metadata source.

v0.10 (R3): the previous version of this script read
`<nvm prefix>/lib/node_modules/@earendil-works/pi-ai/dist/providers/data`,
hardcoded. nini is a rewrite, not a client: its build should not depend on
another project's node_modules layout, and the script failed outright on a
machine where that path did not exist.

What this script does now:

  * with no arguments, reports what nini currently ships - how many models,
    which providers, whether the file is well formed;
  * with `--from <dir>`, regenerates from a directory of upstream model
    metadata in the shape nini uses. The directory is supplied by the
    caller. It is not discovered, and nothing on this machine is required
    for nini to build.

The committed `model-catalog.json` is nini's own data file. Where its
numbers came from is provenance, recorded in the file's first entry and
worth keeping honest; it is not a build dependency.
"""

import argparse
import json
import os
import sys

REPO = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
CATALOG = os.path.join(REPO, "crates", "nini-core", "data", "model-catalog.json")

# What an UPSTREAM entry must carry. Anything absent is an error rather than
# a default: a guessed context window is the defect this project has been
# removing one at a time.
UPSTREAM_REQUIRED = [
    "id", "name", "api", "provider", "reasoning", "input", "cost",
    "contextWindow", "maxTokens",
]

# What nini's own file must carry. These are nini's names, not the
# upstream's - the earlier version of this script checked the upstream
# spelling against nini's file and reported all 112 entries incomplete.
REQUIRED = [
    "id", "name", "api", "provider", "reasoning", "input",
    "context_window", "max_tokens", "cost",
]

# The providers nini can actually reach. Carrying 1267 models for 39
# providers would be a catalog that lies about what is usable, and it was
# 515KB of mostly-dead weight.
REACHABLE = {"anthropic", "openai", "deepseek", "groq", "mistral", "google"}


def report() -> int:
    if not os.path.exists(CATALOG):
        print(f"missing: {CATALOG}", file=sys.stderr)
        return 1
    with open(CATALOG) as f:
        models = json.load(f)

    providers = sorted({m["provider"] for m in models})
    windows = sorted({m["context_window"] for m in models})
    print(f"catalog: {CATALOG}")
    print(f"  models    : {len(models)}")
    print(f"  providers : {', '.join(providers)}")
    print(f"  windows   : {len(windows)} distinct, {windows[0]}..{windows[-1]}")

    missing = [m.get("id", "?") for m in models
               if not all(k in m for k in REQUIRED)]
    if missing:
        print(f"  INCOMPLETE: {len(missing)} entries missing required fields",
              file=sys.stderr)
        return 1

    fixed = [m["context_window"] for m in models]
    if len(set(fixed)) == 1:
        print("  SUSPECT: every model shares one context window", file=sys.stderr)
        return 1
    return 0


def regenerate(source_dir: str) -> int:
    """Rebuild the catalog from a caller-supplied directory of JSON files
    shaped `{ "<api>": { "<model-id>": { ... } } }`."""
    if not os.path.isdir(source_dir):
        print(f"not a directory: {source_dir}", file=sys.stderr)
        return 1

    out = {}
    for name in sorted(os.listdir(source_dir)):
        if not name.endswith(".json"):
            continue
        with open(os.path.join(source_dir, name)) as f:
            blob = json.load(f)
        if not isinstance(blob, dict):
            continue
        for api, models in blob.items():
            if not isinstance(models, dict):
                continue
            for mid, m in models.items():
                if not isinstance(m, dict):
                    continue
                missing = [k for k in UPSTREAM_REQUIRED if k not in m]
                if missing:
                    print(f"MISSING {m.get('provider')}/{mid}: {missing}",
                          file=sys.stderr)
                    return 1
                compat = m.get("compat") or {}
                cost = m["cost"]
                out[f"{m['provider']}/{mid}"] = {
                    # nini names models `provider/model` everywhere, so the
                    # catalog does too.
                    "id": f"{m['provider']}/{mid}",
                    "name": m["name"],
                    "api": m.get("api", api),
                    "provider": m["provider"],
                    "reasoning": bool(m["reasoning"]),
                    "input": list(m["input"]),
                    "context_window": m["contextWindow"],
                    "max_tokens": m["maxTokens"],
                    "cost": {
                        "input": cost["input"],
                        "output": cost["output"],
                        "cache_read": cost.get("cacheRead", 0),
                        "cache_write": cost.get("cacheWrite", 0),
                    },
                    "supports_temperature": bool(
                        compat.get("supportsTemperature", True)),
                    "thinking_levels": sorted((m.get("thinkingLevelMap") or {}).keys()),
                    "cost_tiers": [
                        {
                            "input_tokens_above": t["inputTokensAbove"],
                            "input": t["input"],
                            "output": t["output"],
                            "cache_read": t.get("cacheRead", 0),
                            "cache_write": t.get("cacheWrite", 0),
                        }
                        for t in (cost.get("tiers") or [])
                    ],
                }

    if not out:
        print(f"no models found under {source_dir}", file=sys.stderr)
        return 1

    all_count = len(out)
    ordered = [out[k] for k in sorted(out) if out[k]["provider"] in REACHABLE]
    if not ordered:
        print("no models for the providers nini implements", file=sys.stderr)
        return 1
    with open(CATALOG, "w") as f:
        json.dump(ordered, f, indent=1)
        f.write("\n")
    print(
        f"wrote {len(ordered)} models to {CATALOG} "
        f"({all_count} seen, {all_count - len(ordered)} for providers nini "
        f"does not implement)"
    )
    return 0


if __name__ == "__main__":
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument(
        "--from", dest="source",
        help="directory of upstream model-metadata JSON files to rebuild from",
    )
    args = ap.parse_args()
    sys.exit(regenerate(args.source) if args.source else report())