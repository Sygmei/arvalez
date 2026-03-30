import json
import sys
from collections import Counter

report = sys.argv[1]

with open(report) as f:
    data = json.load(f)

failures = []
for r in data.get("results", []):
    # Top-level spec failure (parse/load errors)
    if r.get("failure"):
        failures.append({"spec": r["spec"], "target": "<load>", **r["failure"]})
    # Per-target failures (codegen errors)
    for t in r.get("targets", []):
        if t.get("failure"):
            failures.append({"spec": r["spec"], "target": t["name"], **t["failure"]})

print(f"Total failures: {len(failures)}")
codes = Counter(f.get("kind", f.get("error_code", "unknown")) for f in failures)
for code, count in codes.most_common():
    print(f"  {count:3d}  {code}")

print()
print("=== Sample messages per error_code ===")
by_code = {}
for f in failures:
    c = f.get("kind", f.get("error_code", "unknown"))
    by_code.setdefault(c, []).append(
        (
            f["spec"],
            f.get("message", f.get("error_message", "")),
            f.get("feature", f.get("error_context", "")),
        )
    )

for code, items in by_code.items():
    print(f"\n-- {code} --")
    seen = set()
    for spec, msg, ctx in items:
        if ctx not in seen:
            seen.add(ctx)
            print(f"  [{spec}] feature={ctx}")
            print(f"  msg: {msg[:180]}")
        if len(seen) >= 6:
            break
