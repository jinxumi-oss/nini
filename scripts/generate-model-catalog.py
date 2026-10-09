"""Generate nini's model catalog from Pi 0.84.2's own data files.

Provenance matters more than convenience here: every number in the output
comes from a file Pi ships, not from a hand-typed table that drifts the
moment a model is added. The generator refuses to invent anything - a
missing field is an error, not a default.
"""

import glob
import json
import os
import sys
from collections import OrderedDict

PI_DATA = (
    "/home/jin/.nvm/versions/node/v25.9.0/lib/node_modules/"
    "@earendil-works/pi-coding-agent/node_modules/"
    "@earendil-works/pi-ai/dist/providers/data"
)

# Fields carried over verbatim from Pi's model entries. Anything not listed
# is dropped on purpose; see CARRIED below.
REQUIRED = ["id", "name", "api", "provider", "reasoning", "input", "cost",
            "contextWindow", "maxTokens"]


def collect():
    out = OrderedDict()
    for path in sorted(glob.glob(os.path.join(PI_DATA, "*.json"))):
        blob = json.load(open(path))
        for api, models in blob.items():
            if not isinstance(models, dict):
                continue
            for mid, m in models.items():
                missing = [k for k in REQUIRED if k not in m]
                if missing:
                    print(f"MISSING {m.get('provider')}/{mid}: {missing}",
                          file=sys.stderr)
                    sys.exit(1)
                compat = m.get("compat") or {}
                out[f"{m['provider']}/{mid}"] = OrderedDict([
                    ("id", mid),
                    ("name", m["name"]),
                    ("api", m.get("api", api)),
                    ("provider", m["provider"]),
                    ("reasoning", bool(m["reasoning"])),
                    ("input", list(m["input"])),
                    ("context_window", m["contextWindow"]),
                    ("max_tokens", m["maxTokens"]),
                    ("cost", OrderedDict([
                        ("input", m["cost"]["input"]),
                        ("output", m["cost"]["output"]),
                        ("cache_read", m["cost"]["cacheRead"]),
                        ("cache_write", m["cost"]["cacheWrite"]),
                    ])),
                    # Per-model wire quirks Pi carries. `supportsTemperature`
                    # is the one that matters today: Pi's Anthropic path
                    # omits temperature when the model declares it false
                    # (anthropic-messages.ts:182, :1021).
                    ("supports_temperature",
                     bool(compat.get("supportsTemperature", True))),
                    ("thinking_levels",
                     sorted((m.get("thinkingLevelMap") or {}).keys())),
                ])
    return out


if __name__ == "__main__":
    models = collect()
    print(f"collected {len(models)} models across "
          f"{len({m['provider'] for m in models.values()})} providers",
          file=sys.stderr)
    with open(sys.argv[1], "w") as f:
        json.dump(list(models.values()), f, indent=1, sort_keys=False)
        f.write("\n")
