"""Run the cfg(test)-only TUI child on a real controlling Unix terminal."""

import ctypes
import errno
import fcntl
import os
import select
import signal
import struct
import subprocess
import sys
import tempfile
import termios
import time
from pathlib import Path


OWNERSHIP_CASES = {
    "raw": ("terminal_raw_enable_failure_does_not_clean_unacquired_screen_in_controlled_pty", ("raw",)),
    "initialization": ("terminal_entry_or_construction_failure_cleans_acquired_resources_once_in_controlled_pty", ("enter_write", "enter_flush", "construct")),
    "restore": ("terminal_restore_error_retains_typed_cause_and_cancels_in_controlled_pty", ("resize", "final_draw", "raw_cleanup", "restore_flush")),
    "cleanup": ("terminal_cleanup_error_attempts_remaining_resources_once_and_keeps_first_cause_in_controlled_pty", ("leave_cleanup", "show_cleanup", "flush_cleanup", "broken")),
    "normal": ("terminal_successful_restore_and_drop_are_idempotent_and_restore_prior_hook_in_controlled_pty", ("normal",)),
    "inactive": ("inactive_terminal_has_no_raw_or_screen_effects_in_controlled_pty", ("inactive",)),
    "unwind": ("terminal_unwind_cancels_releases_resources_and_restores_prior_hook_in_controlled_pty", ("unwind",)),
}


LOGGING_CASES = {
    "normal": "console_stage_restore_resumes_diagnostics_with_live_or_deferred_selected_stream_routing",
    "overflow": "console_backlog_overflow_is_bounded_and_utf8_safe_while_file_log_stays_complete",
    "raw": "console_raw_mode_failure_cancels_and_flushes_file_diagnostics",
    "construct": "console_terminal_construction_failure_cancels_and_flushes_file_diagnostics",
    "draw": "console_draw_failure_restores_terminal_before_final_diagnostics_and_flushes_file",
    "restore": "console_restore_failure_keeps_error_and_flushes_file_after_terminal_cleanup",
    "panic": "console_owner_unwind_restores_terminal_before_prior_hook_and_flushes_file",
    "startup": "console_startup_panic_retires_owner_before_raw_mode_and_flushes_diagnostics",
    "headless": "headless_logging_emits_live_diagnostics_without_terminal_controls",
    "testmode": "console_test_mode_logging_emits_live_diagnostics_without_terminal_controls",
}


PANIC_CASES = {
    "ui": "tui_render_panic_restores_terminal_reports_once_and_reaps_owned_tree",
    "worker": "tui_worker_panic_fails_application_after_terminal_restore_and_reaps_owned_tree",
    "progress": "tui_progress_mapper_panic_fails_application_and_reaps_owned_tree",
    "main": "entry_panic_unwinds_library_boundary_and_reaps_owned_tree_after_terminal_restore",
    "subsequent": "repeated_panics_restore_terminal_and_call_prior_hook_once_each",
    "startup": "startup_panic_retires_before_raw_and_writerless_cleanup_releases_raw_once",
    "cli-ui": "cli_ui_panic_returns_failure_without_repeating_reported_payload",
    "cli-worker": "cli_worker_panic_preserves_typed_library_cause_without_duplicate_diagnostic",
    "cli-mixed": "cli_stage_error_and_display_panic_preserve_primary_io_cause",
    "cli-error": "cli_plain_stage_error_keeps_context_without_panic_diagnostic",
    "cli-aggregate": "cli_aggregate_panic_preserves_codec_output_and_nonpanic_failure",
    "cli-unreported": "cli_unreported_panic_prints_original_payload_instead_of_diagnostic_above",
}


def check_stream(
    binary: str, stream: str, mode: str = "normal", root: str = "",
    cleanup_errors: list[str] | None = None,
) -> None:
    errors = cleanup_errors if cleanup_errors is not None else []
    ownership_group = mode.removeprefix("matrix-") if mode.startswith("matrix-") else None
    if ownership_group:
        ownership_test, ownership_cases = OWNERSHIP_CASES[ownership_group]
        mode = "matrix"
    master, slave = os.openpty()
    try:
        fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 6, 80, 0, 0))
        original_mode = termios.tcgetattr(slave)

        def controlling_terminal() -> None:
            os.setsid()
            fcntl.ioctl(slave, termios.TIOCSCTTY, 0)
            assert os.tcgetpgrp(slave) == os.getpgrp()

        env = os.environ.copy()
        env.pop("CONDOR_TEST_MODE", None)
        env["TUI_PTY_STREAM"] = stream
        env["TERM"] = "xterm-256color"
        if ownership_group:
            env["TUI_OWNERSHIP_TEST"] = f"apps::tests::internal_integration::{ownership_test}"
        if mode in ("normal", "render-failure"):
            env["TUI_SELECTED_TEST"] = "apps::tests::internal_integration::" + {
                "normal": "tui_selected_pty_restore_renders_final_frame_and_keeps_redirected_stream_clean_in_controlled_pty",
                "render-failure": "tui_render_write_failure_restores_terminal_and_returns_broken_pipe_with_cancellation_in_controlled_pty",
            }[mode]
        panic_case = mode.removeprefix("panic-") if mode.startswith("panic-") else None
        logging_case = mode.removeprefix("logging-") if mode.startswith("logging-") else None
        if logging_case:
            env["TUI_LOGGING_CASE"] = logging_case
            env["TUI_LOGGING_ROOT"] = root
            env["TUI_LOGGING_TEST"] = f"apps::tests::internal_integration::{LOGGING_CASES[logging_case]}"
            env.pop("RUST_LOG", None)
            if logging_case == "testmode":
                env["CONDOR_TEST_MODE"] = "1"
        if panic_case:
            env["TUI_PANIC_CASE"] = panic_case
            env["TUI_PANIC_ROOT"] = root
            env["TUI_PANIC_TEST"] = f"apps::tests::internal_integration::{PANIC_CASES[panic_case]}"
        with (
            tempfile.TemporaryFile() as redirected,
            tempfile.TemporaryFile() as headless_stderr,
        ):
            child = subprocess.Popen(
                [
                    binary,
                    "--exact",
                    env["TUI_PANIC_TEST"] if panic_case
                    else env["TUI_LOGGING_TEST"] if logging_case
                    else {
                        "normal": env.get("TUI_SELECTED_TEST", ""),
                        "render-failure": env.get("TUI_SELECTED_TEST", ""),
                        "matrix": env.get("TUI_OWNERSHIP_TEST", ""),
                    }[mode],
                    "--include-ignored",
                    "--nocapture",
                    "--quiet",
                ],
                stdin=slave,
                stdout=slave if stream == "stdout" else redirected,
                stderr=(
                    slave if stream == "stderr"
                    else headless_stderr if stream == "headless" else redirected
                ),
                env=env,
                preexec_fn=controlling_terminal,
            )
            terminal_output = bytearray()
            answered_queries = 0
            released = set()
            deadline = time.monotonic() + 30
            try:
                while child.poll() is None:
                    if time.monotonic() >= deadline:
                        raise AssertionError(f"{stream}: TUI child exceeded 30s watchdog")
                    if select.select([master], [], [], 0.05)[0]:
                        terminal_output.extend(os.read(master, 65536))
                    # Answer even a misrouted query so RED detects stream pollution,
                    # rather than merely timing out in crossterm's cursor reader.
                    redirected_output = os.pread(redirected.fileno(), 1048576, 0)
                    queries = terminal_output.count(b"\x1b[6n")
                    queries += redirected_output.count(b"\x1b[6n")
                    while answered_queries < queries:
                        os.write(master, b"\x1b[1;1R")
                        answered_queries += 1
                    if logging_case:
                        for stage in (1, 2):
                            if stage in released or not Path(root, f"produced-{stage}").exists():
                                continue
                            # A live console must deliver before producer release.
                            # PTY delivery may lag write(): leave it blocked and
                            # retry under the existing watchdog until all arrive.
                            while select.select([master], [], [], 0)[0]:
                                terminal_output.extend(os.read(master, 65536))
                            diagnostic_file = headless_stderr if stream == "headless" else redirected
                            diagnostic = (
                                bytes(terminal_output) if stream == "stderr"
                                else os.pread(diagnostic_file.fileno(), 1048576, 0)
                            )
                            live = stream in ("stdout", "headless") or logging_case == "testmode"
                            markers = [
                                b"TUI_LOGGING-" + event + f"-{stage}".encode()
                                for event in (b"scene-model", b"metric-fallback", b"retry", b"encoder-failure")
                            ]
                            if live:
                                if not all(marker in diagnostic for marker in markers):
                                    continue
                                if logging_case == "overflow" and not all(
                                    diagnostic.count((f"TUI_LOGGING-overflow-{index:02}-" + "🦜" * 3000).encode()) >= stage
                                    for index in range(20)
                                ):
                                    continue
                                assert b"deferred console diagnostics" not in diagnostic, diagnostic
                            else:
                                assert not any(marker in diagnostic for marker in markers), ("active console routing", markers, diagnostic)
                            Path(root, f"release-{stage}").touch()
                            released.add(stage)
                # Close our slave after child exit so EOF, not a zero-time poll,
                # marks the end of all terminal output.
                restored_mode = termios.tcgetattr(slave)
                os.close(slave)
                slave = -1
                while True:
                    try:
                        chunk = os.read(master, 65536)
                    except OSError as error:
                        if error.errno != errno.EIO:
                            raise
                        break
                    if not chunk:
                        break
                    terminal_output.extend(chunk)
            finally:
                try:
                    if child.poll() is None:
                        child.kill()  # Popen still owns this unreaped child, pinning its PID.
                except OSError as error:
                    errors.append(f"child kill: {error}")
                try:
                    child.wait(timeout=5)
                except (OSError, subprocess.TimeoutExpired) as error:
                    errors.append(f"child reap: {error}")

            redirected.seek(0)
            redirected_output = redirected.read()
            terminal_output = bytes(terminal_output)
            evidence = (stream, child.returncode, terminal_output, redirected_output)
            if logging_case:
                stages = 2 if logging_case in ("normal", "overflow", "headless", "testmode") else 1
                assert released == set(range(1, stages + 1)), ("console delivery handshake incomplete", released, evidence)
                assert child.returncode == 0, evidence
                assert restored_mode == original_mode, evidence
                headless_stderr.seek(0)
                diagnostic = (
                    headless_stderr.read() if stream == "headless"
                    else terminal_output if stream == "stderr" else redirected_output
                )
                data = terminal_output if stream == "stdout" else redirected_output
                assert b"1 passed; 0 failed" in data, evidence
                assert b"PTY_LOGGING_ASSERTIONS_COMPLETED" in data, evidence
                sentinel = b"TUI_LOGGING_BEGIN\nsource-output-sentinel\nTUI_LOGGING_END\n"
                assert data.replace(b"\r\n", b"\n").split(b"TUI_LOGGING_BEGIN\n", 1)[1].split(b"TUI_LOGGING_END\n", 1)[0] == b"source-output-sentinel\n", evidence
                assert sentinel in data.replace(b"\r\n", b"\n"), evidence
                assert b"\x1b" not in redirected_output, evidence
                log = Path(root, "events.log").read_text()
                assert "\x1b" not in log and "TUI_LOGGING-final-flush-marker" in log, log
                live = stream in ("stdout", "headless") or logging_case == "testmode"
                if live:
                    assert b"deferred console diagnostics" not in diagnostic, evidence
                for stage in range(1, stages + 1):
                    for event in ("scene-model", "metric-fallback", "retry", "encoder-failure"):
                        marker = f"TUI_LOGGING-{event}-{stage}"
                        assert log.count(marker) == 1, (marker, log)
                        assert diagnostic.count(marker.encode()) == (int(live) if event == "scene-model" else 1), evidence
                    if stages == 2:
                        for marker in (f"TUI_LOGGING-between-stages-{stage}", f"TUI_LOGGING-returned-stage-warning-{stage}"):
                            assert diagnostic.count(marker.encode()) == 1, evidence
                if stream != "headless" and logging_case != "testmode":
                    expected_screens = 0 if logging_case in ("raw", "startup") else stages
                    assert terminal_output.count(b"\x1b[?1049h") == expected_screens, evidence
                    assert terminal_output.count(b"\x1b[?1049l") == expected_screens, evidence
                    assert terminal_output.count(b"\x1b[?25h") == expected_screens, evidence
                    if stream == "stderr":
                        for part in terminal_output.split(b"\x1b[?1049h")[1:]:
                            assert b"TUI_LOGGING-" not in part.split(b"\x1b[?25h", 1)[0], evidence
                elif stream == "headless":
                    assert b"\x1b" not in terminal_output + diagnostic, evidence
                else:
                    assert b"\x1b[?1049h" not in terminal_output, evidence
                    assert b"\x1b[?25h" not in terminal_output, evidence
                diagnostic.decode("utf-8")  # bounded prefixes cannot split a code point
                if logging_case in ("construct", "draw", "restore", "panic"):
                    assert diagnostic.count(b"TUI_LOGGING-final-control-warning") == 1, evidence
                    assert log.count("TUI_LOGGING-final-control-warning") == 1, log
                    if stream == "stderr":
                        assert diagnostic.index(b"\x1b[?25h") < diagnostic.index(b"TUI_LOGGING-final-control-warning"), evidence
                if logging_case == "overflow":
                    if live:
                        assert diagnostic.count(b"TUI_LOGGING-overflow-") == 40, evidence
                    else:
                        assert diagnostic.count(b"15 dropped, 5 truncated") == 2, evidence
                        assert diagnostic.count(b"TUI_LOGGING-overflow-") == 10, evidence
                        for line in diagnostic.splitlines():
                            if b"TUI_LOGGING-overflow-" in line:
                                assert len(line) <= 1050, line
                    for index in range(20):
                        message = f"TUI_LOGGING-overflow-{index:02}-" + "🦜" * 3000
                        assert log.count(message) == 2, index
                        if live:
                            assert diagnostic.count(message.encode()) == 2, index
                if logging_case in ("panic", "startup"):
                    assert diagnostic.count(b"TUI_LOGGING-prior-hook") == 1, evidence
                routing = "live/full diagnostics" if live else "paused/bounded diagnostics"
                print(f"{stream}/{logging_case}: {routing}, delivery handshake, file shutdown flush, owner controls passed")
                print(f"{stream}/{logging_case}: selected logging assertions completed")
                return
            if panic_case:
                headless_stderr.seek(0)
                diagnostic = (
                    headless_stderr.read() if stream == "headless"
                    else terminal_output if stream == "stderr" else redirected_output
                )
                data = terminal_output if stream == "stdout" else redirected_output
                assert child.returncode != 0, evidence
                cli = panic_case.startswith("cli-")
                assert b"running 1 test" in data and b"0 passed; 1 failed" in data, evidence
                assert b"TUI_PANIC_ASSERTIONS_COMPLETED" in data, evidence
                payload_case = (
                    "main" if panic_case == "subsequent"
                    else "ui" if panic_case in ("cli-ui", "cli-mixed")
                    else "worker" if panic_case == "cli-aggregate"
                    else panic_case.removeprefix("cli-")
                )
                payload = f"TUI_PANIC-{payload_case}-payload".encode()
                if panic_case == "cli-error":
                    assert b"TUI_PANIC primary context" in diagnostic, evidence
                    assert b"TUI_PANIC-nonpanic-error" in diagnostic, evidence
                    assert b"panicked" not in diagnostic, evidence
                else:
                    assert diagnostic.count(payload) == 1, (payload, diagnostic, evidence)
                if panic_case == "cli-unreported":
                    assert b"diagnostic above" not in diagnostic, evidence
                assert payload not in data, evidence
                sentinel = b"TUI_PANIC_BEGIN\nsource-output-sentinel\nTUI_PANIC_END\n"
                assert sentinel in data.replace(b"\r\n", b"\n"), evidence
                assert restored_mode == original_mode, evidence
                assert Path(root, "unwound").read_bytes() == b"owned child reaped", evidence
                if cli:
                    assert Path(root, "library-error").read_bytes() == b"original library error retained"
                    if panic_case == "cli-mixed":
                        assert b"TUI_PANIC primary context" in diagnostic, evidence
                        assert b"TUI_PANIC-nonpanic-error" in diagnostic, evidence
                    if panic_case == "cli-aggregate":
                        for preserved in (b"TUI_PANIC-nonpanic-error", b"codec-stdout-sentinel", b"codec-stderr-sentinel"):
                            assert preserved in diagnostic, evidence
                if panic_case == "startup":
                    assert b"TUI_PANIC_STARTUP_RETIRED_RAW_OFF" in data, evidence
                    assert b"\x1b" not in terminal_output, evidence
                elif stream == "headless":
                    assert b"\x1b" not in diagnostic + data + terminal_output, evidence
                else:
                    shows = terminal_output.count(b"\x1b[?25h")
                    assert shows == (2 if panic_case == "subsequent" else 1), evidence
                    assert terminal_output.count(b"\x1b[?1049l") == shows, evidence
                    for segment in terminal_output.split(b"\x1b[?25h")[1:]:
                        assert b"\x1b" not in segment.split(b"\x1b[?1049h")[0], evidence
                    if stream == "stderr" and panic_case not in ("cli-error", "cli-unreported"):
                        leave = terminal_output.index(b"\x1b[?1049l")
                        show = terminal_output.index(b"\x1b[?25h")
                        assert leave < show < diagnostic.index(payload), evidence
                    assert b"\x1b" not in redirected_output, evidence
                if panic_case == "subsequent":
                    assert diagnostic.count(b"TUI_PANIC_PRIOR:") == 3, evidence
                    assert diagnostic.count(b"TUI_PANIC-second-payload") == 1, evidence
                    assert diagnostic.count(b"TUI_PANIC-third-payload") == 1, evidence
                    assert b"TUI_PANIC_PRIOR_COUNT=3" in diagnostic, evidence
                print(
                    f"{stream}/{panic_case}: diagnostic once, failed outcome, "
                    "owned unwind, terminal retired"
                )
                print(f"{stream}/{panic_case}: selected panic assertions completed")
                return
            assert child.returncode == 0, evidence
            harness_output = (
                terminal_output if stream == "stdout" else redirected_output
            )
            assert b"running 1 test" in harness_output, evidence
            assert b"1 passed; 0 failed" in harness_output, evidence
            assert b"\x1b" not in redirected_output, evidence
            if mode == "matrix":
                assert ownership_group, "select a behavior-named ownership group"
                assert b"TERMINAL_OWNERSHIP_SELECTED_CASES_PASSED" in harness_output, evidence
                assert harness_output.count(b"TERMINAL_OWNERSHIP_CASE_") == len(ownership_cases), evidence
                for case in ownership_cases:
                    assert harness_output.count(f"TERMINAL_OWNERSHIP_CASE_{case}_PASSED".encode()) == 1, evidence
                assert restored_mode == original_mode, evidence
                print(harness_output.decode())
                print(f"{stream}/{ownership_group}: ownership cases passed; termios restored")
                return
            assert b"SELECTED_TERMINAL_ASSERTIONS_COMPLETED" in harness_output, evidence
            if mode == "render-failure":
                assert b"TERMINAL_OWNERSHIP_ERROR_RETURNED_CANCEL_TRUE" in harness_output, evidence
            elif stream == "stderr":
                payload = redirected_output.split(b"SELECTED_STREAM_BEGIN\n", 1)[1]
                assert payload.split(b"SELECTED_STREAM_END\n", 1)[0] == b"payload\n", evidence
            enter = terminal_output.index(b"\x1b[?1049h")
            leave = terminal_output.index(b"\x1b[?1049l")
            show = terminal_output.index(b"\x1b[?25h", leave)
            if mode == "render-failure":
                assert enter < leave < show, evidence
                for command in (b"\x1b[?1049h", b"\x1b[?1049l", b"\x1b[?25h"):
                    assert terminal_output.count(command) == 1, evidence
                assert b"\x1b[?25l" not in terminal_output[show:], evidence
            else:
                render = terminal_output.index(b"SELECTED_STREAM-final-render", leave)
                assert enter < leave < render < show, evidence
                assert terminal_output[leave + 8 :].startswith(b"\r\n" * 6), evidence
            assert restored_mode == original_mode, evidence
            print("SELECTED_TERMINAL_ASSERTIONS_COMPLETED")
            actions = (
                "error+cancel/enter/leave/show"
                if mode == "render-failure"
                else "enter/leave/final-render/show"
            )
            print(
                f"{stream}: 1 child passed; "
                f"{actions} on PTY; "
                f"redirected stream clean; {answered_queries} cursor queries; "
                "terminal mode restored"
            )
    finally:
        for fd in (master, slave):
            if fd >= 0:
                try:
                    os.close(fd)
                except OSError as error:
                    errors.append(f"PTY close: {error}")
        if cleanup_errors is None and errors:
            raise AssertionError("; ".join(errors))


def fixture_identity(pid: int) -> tuple[int, int]:
    fields = Path(f"/proc/{pid}/stat").read_text().rsplit(") ", 1)[1].split()
    return int(fields[19]), int(fields[2])  # start ticks and process group


def fixture_pidfd(pid: int, identity: tuple[int, int]) -> int:
    """Bind a kernel handle before checking the recorded lifetime; never kill by PID."""
    fd = os.pidfd_open(pid)
    try:
        if fixture_identity(pid) != identity:
            raise ValueError(f"fixture PID {pid} identity changed; refusing cleanup")
        return fd
    except BaseException:
        os.close(fd)
        raise


def cleanup_fixtures(records: list[str], errors: list[str]) -> None:
    for record in records:
        fd = None
        try:
            pid, start, group = map(int, record.split())
            try:
                fd = fixture_pidfd(pid, (start, group))
            except ProcessLookupError:
                continue  # An already-reaped lifetime needs no further cleanup.
            except FileNotFoundError:
                continue  # It exited between opening the handle and checking identity.
            if not select.select([fd], [], [], 5)[0]:
                errors.append(f"owned PID {pid} alive after child outcome")
                signal.pidfd_send_signal(fd, signal.SIGKILL)
            if not select.select([fd], [], [], 5)[0]:
                errors.append(f"fixture PID {pid} did not exit after kill")
            deadline = time.monotonic() + 5
            while True:
                try:
                    reaped = os.waitid(os.P_PIDFD, fd, os.WEXITED | os.WNOHANG)
                except ChildProcessError:
                    break  # OwnedProcess already reaped the leader.
                if reaped:
                    break
                if time.monotonic() >= deadline:
                    errors.append(f"fixture PID {pid} not reaped")
                    break
                time.sleep(0.01)
        except (OSError, ValueError) as error:
            errors.append(str(error))
        finally:
            if fd is not None:
                try:
                    os.close(fd)
                except OSError as error:
                    errors.append(f"fixture handle close: {error}")


def check_fixture_safety() -> None:
    # A stale first record must neither signal its PID nor skip the next cleanup.
    children = []
    try:
        for _ in range(2):
            children.append(subprocess.Popen(["sleep", "300"]))
        first, second = children
        start, group = fixture_identity(first.pid)
        next_start, next_group = fixture_identity(second.pid)
        errors = []
        cleanup_fixtures(
            [f"{first.pid} {start + 1} {group}", f"{second.pid} {next_start} {next_group}"],
            errors,
        )
        assert first.poll() is None, "stale identity signaled a live process"
        assert second.poll() is not None, "first error skipped later fixture cleanup"
        assert any("identity changed" in error for error in errors), errors
        assert any("alive after child outcome" in error for error in errors), errors
        print("fixture safety: stale identity refused; later owned cleanup attempted and reaped")
    finally:
        errors = []
        for child in children:
            try:
                if child.poll() is None:
                    child.kill()
                child.wait(timeout=5)
            except (OSError, subprocess.TimeoutExpired) as error:
                errors.append(str(error))
        if errors:
            raise AssertionError("; ".join(errors))


def check_panics(binary: str, selected_case: str) -> None:
    if selected_case not in PANIC_CASES:
        raise ValueError(f"unknown panic case {selected_case}")
    # Local to this test driver: adopt/reap fixture descendants even on RED.
    libc = ctypes.CDLL(None, use_errno=True)
    assert libc.prctl(36, 1, 0, 0, 0) == 0  # PR_SET_CHILD_SUBREAPER
    failures = []
    for stream in ("stderr", "stdout", "headless"):
        cases = (
            ("ui", "worker", "progress", "main", "subsequent", "startup", "cli-ui", "cli-worker", "cli-mixed", "cli-error", "cli-aggregate", "cli-unreported")
            if stream != "headless" else ("main", "progress", "cli-worker", "cli-error", "cli-aggregate", "cli-unreported")
        )
        for case in cases:
            if case != selected_case:
                continue
            with tempfile.TemporaryDirectory(prefix="panic-") as root:
                Path(root, "source").write_bytes(b"source-sentinel")
                Path(root, "output").write_bytes(b"output-sentinel")
                cleanup_errors = []
                try:
                    check_stream(binary, stream, f"panic-{case}", root, cleanup_errors)
                except (AssertionError, OSError, subprocess.TimeoutExpired) as error:
                    failures.append(f"{stream}/{case}: {error}")
                finally:
                    # Also clean real descendants after a RED timeout/failed assertion.
                    pids_path = Path(root, "pids")
                    try:
                        if pids_path.exists():
                            cleanup_fixtures(pids_path.read_text().splitlines(), cleanup_errors)
                    except OSError as error:
                        cleanup_errors.append(str(error))
                    for name in ("source", "output"):
                        try:
                            if Path(root, name).read_bytes() != f"{name}-sentinel".encode():
                                cleanup_errors.append(f"{name} sentinel changed")
                        except OSError as error:
                            cleanup_errors.append(str(error))
                    failures.extend(f"{stream}/{case}: {error}" for error in cleanup_errors)
    assert not failures, "\n".join(failures)


def check_logging(binary: str, selected_case: str) -> None:
    if selected_case not in LOGGING_CASES:
        raise ValueError(f"unknown logging case {selected_case}")
    for selected_stream in ("stderr", "stdout", "headless"):
        cases = ("headless",) if selected_stream == "headless" else (
            "normal", "overflow", "raw", "construct", "draw", "restore", "panic", "startup",
        )
        if selected_stream == "stderr":
            cases += ("testmode",)
        for case in cases:
            if case != selected_case:
                continue
            with tempfile.TemporaryDirectory(prefix="logging-") as root:
                check_stream(binary, selected_stream, f"logging-{case}", root)


if __name__ == "__main__" and len(sys.argv) > 2 and sys.argv[2] == "logging":
    check_logging(sys.argv[1], sys.argv[3])
elif __name__ == "__main__" and len(sys.argv) > 2 and sys.argv[2] == "panic":
    check_panics(sys.argv[1], sys.argv[3])
elif __name__ == "__main__" and len(sys.argv) > 2 and sys.argv[2] == "panic-fixture-cleanup":
    assert ctypes.CDLL(None, use_errno=True).prctl(36, 1, 0, 0, 0) == 0
    check_fixture_safety()
elif __name__ == "__main__":
    for selected_stream in ("stderr", "stdout"):
        check_stream(
            sys.argv[1], selected_stream, sys.argv[2] if len(sys.argv) > 2 else "normal"
        )
