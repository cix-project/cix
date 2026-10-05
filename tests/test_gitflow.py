import unittest

from tools.check_gitflow import evaluate, route_allowed


class GitflowTests(unittest.TestCase):
    def test_ordinary_contributions(self):
        for prefix in ("feature", "fix", "docs", "chore", "refactor", "test", "build", "ci"):
            with self.subTest(prefix=prefix):
                self.assertTrue(route_allowed("develop", prefix + "/topic", False))
                self.assertFalse(route_allowed("main", prefix + "/topic", True))

    def test_release_promotions_require_same_repository(self):
        for prefix in ("release", "hotfix"):
            self.assertTrue(route_allowed("main", prefix + "/1.0", True))
            self.assertFalse(route_allowed("main", prefix + "/1.0", False))
        self.assertFalse(route_allowed("main", "develop", True))

    def test_backmerge_and_stabilization(self):
        self.assertTrue(route_allowed("develop", "main", True))
        self.assertFalse(route_allowed("develop", "main", False))
        self.assertTrue(route_allowed("release/1.0", "fix/crash", True))
        self.assertFalse(route_allowed("release/1.0", "feature/new", True))
        self.assertTrue(route_allowed("hotfix/crash", "test/regression", True))

    def test_dependabot_identity_and_repository_are_required(self):
        for base in ("main", "develop", "release/1.0"):
            self.assertTrue(route_allowed(base, "dependabot/github_actions/update", True, True))
            self.assertFalse(route_allowed(base, "dependabot/github_actions/update", True, False))
            self.assertFalse(route_allowed(base, "dependabot/github_actions/update", False, True))

    def test_invalid_names_and_targets(self):
        self.assertFalse(route_allowed("develop", "feature/", True))
        self.assertFalse(route_allowed("develop", "feature-malicious", True))
        self.assertFalse(route_allowed("unknown", "feature/topic", True))

    def test_activation_fails_closed(self):
        event = {"pull_request": {
            "base": {"ref": "develop", "repo": {"id": 1}},
            "head": {"ref": "feature/topic", "repo": {"id": 2}},
        }}
        for value in (None, "", "false", "True", "1"):
            self.assertFalse(evaluate(event, value)[0])
        self.assertTrue(evaluate(event, "true")[0])
        self.assertTrue(evaluate({}, "false")[0])

    def test_deleted_fork_does_not_impersonate_release(self):
        event = {"pull_request": {
            "base": {"ref": "main", "repo": {"id": 1}},
            "head": {"ref": "release/1.0", "repo": None},
        }}
        self.assertFalse(evaluate(event, "true")[0])


if __name__ == "__main__":
    unittest.main()
