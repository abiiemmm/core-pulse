"""Ensure frontend command names are registered by the Tauri backend."""
from pathlib import Path
import re

frontend = Path("src/api.ts").read_text(encoding="utf-8")
backend = Path("src-tauri/src/lib.rs").read_text(encoding="utf-8")
commands = set(re.findall(r"invoke<[^>]+>\('([^']+)'", frontend))
handler = re.search(r"generate_handler!\[(.*?)\]", backend, re.S)
assert handler, "Tauri handler registration not found"
registered = set(re.findall(r"\b[a-z][a-z0-9_]+\b", handler.group(1)))
missing = commands - registered
assert not missing, f"commands missing from Rust handler: {sorted(missing)}"
print(f"IPC contract OK: {len(commands)} commands registered")
