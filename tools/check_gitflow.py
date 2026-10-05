"""Enforce the preparation hold and the documented Gitflow PR routes."""

import json
import os
from pathlib import Path


def prefixed(branch, prefixes):
    return any(branch.startswith(prefix + "/") and len(branch) > len(prefix) + 1
               for prefix in prefixes)


def route_allowed(base, head, same_repository, dependabot=False):
    if base == "main":
        return same_repository and (
            prefixed(head, ("release", "hotfix"))
            or (dependabot and prefixed(head, ("dependabot",)))
        )
    if base == "develop":
        return (
            prefixed(head, ("feature", "fix", "docs", "chore", "refactor",
                            "test", "build", "ci"))
            or (same_repository and head == "main")
            or (same_repository and prefixed(head, ("hotfix",)))
            or (same_repository and dependabot and prefixed(head, ("dependabot",)))
        )
    if prefixed(base, ("release",)):
        return same_repository and (
            prefixed(head, ("fix", "hotfix", "docs", "chore"))
            or (dependabot and prefixed(head, ("dependabot",)))
        )
    if prefixed(base, ("hotfix",)):
        return same_repository and prefixed(head, ("fix", "docs", "chore", "test"))
    return False


def evaluate(event, approval):
    pr = event.get("pull_request")
    if not pr:
        return True, "Setup verification only; no development is activated."
    if approval != "true":
        return False, "Development is paused pending explicit owner activation."
    base = pr["base"]["ref"]
    head = pr["head"]["ref"]
    same = pr["head"].get("repo") is not None and (
        pr["head"]["repo"]["id"] == pr["base"]["repo"]["id"]
    )
    author = pr.get("user", {})
    bot = author.get("login") == "dependabot[bot]" and author.get("type") == "Bot"
    allowed = route_allowed(base, head, same, bot)
    return allowed, ("Gitflow route accepted." if allowed else
                     "Invalid Gitflow route; see docs/gitflow.md.")


def main():
    path = os.environ.get("GITHUB_EVENT_PATH")
    if not path:
        raise SystemExit("GITHUB_EVENT_PATH is required; use the unit tests locally.")
    event = json.loads(Path(path).read_text(encoding="utf-8"))
    ok, message = evaluate(event, os.environ.get("CIX_DEVELOPMENT_APPROVED", "false"))
    print(message)
    raise SystemExit(0 if ok else 1)


if __name__ == "__main__":
    main()
