#!/usr/bin/env python3
"""Focused process-observer race tests using only Python's standard library."""

from __future__ import annotations

import importlib.util
import unittest
from pathlib import Path
from unittest import mock


SCRIPT = Path(__file__).with_name("measure-process.py").resolve()
SPEC = importlib.util.spec_from_file_location("lumin_measure_process", SCRIPT)
if SPEC is None or SPEC.loader is None:
    raise RuntimeError(f"cannot load benchmark process observer from {SCRIPT}")
MEASURE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MEASURE)


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


if __name__ == "__main__":
    unittest.main(verbosity=2)
