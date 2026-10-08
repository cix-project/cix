"""Dependency-free checks for the governance scaffold (not CIX product tests)."""

import ast
import hashlib
import json
from pathlib import Path
import re

ROOT = Path(__file__).resolve().parents[1]
EXCLUDED = {".git", "__pycache__", ".venv"}
REQUIRED = (
    "README.md", "AGENTS.md", "CONTRIBUTING.md", "SECURITY.md",
    "docs/gitflow.md", "docs/activation.md", ".github/CODEOWNERS",
    ".github/dependabot.yml", ".github/workflows/governance.yml",
)


def main():
    errors = []
    exception_path = ROOT / "tools/local-source-byte-exceptions.json"
    exceptions = {}
    if hashlib.sha256(exception_path.read_bytes()).hexdigest() != "ad24d804119c1f37b20536356fa8c173b0a2e0edf0d1bc41d9445226c35d644b":
        errors.append("Preserved-source exception manifest hash mismatch")
    else:
        document = json.loads(exception_path.read_text())
        rows = document["exceptions"]
        if document["schema"] != "cix-exact-preserved-source-bytes-v1" or len(rows) != 142:
            errors.append("Invalid preserved-source exception manifest")
        else:
            exceptions = {row["path"]: row["sha256"] for row in rows}
            if len(exceptions) != 142:
                errors.append("Duplicate preserved-source exception path")
            for relative, expected in exceptions.items():
                selected = ROOT / relative
                if (Path(relative).is_absolute() or ".." in Path(relative).parts
                        or not selected.resolve().is_relative_to(ROOT.resolve())
                        or not selected.is_file()
                        or hashlib.sha256(selected.read_bytes()).hexdigest() != expected):
                    errors.append(f"Preserved-source byte identity mismatch: {relative}")
    for relative in REQUIRED:
        if not (ROOT / relative).is_file():
            errors.append(f"Missing required file: {relative}")
    for path in sorted(ROOT.rglob("*")):
        if not path.is_file() or EXCLUDED.intersection(path.relative_to(ROOT).parts):
            continue
        relative = path.relative_to(ROOT)
        if path.stat().st_size > 1024 * 1024:
            errors.append(f"Scaffold file exceeds 1 MiB: {relative}")
            continue
        data = path.read_bytes()
        exact_preserved = (relative.as_posix() in exceptions and
                           hashlib.sha256(data).hexdigest() == exceptions[relative.as_posix()])
        try:
            text = data.decode("utf-8")
        except UnicodeDecodeError:
            if not exact_preserved:
                errors.append(f"Unexpected binary file in scaffold: {relative}")
            continue
        if not exact_preserved and ("\r" in text or (text and not text.endswith("\n"))):
            errors.append(f"Use LF and a final newline: {relative}")
        for line in text.splitlines():
            if not exact_preserved and line.rstrip() != line:
                errors.append(f"Trailing whitespace: {relative}")
            if re.match(r"^(<{7}|={7}|>{7})( |$)", line):
                errors.append(f"Possible unresolved merge marker: {relative}")
        if path.suffix == ".py":
            try:
                ast.parse(text, filename=str(relative))
            except SyntaxError as exc:
                errors.append(str(exc))
        if path.parent == ROOT / ".github/workflows":
            for action in re.findall(r"(?m)^\s*-?\s*uses:\s*(\S+)", text):
                if not re.fullmatch(r"[\w.-]+/[\w./-]+@[0-9a-f]{40}", action):
                    errors.append(f"Action must use a full commit SHA: {action}")
            if re.search(r"\bpull_request_target\s*:", text):
                errors.append(f"Privileged PR trigger is not allowed: {relative}")
    if errors:
        raise SystemExit("\n".join(errors))
    print("Repository scaffold checks passed; no product validation is implied.")


if __name__ == "__main__":
    main()
