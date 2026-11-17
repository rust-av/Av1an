#!/usr/bin/env python3
"""Offline Docker workflow contract check; run with Python 3 + PyYAML.

Executes the workflow's credential/intent shell steps with public dummy values;
never invokes Docker, an action, a network request, or real credentials.
"""

import fnmatch
from pathlib import Path
import re
import shutil
import subprocess
import tempfile
from types import SimpleNamespace

import yaml


ROOT = Path(__file__).resolve().parents[2]
WORKFLOW = yaml.load(
    (ROOT / ".github/workflows/tests.yml").read_text(), Loader=yaml.BaseLoader
)
TOKEN = "OFFLINE_TOKEN_DO_NOT_PRINT"
SHA = "1234567890abcdef1234567890abcdef12345678"


def expression(value: str, context: dict) -> object:
    """Evaluate the boolean/equality/startsWith expressions used by this workflow."""
    value = value.strip().removeprefix("${{").removesuffix("}}").strip()
    # ponytail: bounded workflow expressions, not a GitHub Actions interpreter.
    return eval(
        value.replace("&&", " and ").replace("||", " or "),
        {"__builtins__": {}, "startsWith": str.startswith},
        context,
    )


def expand(value: str, context: dict) -> str:
    return re.sub(
        r"\$\{\{(.*?)\}\}",
        lambda match: str(expression(match[1], context)),
        value,
    )


def run_step(step: dict, context: dict) -> tuple[str, str]:
    """Run only a selected shell step, with no inherited credentials or PATH."""
    with tempfile.TemporaryDirectory() as temp:
        output = Path(temp) / "output"
        output.touch()
        env = {"PATH": "/usr/bin:/bin", "GITHUB_OUTPUT": str(output)}
        env.update({key: expand(value, context) for key, value in step["env"].items()})
        result = subprocess.run(
            [shutil.which("bash") or "bash", "--noprofile", "--norc", "-e", "-c", step["run"]],
            env=env, capture_output=True, text=True, check=True,
        )
        logs = result.stdout + result.stderr
        assert TOKEN not in logs + output.read_text(), "credential leaked"
        return output.read_text(), logs


def metadata(inputs: dict, context: dict) -> list[str]:
    """Render the actual branch/semver/SHA recipes; no action runtime is executed."""
    image = expand(inputs["images"], context)
    ref = context["github"].ref
    # ponytail: stable/rc fixtures only; extend when adding other version cases.
    version = re.fullmatch(r"refs/tags/v(\d+)\.(\d+)\.(\d+)(-rc\.\d+)?", ref)
    tags = []
    latest = False
    for recipe in inputs["tags"].splitlines():
        fields = dict(part.split("=", 1) for part in recipe.split(","))
        kind = fields["type"]
        if kind == "ref" and fields["event"] == "branch":
            if ref.startswith("refs/heads/"):
                tags.append(ref.removeprefix("refs/heads/"))
        elif kind == "semver":
            if version:
                major, minor, patch, prerelease = version.groups()
                full = f"{major}.{minor}.{patch}{prerelease or ''}"
                tags.append(full if prerelease else fields["pattern"]
                            .replace("{{version}}", full).replace("{{major}}", major)
                            .replace("{{minor}}", minor))
                latest = not prerelease
        elif kind == "sha":
            tags.append("sha-" + SHA[:7])
        elif kind == "raw":
            if expression(fields["enable"], context):
                tags.append(fields["value"])
        else:
            raise AssertionError(f"unsupported metadata recipe: {recipe}")
    if latest and inputs.get("flavor", "latest=auto") == "latest=auto":
        tags.append("latest")
    return [f"{image}:{tag}" for tag in dict.fromkeys(tags)]


def main() -> None:
    """Catch unreachable releases, untrusted publication, leaks and wrong tags."""
    jobs = WORKFLOW["jobs"]
    triggers = WORKFLOW["on"]
    assert "v*" in triggers["push"].get("tags", []), "release tags cannot trigger CI"
    build = jobs["docker"]
    assert build["needs"] == ["formatting", "unit-tests"]
    assert "if" not in build, "build must not depend on publish readiness"
    assert "secrets." not in str(build), "build must not receive credentials"
    build_action = next(s for s in build["steps"] if s.get("id") == "docker_build")
    assert build_action["with"]["push"] == "false", "validation must never push"
    for job in jobs.values():
        assert "secrets." not in str(job.get("outputs", {})), "secret job output"
    publish = jobs["docker-publish"]
    assert set(publish["needs"]) == {"unit-tests", "docker"}
    steps = {step.get("id", step.get("name")): step for step in publish["steps"]}
    credential = steps["credentials"]
    assert "if" not in credential, "trusted jobs must check credential readiness"
    meta = steps["docker_meta"]
    login = steps["Login to DockerHub"]
    push = steps["docker_build"]
    intent = steps["Show publish intent"]
    assert push["with"]["tags"] == "${{ steps.docker_meta.outputs.tags }}"
    assert push["with"]["push"] == "true"

    upstream = "rust-av/Av1an"
    both = ("offline-owner", TOKEN)
    # Literal expected outcomes are independent of the workflow gate/recipes.
    cases = [
        ("main", upstream, "push", "refs/heads/main", both, True,
         ["main", "sha-1234567"]),
        ("release", upstream, "push", "refs/tags/v1.2.3", both, True,
         ["1.2.3", "1.2", "1", "sha-1234567", "latest"]),
        ("prerelease", upstream, "push", "refs/tags/v1.2.3-rc.1", both, True,
         ["1.2.3-rc.1", "sha-1234567"]),
        ("fork-pr", "fork/Av1an", "pull_request", "refs/pull/1/merge", both, False, []),
        ("upstream-pr", upstream, "pull_request", "refs/pull/1/merge", both, False, []),
        ("fork-push", "fork/Av1an", "push", "refs/heads/main", both, False, []),
        ("fork-tag", "fork/Av1an", "push", "refs/tags/v1.2.3", both, False, []),
        ("manual-feature", upstream, "workflow_dispatch", "refs/heads/feature", both, False, []),
        ("manual-main", upstream, "workflow_dispatch", "refs/heads/main", both, False, []),
        ("manual-tag", upstream, "workflow_dispatch", "refs/tags/v1.2.3", both, False, []),
        ("missing-owner", upstream, "push", "refs/heads/main", ("", TOKEN), True, []),
        ("missing-token", upstream, "push", "refs/heads/main", ("offline-owner", ""), True, []),
        ("missing-both", upstream, "push", "refs/tags/v1.2.3", ("", ""), True, []),
        ("feature-push", upstream, "push", "refs/heads/feature", both, False, []),
        ("other-tag", upstream, "push", "refs/tags/other", both, False, []),
        ("invalid-version", upstream, "push", "refs/tags/vnope", both, True,
         ["sha-1234567"]),
    ]
    for name, repo, event, ref, secrets, trusted, wanted in cases:
        github = SimpleNamespace(repository=repo, event_name=event, ref=ref)
        context = {"github": github}  # Secrets are unavailable until trusted.
        allowed = bool(expression(publish["if"], context))
        assert allowed == trusted, f"{name}: trust gate"
        tags = []
        if allowed:
            context["secrets"] = SimpleNamespace(
                DOCKERHUB_USERNAME=secrets[0], DOCKERHUB_TOKEN=secrets[1]
            )
            output, logs = run_step(credential, context)
            ready = bool(secrets[0] and secrets[1])
            assert output == f"ready={str(ready).lower()}\n", f"{name}: boolean output"
            assert not secrets[0] or secrets[0] not in output + logs
            context["steps"] = SimpleNamespace(
                credentials=SimpleNamespace(outputs=SimpleNamespace(ready=str(ready).lower()))
            )
            for step in (meta, login, push, intent):
                assert bool(expression(step["if"], context)) == ready, f"{name}: step gate"
            if ready:
                assert expand(login["with"]["username"], context) == secrets[0]
                assert expand(login["with"]["password"], context) == secrets[1]
                tags = metadata(meta["with"], context)
                context["steps"].docker_meta = SimpleNamespace(
                    outputs=SimpleNamespace(tags="\n".join(tags))
                )
                output, logs = run_step(intent, context)
                assert not output, "intent step must not export credentials or job outputs"
                assert all(tag in logs for tag in tags), "intent must show intended tags"
        assert tags == [f"offline-owner/av1an:{tag}" for tag in wanted], name
        reachable = event == "workflow_dispatch" or event == "pull_request" or any(
            fnmatch.fnmatch(ref.removeprefix(prefix), pattern)
            for prefix, patterns in (
                ("refs/heads/", triggers["push"]["branches"]),
                ("refs/tags/", triggers["push"].get("tags", [])),
            ) if ref.startswith(prefix) for pattern in patterns
        )
        assert reachable or name in {"feature-push", "other-tag"}, name
        print(f"{name}: build={'push:false' if reachable else 'not triggered'}; "
              f"publish={','.join(tags) if tags else 'skipped'}")
    print(f"PASS: {len(cases)} offline cases; no Docker/actions/network/real credentials")


if __name__ == "__main__":
    main()
