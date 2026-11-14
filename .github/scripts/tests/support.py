"""Owned Git fixtures shared by release-script tests."""

import os
from pathlib import Path
import subprocess


def git(repo: Path, *args: str) -> str:
    # SECURITY: Hook/wrapper Git routing must never redirect fixture writes.
    return subprocess.run(
        ["git", *args], cwd=repo, check=True, capture_output=True, text=True,
        env={**{key: value for key, value in os.environ.items()
                if not key.startswith("GIT_")}, "GIT_CONFIG_NOSYSTEM": "1",
             "GIT_CONFIG_GLOBAL": os.devnull,
             "GIT_CEILING_DIRECTORIES": str(repo.parent.resolve()),
             "GIT_AUTHOR_DATE": "2000-01-01T00:00:00+0000",
             "GIT_COMMITTER_DATE": "2000-01-01T00:00:00+0000"},
    ).stdout.strip()


def init_repo(repo: Path) -> None:
    repo.mkdir()
    git(repo, "init", "--initial-branch=main")
    git(repo, "config", "user.name", "Release Fixture")
    git(repo, "config", "user.email", "fixture@example.invalid")
    git(repo, "config", "commit.gpgsign", "false")
    git(repo, "config", "tag.gpgsign", "false")
