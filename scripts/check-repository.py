"""Verify sole-maintainer ownership and commit attribution without rewriting history."""
from pathlib import Path
import re
import subprocess

MAINTAINER = "abiiemmm"
AUTHOR_EMAIL = "131965106+abiiemmm@users.noreply.github.com"

# A shallow checkout can hide an earlier author or co-author trailer.
shallow = subprocess.run(
    ["git", "rev-parse", "--is-shallow-repository"],
    check=True, capture_output=True, encoding="utf-8",
).stdout.strip()
if shallow != "false":
    raise SystemExit("Ownership audit requires full history; fetch with --unshallow or fetch-depth: 0.")

owners = [
    line.strip()
    for line in Path(".github/CODEOWNERS").read_text(encoding="utf-8").splitlines()
    if line.strip() and not line.lstrip().startswith("#")
]
if owners != [f"* @{MAINTAINER}"]:
    raise SystemExit("CODEOWNERS must assign every path to the sole maintainer.")

history = subprocess.run(
    ["git", "log", "HEAD", "--format=%H%x1f%ae%x1f%B%x1e"],
    check=True, capture_output=True, encoding="utf-8",
).stdout
count = 0
for record in history.split("\x1e"):
    if not record.strip():
        continue
    commit, email, message = record.strip().split("\x1f", 2)
    if email != AUTHOR_EMAIL:
        raise SystemExit(f"Commit {commit[:12]} has an author outside the maintainer identity.")
    if re.search(r"^Co-authored-by\s*:", message, re.IGNORECASE | re.MULTILINE):
        raise SystemExit(f"Commit {commit[:12]} contains a co-author trailer.")
    count += 1

print(f"Repository ownership OK: {count} commits, sole maintainer, no co-author trailers.")
