"""Patches the upstream Playground bundle for the Rust server image.

1. index.html carries a Segment analytics snippet that loads a third-party
   script on every page view and ignores the `telemetry: false` the server
   sends, so it is removed.
2. The Frontend Integrations page prints copy-paste addresses under
   `/cubejs-api`; this server serves the API under `/cube`.

Usage: python3 patch-playground.py <playground-dir>
"""

import pathlib
import re
import sys

target = pathlib.Path(sys.argv[1])

index = target / "index.html"
html = index.read_text(encoding="utf-8")
snippet = re.compile(
    r"\n?\s*<script>(?:(?!</script>).)*?analytics\.load\((?:(?!</script>).)*?</script>",
    re.DOTALL,
)
html, removed = snippet.subn("\n    <!-- Segment analytics removed -->", html)
if "cdn.segment.com" in html:
    sys.exit("Found the Segment snippet but could not remove it; check index.html")
index.write_text(html, encoding="utf-8")
print(f"index.html: removed {removed} analytics snippet(s)")

if not (target / "vizard" / "index.html").is_file():
    sys.exit("vizard/index.html is missing: build packages/cubejs-playground/vizard first")

# Order matters: the full URL before the bare prefix.
replacements = {
    "`http://localhost:4000/cubejs-api`": "`http://localhost:4000/cube`",
    "`ws://localhost:4000/`": "`ws://localhost:4000/cube/ws`",
    "`/cubejs-api`": "`/cube`",
}
patched = 0
for script in sorted(target.glob("assets/*.js")):
    source = script.read_text(encoding="utf-8")
    result = source
    for old, new in replacements.items():
        result = result.replace(old, new)
    if result != source:
        script.write_text(result, encoding="utf-8")
        patched += 1
if patched == 0:
    sys.exit("Frontend Integrations addresses not found; upstream may have changed them")
for script in sorted(target.glob("assets/*.js")):
    if "/cubejs-api" in script.read_text(encoding="utf-8"):
        sys.exit(f"{script.name} still refers to /cubejs-api")
print(f"assets: patched {patched} file(s)")
