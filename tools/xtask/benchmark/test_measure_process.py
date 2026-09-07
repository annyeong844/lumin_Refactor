#!/usr/bin/env python3
"""Focused process-observer race tests using only Python's standard library."""

from __future__ import annotations

import importlib.util
import argparse
import copy
import ctypes
import io
import json
import os
import socket
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock


SCRIPT = Path(__file__).with_name("measure-process.py").resolve()
sys.dont_write_bytecode = True
SPEC = importlib.util.spec_from_file_location("lumin_measure_process", SCRIPT)
if SPEC is None or SPEC.loader is None:
    raise RuntimeError(f"cannot load benchmark process observer from {SCRIPT}")
MEASURE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MEASURE)


def wait_helper(helper):
    try:
        return helper.communicate(timeout=30)
    except subprocess.TimeoutExpired:
        # Popen.__exit__ waits without a deadline. Do not enter that context on
        # the watchdog path; terminate only our held helper and fail the test.
        helper.kill()
        helper.communicate(timeout=30)
        raise


class LinuxProcessTreeTests(unittest.TestCase):
    def test_process_exit_between_enumeration_and_stat_read_is_ignored(self) -> None:
        entry = Path("/proc/123")
        with (
            mock.patch.object(Path, "iterdir", return_value=iter([entry])),
            mock.patch.object(
                Path,
                "read_text",
                side_effect=ProcessLookupError(3, "No such process"),
            ),
        ):
            self.assertEqual(MEASURE.linux_process_tree(), {})

    def test_unrelated_proc_read_failures_remain_visible(self) -> None:
        entry = Path("/proc/123")
        with (
            mock.patch.object(Path, "iterdir", return_value=iter([entry])),
            mock.patch.object(Path, "read_text", side_effect=OSError(5, "I/O error")),
        ):
            with self.assertRaises(OSError):
                MEASURE.linux_process_tree()


class FakeApi:
    """Authored job state, with no access to the global process namespace."""

    def __init__(self, fail=None, total=2):
        self.fail = fail
        self.total = total
        self.calls = []
        self.counts = {}

    def call(self, name, value=None):
        self.calls.append(name)
        self.counts[name] = self.counts.get(name, 0) + 1
        if self.fail == (name, self.counts[name]):
            raise OSError(5, f"injected {name} failure")
        return value

    def create(self): return self.call("create", 99)
    def GetCurrentProcess(self): return -1
    def assign(self, job, process):
        assert (job, process) == (99, -1)
        self.call("assign")
    def member(self, job, process):
        assert job == 99 and process in (-1, 24)
        return self.call("member", True)
    def times(self, process):
        return self.call("times", (100, 0) if process == -1 else (200, 300))
    def flags(self, job): return self.call("flags", 0)
    def accounting(self, job):
        total = 1 if self.counts.get("accounting", 0) == 0 else self.total
        return self.call("accounting", {"totalProcesses": total, "activeProcesses": 1,
                                        "totalTerminatedProcesses": 0})
    def peak(self, process): return self.call("peak", 4096)
    def close(self, job):
        assert job == 99
        self.call("close")
    def terminate(self, job):
        assert job == 99  # Never a PID or an outside job.
        self.call("terminate")


class WindowsJobModelTests(unittest.TestCase):
    def process(self):
        return argparse.Namespace(_handle=24, pid=os.getpid() + 1)

    def test_watchdog_kills_owned_helper_and_reraises_without_unbounded_wait(self):
        helper = mock.Mock()
        helper.communicate.side_effect = [subprocess.TimeoutExpired("fixture", 30), (b"", b"")]
        with self.assertRaises(subprocess.TimeoutExpired):
            wait_helper(helper)
        self.assertEqual(helper.mock_calls, [mock.call.communicate(timeout=30), mock.call.kill(),
                                            mock.call.communicate(timeout=30)])

    def test_every_api_failure_rejects_without_foreign_cleanup(self):
        for fail in [("create", 1), ("assign", 1), ("member", 1), ("member", 2),
                     ("times", 1), ("times", 2), ("times", 3), ("flags", 1), ("flags", 2),
                     ("accounting", 1), ("accounting", 2), ("peak", 1), ("close", 1)]:
            with self.subTest(fail=fail), tempfile.TemporaryDirectory() as root:
                api = FakeApi(fail)
                job = MEASURE.WindowsJob(api)
                with self.assertRaises(OSError) as error:
                    job.admit()
                    job.bind(self.process())
                    api.peak(24)
                    job.finish(self.process(), Path(root) / "measurement.json")
                    job.close()
                confirmed = job.confirmed
                with mock.patch.object(sys, "stderr", io.StringIO()):
                    job.abort(error.exception)
                self.assertEqual("terminate" in api.calls, confirmed)
                self.assertFalse((Path(root) / "measurement.json").exists())

    def test_child_total_is_rejected_and_raw_complete_receipt_is_retained(self):
        for total in (3, 4):
            with self.subTest(total=total), tempfile.TemporaryDirectory() as root:
                job = MEASURE.WindowsJob(FakeApi(total=total))
                job.admit()
                job.bind(self.process())
                with self.assertRaisesRegex(RuntimeError, "analysis child processes"):
                    job.finish(self.process(), Path(root) / "measurement.json")
                raw = (Path(root) / "windows-process-observation.json").read_bytes()
                self.assertEqual(json.loads(raw)["after"]["totalProcesses"], total)
                self.assertEqual(raw, MEASURE.json_bytes(json.loads(raw)))
                job.close()

    def test_counter_lifetime_flag_and_membership_contradictions_fail(self):
        with tempfile.TemporaryDirectory() as root:
            job = MEASURE.WindowsJob(FakeApi())
            job.admit()
            job.bind(self.process())
            job.finish(self.process(), Path(root) / "measurement.json")
            valid = job.record
            for key, value in [("processId", 0), ("helperInJob", False), ("processInJob", False),
                               ("helperCreationTime100ns", 201), ("processExitTime100ns", 199),
                               ("limitFlagsBefore", 1), ("limitFlagsAfter", 1)]:
                altered = copy.deepcopy(valid)
                altered[key] = value
                with self.subTest(key=key), self.assertRaises(RuntimeError):
                    MEASURE.validate_windows_observation(altered)
            for section, field, value in [("before", "totalProcesses", 0), ("before", "activeProcesses", 2),
                                           ("after", "totalProcesses", 1), ("after", "activeProcesses", 0),
                                           ("after", "activeProcesses", 3), ("after", "totalTerminatedProcesses", 1)]:
                altered = copy.deepcopy(valid)
                altered[section][field] = value
                with self.subTest(field=field), self.assertRaises(RuntimeError):
                    MEASURE.validate_windows_observation(altered)
            valid["after"]["activeProcesses"] = 2  # Held exited-process references are legal.
            MEASURE.validate_windows_observation(valid)

    def test_partial_receipt_write_and_cleanup_failure_remain_errors(self):
        job = MEASURE.WindowsJob(FakeApi(("terminate", 1)))
        job.admit()
        job.bind(self.process())
        with mock.patch.object(MEASURE, "write_new", side_effect=OSError(28, "disk full")):
            with self.assertRaises(OSError) as error:
                job.finish(self.process(), Path("unused-measurement.json"))
        diagnostic = io.StringIO()
        with mock.patch.object(sys, "stderr", diagnostic):
            job.abort(error.exception)
        self.assertIn("cleanup requested", diagnostic.getvalue())
        self.assertIn("cleanup unknown", diagnostic.getvalue())
        self.assertNotIn("cleanup complete", diagnostic.getvalue())

    def test_thread_and_final_rss_failure_cannot_borrow_an_earlier_peak(self):
        # A synchronous thread double fixes poll-before-wait ordering without sleeps.
        class Thread:
            ident = None
            def __init__(self, target, **kwargs): self.target = target
            def start(self):
                self.ident = 1
                self.target()
            def join(self): pass

        for fail_at in (1, 2):
            with self.subTest(fail_at=fail_at), tempfile.TemporaryDirectory() as root:
                args = argparse.Namespace(cwd=Path(root), stdout=Path(root) / "stdout",
                                          stderr=Path(root) / "stderr", output=Path(root) / "measurement.json",
                                          subcommand="measure")
                process = self.process()
                process.wait = lambda: 0
                api = FakeApi(("peak", fail_at))
                job = MEASURE.WindowsJob(api)
                job.admit()
                # Stop after the first successful poll; final query then fails at 2.
                event = mock.Mock()
                event.is_set.side_effect = [False, True]
                with (mock.patch.object(MEASURE.subprocess, "Popen", return_value=process),
                      mock.patch.object(MEASURE.threading, "Thread", Thread),
                      mock.patch.object(MEASURE.threading, "Event", return_value=event)):
                    with self.assertRaises((OSError, RuntimeError)):
                        MEASURE.measure_in_job(args, ["product"], job)
                self.assertFalse(args.output.exists())


@unittest.skipUnless(os.name == "nt", "native Windows job tests run in the required Windows lane")
class WindowsNativeJobTests(unittest.TestCase):
    def setUp(self):
        self.root = tempfile.TemporaryDirectory()
        self.addCleanup(self.root.cleanup)
        self.path = Path(self.root.name)

    def command(self, mode="clean", diagnostic=False, wrapper=None, extra=()):
        fixture = SCRIPT.with_name("observer_fixture.py")
        helper = [str(fixture), wrapper] if wrapper else [str(SCRIPT)]
        return [sys.executable, "-I", "-S", *helper,
                "measure-audit-diagnostic" if diagnostic else "measure",
                "--cwd", str(self.path), "--output", str(self.path / "measurement.json"),
                "--stdout", str(self.path / "stdout"), "--stderr", str(self.path / "stderr"),
                "--", sys.executable, "-I", "-S", str(fixture), "product", mode, *extra]

    def test_fast_exit_both_envelopes_and_nested_host_job(self):
        # Each subtest has a fresh isolated helper and create-new capture directory.
        for diagnostic, wrapper in [(False, None), (True, None), (True, "nested-helper"),
                                    (False, "poisoned-tree-helper")]:
            with self.subTest(diagnostic=diagnostic, wrapper=wrapper), tempfile.TemporaryDirectory() as root:
                self.path = Path(root)
                helper = subprocess.Popen(self.command(diagnostic=diagnostic, wrapper=wrapper),
                                          stdout=subprocess.PIPE, stderr=subprocess.PIPE)
                output, errors = wait_helper(helper)
                self.assertEqual((helper.returncode, output, errors), (0, b"", b""))
                record = json.loads((self.path / "windows-process-observation.json").read_bytes())
                self.assertEqual(record["helperProcessId"], helper.pid)
                MEASURE.validate_windows_observation(record)
                measurement = json.loads((self.path / "measurement.json").read_bytes())
                expected_fields = {"schemaVersion", "analysisChildPids", "elapsedNanoseconds", "exitCode",
                                   "observerResolutionNanoseconds", "peakRssBytes", "rssSource"}
                if diagnostic:
                    expected_fields.add("processId")
                    self.assertEqual(measurement["processId"], record["processId"])
                self.assertEqual(set(measurement), expected_fields)
                self.assertEqual(measurement["schemaVersion"], f"lumin.phase1-process-measurement.v{2 if diagnostic else 1}")
                self.assertEqual(measurement["analysisChildPids"], [])
                for name in ("measurement.json", "windows-process-observation.json"):
                    raw = (self.path / name).read_bytes()
                    self.assertEqual(raw, MEASURE.json_bytes(json.loads(raw)))
                self.assertEqual((self.path / "stdout").read_bytes(), b"{}\n")
                self.assertEqual((self.path / "stderr").read_bytes(), b"")

    def test_all_terminated_children_and_grandchildren_are_counted(self):
        for mode, total in [("child", 3), ("grandchild", 4), ("terminated-child", 3)]:
            with self.subTest(mode=mode), tempfile.TemporaryDirectory() as root:
                self.path = Path(root)
                helper = subprocess.run(self.command(mode), capture_output=True, timeout=30)
                self.assertNotEqual(helper.returncode, 0)
                self.assertIn(f"job total {total}".encode(), helper.stderr)
                self.assertIn(b"cleanup requested", helper.stderr)
                self.assertFalse((self.path / "measurement.json").exists())
                record = json.loads((self.path / "windows-process-observation.json").read_bytes())
                self.assertEqual(record["after"]["totalProcesses"], total)
                self.assertEqual((self.path / "stdout").read_bytes(), b"{}\n")

    def test_failure_cleanup_terminates_held_descendant_not_outside_process(self):
        kernel = ctypes.WinDLL("kernel32", use_last_error=True)
        kernel.OpenProcess.argtypes = [ctypes.c_uint32, ctypes.c_int32, ctypes.c_uint32]
        kernel.OpenProcess.restype = ctypes.c_void_p
        kernel.WaitForSingleObject.argtypes = [ctypes.c_void_p, ctypes.c_uint32]
        kernel.WaitForSingleObject.restype = ctypes.c_uint32
        api = MEASURE.WindowsApi()
        outsider = subprocess.Popen([sys.executable, "-I", "-S", "-c", "import sys; sys.stdin.buffer.read(1)"],
                                    stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        self.addCleanup(lambda: outsider.communicate(b"X", timeout=30))
        with socket.socket() as listener:
            listener.bind(("127.0.0.1", 0))
            listener.listen()
            listener.settimeout(30)
            helper = subprocess.Popen(self.command("lingering", extra=(str(listener.getsockname()[1]),)),
                                      stdout=subprocess.PIPE, stderr=subprocess.PIPE)
            peers = {}
            try:
                for _ in range(2):
                    peer, _ = listener.accept()
                    peer.settimeout(30)
                    line = b""
                    while not line.endswith(b"\n"):
                        chunk = peer.recv(1)
                        self.assertTrue(chunk, "fixture closed before barrier")
                        line += chunk
                    role, pid = line.decode().split()
                    self.assertNotIn(role, peers)
                    peers[role] = (peer, int(pid))
                self.assertEqual(set(peers), {"root", "child"})
                child = kernel.OpenProcess(0x00100000 | 0x1000, False, peers["child"][1])
                api.checked(child, "OpenProcess fixture child")
                try:
                    self.assertEqual(kernel.WaitForSingleObject(child, 0), 258)  # Still at barrier.
                    peers["root"][0].sendall(b"R")
                    _, errors = helper.communicate(timeout=30)
                    self.assertNotEqual(helper.returncode, 0)
                    self.assertIn(b"job total 3", errors)
                    self.assertEqual(kernel.WaitForSingleObject(child, 30_000), 0)
                    self.assertIsNone(outsider.poll())
                finally:
                    api.close(child)
            finally:
                for peer, _ in peers.values():
                    peer.close()
                if helper.poll() is None:
                    helper.kill()  # Watchdog/test failure: exact owned helper handle only.
                    helper.communicate(timeout=30)


if __name__ == "__main__":
    unittest.main(verbosity=2)
