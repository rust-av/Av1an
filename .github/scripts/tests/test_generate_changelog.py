"""Offline generate changelog contracts."""

import os
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch

from support import git, init_repo


class GenerateChangelogTests(unittest.TestCase):
    def _changelog(
        self, repo: Path, output: Path, *args: str,
    ) -> subprocess.CompletedProcess[str]:
        script = Path(__file__).resolve().parents[1] / "generate-changelog.sh"
        return subprocess.run(
            ["bash", str(script), *args], cwd=repo, capture_output=True, text=True,
            env={**{key: value for key, value in os.environ.items()
                    if not key.startswith("GIT_")}, "GITHUB_SHA": "OFFLINE_HISTORY_SHA",
                 "GITHUB_OUTPUT": str(output), "GIT_CONFIG_NOSYSTEM": "1",
                 "GIT_CONFIG_GLOBAL": os.devnull,
                 "GIT_CEILING_DIRECTORIES": str(repo.parent.resolve())},
        )

    def test_helpers_isolate_git_environment_and_parent_discovery(self) -> None:
        """Inherited routing must not write to a containing repo or leak its history."""
        with tempfile.TemporaryDirectory(prefix="changelog isolation ") as directory:
            base = Path(directory)
            env = {key: os.environ[key] for key in ("PATH", "SYSTEMROOT")
                   if key in os.environ}
            env["TMPDIR"] = str(base)
            # SECURITY: Never route the unsafe RED helpers to the actual project.
            with patch.dict(os.environ, env, clear=True):
                outer = base / "outer repo"
                init_repo(outer)
                (outer / "sentinel").write_text("preserve outer index and worktree\n")
                git(outer, "add", "--", "sentinel")
                git(outer, "commit", "-m", "HISTORY_OUTER")
                git(outer, "tag", "v9.0.0")
                git(outer, "config", "user.name", "Outer Sentinel")

                def outer_state() -> dict[str, bytes]:
                    git_dir = outer / ".git"
                    paths = [git_dir / name for name in
                             ("HEAD", "config", "index", "packed-refs")]
                    paths.extend((git_dir / "refs").rglob("*"))
                    return {str(path.relative_to(git_dir)): path.read_bytes()
                            for path in paths if path.is_file()}

                before = outer_state()
                inner = outer / "inner repo"
                output = base / "inner output"
                polluted = {
                    "GIT_DIR": str(outer / ".git"),
                    "GIT_COMMON_DIR": str(outer / ".git"),
                    "GIT_WORK_TREE": str(outer),
                    "GIT_INDEX_FILE": str(outer / ".git/index"),
                    "GIT_OBJECT_DIRECTORY": str(outer / ".git/objects"),
                    "GIT_ALTERNATE_OBJECT_DIRECTORIES": str(outer / ".git/objects"),
                    "GIT_CONFIG": str(outer / ".git/config"),
                    "GIT_CONFIG_COUNT": "1",
                    "GIT_CONFIG_KEY_0": "user.name",
                    "GIT_CONFIG_VALUE_0": "Injected Outer Identity",
                    "GIT_AUTHOR_NAME": "Injected Outer Author",
                    "GIT_CEILING_DIRECTORIES": str(base),
                }
                with patch.dict(os.environ, polluted):
                    init_repo(inner)
                    git(inner, "commit", "--allow-empty", "-m", "HISTORY_INNER")
                    git(inner, "tag", "v0.5.1")
                    result = self._changelog(inner, output)

                self.assertEqual(outer_state(), before, "outer HEAD/config/index/refs changed")
                self.assertTrue((inner / ".git").is_dir())
                self.assertEqual(git(inner, "log", "-1", "--format=%s"), "HISTORY_INNER")
                self.assertEqual(git(inner, "tag", "--list"), "v0.5.1")
                self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
                body = output.read_text()
                self.assertEqual(
                    [line[2:] for line in body.splitlines() if line.startswith("- ")],
                    ["HISTORY_INNER"],
                )
                self.assertIn(": HISTORY_INNER (Release Fixture)\n", body)

                not_git = outer / "not git"
                not_git.mkdir()
                with self.assertRaises(subprocess.CalledProcessError):
                    git(not_git, "rev-parse", "--show-toplevel")
                output.write_text("existing=true\n")
                result = self._changelog(not_git, output)
                self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
                self.assertEqual(output.read_text(), "existing=true\n")
                self.assertEqual(outer_state(), before)

    def test_previous_reachable_version_excludes_head_aliases_and_latest(self) -> None:
        """Current, moving, non-version, or numerically largest tags are not baselines."""
        root = ("commit", "--allow-empty", "-m", "HISTORY_ROOT")
        change = ("commit", "--allow-empty", "-m", "HISTORY_CHANGE")
        head = ("commit", "--allow-empty", "-m", "HISTORY_HEAD")
        previous = ("tag", "-a", "v0.5.1", "-m", "previous release")
        cases = (
            ("firsttag", [root, change, ("tag", "v0.5.1")],
             ["HISTORY_CHANGE", "HISTORY_ROOT"], "the beginning of history"),
            ("secondtag", [root, previous, change, ("tag", "v0.5.2")],
             ["HISTORY_CHANGE"], "v0.5.1"),
            ("masteraftertag", [root, previous, change, head],
             ["HISTORY_HEAD", "HISTORY_CHANGE"], "v0.5.1"),
            ("stale-latest-ancestor", [root, previous, change, ("tag", "latest"), head],
             ["HISTORY_HEAD", "HISTORY_CHANGE"], "v0.5.1"),
            ("moving-latest-head", [root, previous, change, ("tag", "latest")],
             ["HISTORY_CHANGE"], "v0.5.1"),
            ("multiple-head-aliases", [root, previous, change,
             ("tag", "v0.5.2"), ("tag", "-a", "v0.5.3", "-m", "alias"),
             ("tag", "latest")], ["HISTORY_CHANGE"], "v0.5.1"),
            ("untagged-first-history", [root, change],
             ["HISTORY_CHANGE", "HISTORY_ROOT"], "the beginning of history"),
            ("ignore-nonrelease-tags", [root, previous, change,
             ("tag", "build-2026"), ("tag", "v9.9.9-preview"),
             ("tag", "v8.8.8junk"), ("tag", "v7.7.7.7"), head],
             ["HISTORY_HEAD", "HISTORY_CHANGE"], "v0.5.1"),
            ("nearest-not-largest", [root, ("tag", "v9.0.0"), change, previous, head],
             ["HISTORY_HEAD"], "v0.5.1"),
        )
        with tempfile.TemporaryDirectory(prefix="changelog git ") as directory:
            base = Path(directory)
            for name, commands, expected, baseline in cases:
                with self.subTest(history=name):
                    repo = base / name
                    init_repo(repo)
                    for command in commands:
                        git(repo, *command)
                    output = base / f"{name} output"

                    result = self._changelog(repo, output)

                    self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
                    body = output.read_text()
                    self.assertEqual(
                        [line[2:] for line in body.splitlines() if line.startswith("- ")],
                        expected,
                    )
                    self.assertIn(f"Changes since {baseline}:\n", body)
                    self.assertNotIn("No changes", body)

    def test_merge_uses_release_on_second_parent_not_unreachable_tag(self) -> None:
        """Walking only HEAD's first parent loses the reachable side-branch release."""
        with tempfile.TemporaryDirectory(prefix="changelog merge ") as directory:
            base = Path(directory)
            repo = base / "repo"
            init_repo(repo)
            for command in (
                ("commit", "--allow-empty", "-m", "HISTORY_ROOT"),
                ("branch", "unreachable"),
                ("checkout", "-b", "side"),
                ("commit", "--allow-empty", "-m", "HISTORY_PREVIOUS"),
                ("tag", "v0.5.1"),
                ("commit", "--allow-empty", "-m", "HISTORY_SIDE"),
                ("checkout", "main"),
                ("commit", "--allow-empty", "-m", "HISTORY_MAIN"),
                ("merge", "--no-ff", "side", "-m", "HISTORY_MERGE"),
                ("tag", "v0.5.2"),
                ("checkout", "unreachable"),
                ("commit", "--allow-empty", "-m", "HISTORY_UNREACHABLE"),
                ("tag", "v99.0.0"),
                ("checkout", "main"),
            ):
                git(repo, *command)
            output = base / "output"

            result = self._changelog(repo, output)

            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            body = output.read_text()
            self.assertCountEqual(
                [line[2:] for line in body.splitlines() if line.startswith("- ")],
                ["HISTORY_MERGE", "HISTORY_MAIN", "HISTORY_SIDE"],
            )
            self.assertIn("Changes since v0.5.1:\n", body)

    def test_body_protocol_notes_and_first_ten_commit_boundary(self) -> None:
        """Keep every subject exactly once, correct overflow counts, and spaced paths."""
        with tempfile.TemporaryDirectory(prefix="changelog body ") as directory:
            base = Path(directory)
            for count in (1, 10, 11, 12):
                with self.subTest(commits=count):
                    repo = base / f"repo {count}"
                    init_repo(repo)
                    git(repo, "commit", "--allow-empty", "-m", "HISTORY_BASE")
                    git(repo, "tag", "v0.5.1")
                    subjects = [f"history change {i:02d} `quotes` & spaces" for i in range(count)]
                    for subject in subjects:
                        git(repo, "commit", "--allow-empty", "-m", subject)
                    notes = base / "release notes with spaces.md"
                    notes.write_text("Offline release notes\nverbatim, no final newline")
                    output = base / f"output {count}"
                    output.write_text("existing=true\n")

                    result = self._changelog(repo, output, str(notes))

                    self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
                    body = output.read_text()
                    self.assertTrue(body.startswith(
                        "existing=true\nbody<<CHANGELOG_EOF\nLatest build from OFFLINE_HISTORY_SHA\n"
                    ))
                    self.assertIn(f": {subjects[-1]} (Release Fixture)\n", body)
                    self.assertEqual(
                        [line[2:] for line in body.splitlines() if line.startswith("- ")],
                        subjects[::-1],
                    )
                    inline = body.split("<details>")[0]
                    self.assertEqual(inline.count("\n- history change"), min(count, 10))
                    if count > 10:
                        self.assertIn(f"<summary>... and {count - 10} more commits</summary>", body)
                        self.assertEqual(body.count("</details>"), 1)
                    else:
                        self.assertNotIn("<details>", body)
                    self.assertTrue(body.endswith(f"\n---\n\n{notes.read_text()}\nCHANGELOG_EOF\n"))

    def test_shallow_and_git_errors_fail_before_output_mutation(self) -> None:
        """Shallow roots must not masquerade as first releases, nor Git errors as success."""
        with tempfile.TemporaryDirectory(prefix="changelog errors ") as directory:
            base = Path(directory)
            source = base / "source"
            init_repo(source)
            git(source, "commit", "--allow-empty", "-m", "HISTORY_ROOT")
            git(source, "tag", "v0.5.1")
            git(source, "commit", "--allow-empty", "-m", "HISTORY_CHANGE")
            git(source, "clone", "--depth=1", source.as_uri(), str(base / "shallow"))
            init_repo(base / "empty")
            (base / "not-git").mkdir()
            for name in ("shallow", "empty", "not-git"):
                with self.subTest(history=name):
                    output = base / f"{name} output"
                    output.write_text("existing=true\n")

                    result = self._changelog(base / name, output)

                    self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
                    self.assertEqual(output.read_text(), "existing=true\n")
                    self.assertNotIn("No changes", result.stdout + result.stderr)
                    if name == "shallow":
                        self.assertIn("shallow", result.stderr.lower())
                        self.assertIn("fetch-depth: 0", result.stderr)


if __name__ == "__main__":
    unittest.main()
