"""Exercise the ownership audit against isolated Git histories, never the worktree."""
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

AUDIT = Path(__file__).resolve().parents[1] / "scripts" / "check-repository.py"
OWNER_EMAIL = "131965106+abiiemmm@users.noreply.github.com"


class RepositoryOwnershipTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="core-pulse-ownership-")
        self.addCleanup(self.temporary.cleanup)
        self.repo = Path(self.temporary.name) / "source"
        self.repo.mkdir()
        # Make fixture commits independent of the user's Git identity/configuration.
        self.environment = {
            key: value for key, value in os.environ.items()
            if not key.startswith("GIT_")
        }
        self.environment.update({
            "GIT_CONFIG_NOSYSTEM": "1",
            "GIT_CONFIG_GLOBAL": os.devnull,
            "GIT_AUTHOR_NAME": "abiiemmm",
            "GIT_AUTHOR_EMAIL": OWNER_EMAIL,
            "GIT_COMMITTER_NAME": "abiiemmm",
            "GIT_COMMITTER_EMAIL": OWNER_EMAIL,
            "GIT_TERMINAL_PROMPT": "0",
        })
        self.git("init", "-b", "main")
        (self.repo / ".github").mkdir()
        (self.repo / ".github" / "CODEOWNERS").write_text("* @abiiemmm\n", encoding="utf-8")
        self.git("add", ".github/CODEOWNERS")

    def git(self, *arguments, environment=None, cwd=None):
        return subprocess.run(
            ["git", *arguments], cwd=cwd or self.repo,
            env=self.environment | (environment or {}),
            check=True, capture_output=True, encoding="utf-8",
        ).stdout.strip()

    def commit(self, message="Fixture commit", **environment):
        return self.git("commit", "--allow-empty", "-m", message, environment=environment)

    def audit(self, expected_error=None, cwd=None):
        result = subprocess.run(
            [sys.executable, str(AUDIT)], cwd=cwd or self.repo,
            env=self.environment, capture_output=True, encoding="utf-8",
        )
        if expected_error:
            self.assertNotEqual(result.returncode, 0, result.stdout)
            self.assertIn(expected_error, result.stderr)
        else:
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertIn("sole maintainer, no co-author trailers", result.stdout)

    def test_accepts_owner_email_with_historical_name_and_web_committer(self):
        self.commit(GIT_AUTHOR_NAME="Kucing Pungut", GIT_COMMITTER_NAME="GitHub",
                    GIT_COMMITTER_EMAIL="noreply@github.com")
        self.commit()
        self.audit()

    def test_rejects_foreign_author(self):
        self.commit(GIT_AUTHOR_EMAIL="someone@example.invalid")
        self.audit("author outside the maintainer identity")

    def test_rejects_foreign_ancestor_even_with_owner_head(self):
        self.commit(GIT_AUTHOR_EMAIL="someone@example.invalid")
        self.commit()
        self.audit("author outside the maintainer identity")

    def test_rejects_coauthor_trailer_in_ancestor(self):
        self.commit("Fixture\n\ncO-AuThOrEd-By: Someone <someone@example.invalid>")
        self.commit()
        self.audit("co-author trailer")

    def test_rejects_additional_codeowner(self):
        (self.repo / ".github" / "CODEOWNERS").write_text(
            "* @abiiemmm @someone\n", encoding="utf-8")
        self.git("add", ".github/CODEOWNERS")
        self.commit()
        self.audit("assign every path to the sole maintainer")

    def test_rejects_path_specific_owner_override(self):
        (self.repo / ".github" / "CODEOWNERS").write_text(
            "* @abiiemmm\nsrc/ @someone\n", encoding="utf-8")
        self.git("add", ".github/CODEOWNERS")
        self.commit()
        self.audit("assign every path to the sole maintainer")

    def test_rejects_shallow_history_hiding_foreign_ancestor(self):
        self.commit(GIT_AUTHOR_EMAIL="someone@example.invalid")
        self.commit()
        clone = self.repo.parent / "shallow"
        self.git("clone", "--depth", "1", self.repo.as_uri(), str(clone))
        self.audit("requires full history", cwd=clone)

    def test_audits_pr_head_without_exempting_published_github_merge(self):
        self.commit()
        self.git("switch", "-c", "feature")
        self.commit("Feature")
        feature_head = self.git("rev-parse", "HEAD")
        self.git("switch", "main")
        self.commit("Base update")
        self.git("merge", "--no-ff", "feature", "-m", "Synthetic PR merge", environment={
            "GIT_AUTHOR_NAME": "GitHub", "GIT_AUTHOR_EMAIL": "noreply@github.com",
        })
        self.audit("author outside the maintainer identity")
        self.git("checkout", "--detach", feature_head)
        self.audit()


if __name__ == "__main__":
    unittest.main()
