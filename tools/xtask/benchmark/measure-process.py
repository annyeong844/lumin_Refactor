#!/usr/bin/env python3
"""Measure one direct packaged Lumin process with only Python's standard library."""

from __future__ import annotations

import argparse
import ctypes
import hashlib
import json
import os
import platform
import subprocess
import sys
import threading
import time
from pathlib import Path


SCHEMA = "lumin.phase1-process-measurement.v1"


def json_bytes(value: object) -> bytes:
    return (json.dumps(value, separators=(",", ":"), sort_keys=True) + "\n").encode("utf-8")


def write_new(path: Path, value: object) -> None:
    with path.open("xb") as stream:
        stream.write(json_bytes(value))
        stream.flush()
        os.fsync(stream.fileno())


def decode_mount_field(value: str) -> str:
    for encoded, decoded in (("\\040", " "), ("\\011", "\t"), ("\\012", "\n"), ("\\134", "\\")):
        value = value.replace(encoded, decoded)
    return value


def linux_mount(path: Path) -> tuple[str, str]:
    resolved = str(path.resolve(strict=True))
    winner = ("", "unknown")
    for line in Path("/proc/self/mountinfo").read_text(encoding="utf-8").splitlines():
        fields = line.split()
        separator = fields.index("-")
        mount_point = decode_mount_field(fields[4])
        filesystem = fields[separator + 1]
        if resolved == mount_point or resolved.startswith(mount_point.rstrip("/") + "/"):
            if len(mount_point) > len(winner[0]):
                winner = (mount_point, filesystem)
    if not winner[0]:
        raise RuntimeError(f"cannot identify filesystem mount for {resolved}")
    return winner


def windows_volume(path: Path) -> tuple[str, str]:
    kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)
    volume_path = ctypes.create_unicode_buffer(32768)
    if not kernel32.GetVolumePathNameW(str(path.resolve(strict=True)), volume_path, len(volume_path)):
        raise ctypes.WinError(ctypes.get_last_error())
    filesystem = ctypes.create_unicode_buffer(256)
    if not kernel32.GetVolumeInformationW(
        volume_path.value,
        None,
        0,
        None,
        None,
        None,
        filesystem,
        len(filesystem),
    ):
        raise ctypes.WinError(ctypes.get_last_error())
    return volume_path.value, filesystem.value.lower()


def total_memory_bytes() -> int:
    if os.name != "nt":
        return int(os.sysconf("SC_PAGE_SIZE")) * int(os.sysconf("SC_PHYS_PAGES"))

    class MemoryStatus(ctypes.Structure):
        _fields_ = [
            ("length", ctypes.c_ulong),
            ("memory_load", ctypes.c_ulong),
            ("total_physical", ctypes.c_ulonglong),
            ("available_physical", ctypes.c_ulonglong),
            ("total_page_file", ctypes.c_ulonglong),
            ("available_page_file", ctypes.c_ulonglong),
            ("total_virtual", ctypes.c_ulonglong),
            ("available_virtual", ctypes.c_ulonglong),
            ("available_extended_virtual", ctypes.c_ulonglong),
        ]

    status = MemoryStatus()
    status.length = ctypes.sizeof(status)
    if not ctypes.WinDLL("kernel32", use_last_error=True).GlobalMemoryStatusEx(ctypes.byref(status)):
        raise ctypes.WinError(ctypes.get_last_error())
    return int(status.total_physical)


def cpu_model() -> str:
    if os.name == "nt":
        return os.environ.get("PROCESSOR_IDENTIFIER", "unknown")
    cpuinfo = Path("/proc/cpuinfo")
    if cpuinfo.is_file():
        for line in cpuinfo.read_text(encoding="utf-8", errors="strict").splitlines():
            if line.startswith(("model name", "Hardware")) and ":" in line:
                return line.split(":", 1)[1].strip()
    return platform.processor() or "unknown"


def host_info(root: Path) -> dict[str, object]:
    if os.name == "nt":
        mount, filesystem = windows_volume(root)
    else:
        mount, filesystem = linux_mount(root)
    return {
        "schemaVersion": "lumin.phase1-benchmark-host.v1",
        "architecture": platform.machine(),
        "cpuModel": cpu_model(),
        "filesystemClass": filesystem,
        "filesystemMount": mount,
        "kernelRelease": platform.release(),
        "kernelVersion": platform.version(),
        "logicalProcessorCount": os.cpu_count(),
        "operatingSystem": platform.system().lower(),
        "platform": platform.platform(),
        "pythonExecutable": str(Path(sys.executable).resolve(strict=True)),
        "pythonVersion": platform.python_version(),
        "totalMemoryBytes": total_memory_bytes(),
        "wsl": os.name != "nt" and "microsoft" in (platform.release() + platform.version()).lower(),
    }


def linux_process_tree() -> dict[int, int]:
    tree: dict[int, int] = {}
    for entry in Path("/proc").iterdir():
        if not entry.name.isdecimal():
            continue
        try:
            stat = (entry / "stat").read_text(encoding="utf-8")
            tail = stat[stat.rfind(")") + 2 :].split()
            tree[int(entry.name)] = int(tail[1])
        except (FileNotFoundError, ProcessLookupError, PermissionError, IndexError, ValueError):
            continue
    return tree


def windows_process_tree() -> dict[int, int]:
    from ctypes import wintypes

    class ProcessEntry(ctypes.Structure):
        _fields_ = [
            ("size", wintypes.DWORD),
            ("usage", wintypes.DWORD),
            ("process_id", wintypes.DWORD),
            ("default_heap_id", ctypes.POINTER(ctypes.c_ulong)),
            ("module_id", wintypes.DWORD),
            ("threads", wintypes.DWORD),
            ("parent_process_id", wintypes.DWORD),
            ("priority_base", wintypes.LONG),
            ("flags", wintypes.DWORD),
            ("exe_file", wintypes.WCHAR * 260),
        ]

    kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)
    snapshot = kernel32.CreateToolhelp32Snapshot(0x00000002, 0)
    invalid = ctypes.c_void_p(-1).value
    if snapshot == invalid:
        raise ctypes.WinError(ctypes.get_last_error())
    try:
        entry = ProcessEntry()
        entry.size = ctypes.sizeof(entry)
        tree: dict[int, int] = {}
        present = kernel32.Process32FirstW(snapshot, ctypes.byref(entry))
        while present:
            tree[int(entry.process_id)] = int(entry.parent_process_id)
            present = kernel32.Process32NextW(snapshot, ctypes.byref(entry))
        return tree
    finally:
        kernel32.CloseHandle(snapshot)


def descendants(root: int, tree: dict[int, int]) -> set[int]:
    found: set[int] = set()
    frontier = {root}
    while frontier:
        next_frontier = {pid for pid, parent in tree.items() if parent in frontier and pid not in found}
        found.update(next_frontier)
        frontier = next_frontier
    return found


def windows_peak_working_set(process: subprocess.Popen[bytes]) -> int:
    from ctypes import wintypes

    class ProcessMemoryCounters(ctypes.Structure):
        _fields_ = [
            ("size", wintypes.DWORD),
            ("page_fault_count", wintypes.DWORD),
            ("peak_working_set_size", ctypes.c_size_t),
            ("working_set_size", ctypes.c_size_t),
            ("quota_peak_paged_pool_usage", ctypes.c_size_t),
            ("quota_paged_pool_usage", ctypes.c_size_t),
            ("quota_peak_non_paged_pool_usage", ctypes.c_size_t),
            ("quota_non_paged_pool_usage", ctypes.c_size_t),
            ("pagefile_usage", ctypes.c_size_t),
            ("peak_pagefile_usage", ctypes.c_size_t),
        ]

    counters = ProcessMemoryCounters()
    counters.size = ctypes.sizeof(counters)
    if not ctypes.WinDLL("psapi", use_last_error=True).GetProcessMemoryInfo(
        int(process._handle), ctypes.byref(counters), counters.size
    ):
        error = ctypes.get_last_error()
        if process.poll() is None:
            raise ctypes.WinError(error)
        return 0
    return int(counters.peak_working_set_size)


def minimal_environment() -> dict[str, str]:
    if os.name != "nt":
        return {"LANG": "C", "LC_ALL": "C"}
    environment = {}
    for name in ("SystemRoot", "WINDIR"):
        value = os.environ.get(name)
        if value:
            environment[name] = value
    if "SystemRoot" not in environment:
        raise RuntimeError("SystemRoot is required to launch the packaged binary")
    return environment


def measure(args: argparse.Namespace) -> None:
    command = args.command
    if command and command[0] == "--":
        command = command[1:]
    if not command:
        raise RuntimeError("measurement requires a product command after --")

    child_pids: set[int] = set()
    peak_windows = 0
    stop = threading.Event()
    with args.stdout.open("xb") as stdout, args.stderr.open("xb") as stderr:
        started = time.perf_counter_ns()
        process = subprocess.Popen(
            command,
            cwd=args.cwd,
            env=minimal_environment(),
            stdin=subprocess.DEVNULL,
            stdout=stdout,
            stderr=stderr,
        )

        def observe() -> None:
            nonlocal peak_windows
            while not stop.is_set():
                tree = windows_process_tree() if os.name == "nt" else linux_process_tree()
                child_pids.update(descendants(process.pid, tree))
                if os.name == "nt":
                    peak_windows = max(peak_windows, windows_peak_working_set(process))
                stop.wait(0.001)

        observer = threading.Thread(target=observe, name="lumin-process-observer", daemon=True)
        observer.start()
        exit_code = process.wait()
        elapsed = time.perf_counter_ns() - started
        stop.set()
        observer.join()
        if os.name == "nt":
            peak_rss = max(peak_windows, windows_peak_working_set(process))
            rss_source = "GetProcessMemoryInfo.PeakWorkingSetSize"
        else:
            import resource

            peak_rss = int(resource.getrusage(resource.RUSAGE_CHILDREN).ru_maxrss) * 1024
            rss_source = "wait4-rusage-ru_maxrss-kib"
        stdout.flush()
        stderr.flush()
        os.fsync(stdout.fileno())
        os.fsync(stderr.fileno())

    measurement = {
        "schemaVersion": SCHEMA,
        "analysisChildPids": sorted(child_pids),
        "elapsedNanoseconds": elapsed,
        "exitCode": exit_code,
        "observerResolutionNanoseconds": 1_000_000,
        "peakRssBytes": peak_rss,
        "rssSource": rss_source,
    }
    if args.subcommand == "measure-audit-diagnostic":
        measurement["schemaVersion"] = "lumin.phase1-process-measurement.v2"
        measurement["processId"] = process.pid
    write_new(args.output, measurement)


def main() -> None:
    parser = argparse.ArgumentParser()
    subparsers = parser.add_subparsers(dest="subcommand", required=True)
    host = subparsers.add_parser("host")
    host.add_argument("--root", required=True, type=Path)
    host.add_argument("--output", required=True, type=Path)
    for name in ("measure", "measure-audit-diagnostic"):
        run = subparsers.add_parser(name)
        run.add_argument("--cwd", required=True, type=Path)
        run.add_argument("--output", required=True, type=Path)
        run.add_argument("--stdout", required=True, type=Path)
        run.add_argument("--stderr", required=True, type=Path)
        run.add_argument("command", nargs=argparse.REMAINDER)
    args = parser.parse_args()
    if args.subcommand == "host":
        write_new(args.output, host_info(args.root))
    else:
        measure(args)


if __name__ == "__main__":
    main()
