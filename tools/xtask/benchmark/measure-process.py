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


class WindowsApi:
    """One pointer-width-correct owner of the process/job observation APIs."""

    class Accounting(ctypes.Structure):
        _fields_ = [
            ("user", ctypes.c_int64), ("kernel", ctypes.c_int64),
            ("period_user", ctypes.c_int64), ("period_kernel", ctypes.c_int64),
            ("faults", ctypes.c_uint32), ("total", ctypes.c_uint32),
            ("active", ctypes.c_uint32), ("terminated", ctypes.c_uint32),
        ]

    class Limits(ctypes.Structure):
        _fields_ = [
            ("process_time", ctypes.c_int64), ("job_time", ctypes.c_int64),
            ("flags", ctypes.c_uint32), ("min_working_set", ctypes.c_size_t),
            ("max_working_set", ctypes.c_size_t), ("active_limit", ctypes.c_uint32),
            ("affinity", ctypes.c_size_t), ("priority", ctypes.c_uint32),
            ("scheduling", ctypes.c_uint32),
        ]

    class FileTime(ctypes.Structure):
        _fields_ = [("low", ctypes.c_uint32), ("high", ctypes.c_uint32)]

        def value(self) -> int:
            return (self.high << 32) | self.low

    class MemoryCounters(ctypes.Structure):
        _fields_ = [
            ("size", ctypes.c_uint32), ("faults", ctypes.c_uint32),
            ("peak", ctypes.c_size_t), ("working_set", ctypes.c_size_t),
            ("peak_paged", ctypes.c_size_t), ("paged", ctypes.c_size_t),
            ("peak_nonpaged", ctypes.c_size_t), ("nonpaged", ctypes.c_size_t),
            ("pagefile", ctypes.c_size_t), ("peak_pagefile", ctypes.c_size_t),
        ]

    def __init__(self) -> None:
        kernel = ctypes.WinDLL("kernel32", use_last_error=True)
        psapi = ctypes.WinDLL("psapi", use_last_error=True)
        handle, boolean, dword = ctypes.c_void_p, ctypes.c_int32, ctypes.c_uint32
        pointer = ctypes.POINTER
        for dll, name, arguments, result in [
            (kernel, "CreateJobObjectW", [handle, ctypes.c_wchar_p], handle),
            (kernel, "GetCurrentProcess", [], handle),
            (kernel, "AssignProcessToJobObject", [handle, handle], boolean),
            (kernel, "IsProcessInJob", [handle, handle, pointer(boolean)], boolean),
            (kernel, "QueryInformationJobObject", [handle, ctypes.c_int32, handle, dword, pointer(dword)], boolean),
            (kernel, "GetProcessTimes", [handle] + [pointer(self.FileTime)] * 4, boolean),
            (kernel, "TerminateJobObject", [handle, dword], boolean),
            (kernel, "CloseHandle", [handle], boolean),
            (psapi, "GetProcessMemoryInfo", [handle, pointer(self.MemoryCounters), dword], boolean),
        ]:
            function = getattr(dll, name)
            function.argtypes, function.restype = arguments, result
            setattr(self, name, function)

    @staticmethod
    def checked(result: object, name: str) -> None:
        if not result:
            error = ctypes.get_last_error()
            raise OSError(error, f"{name}: {ctypes.FormatError(error)}")

    def create(self) -> int:
        handle = self.CreateJobObjectW(None, None)
        self.checked(handle, "CreateJobObjectW")
        return handle

    def assign(self, job: int, process: int) -> None:
        self.checked(self.AssignProcessToJobObject(job, process), "AssignProcessToJobObject")

    def member(self, job: int, process: int) -> bool:
        member = ctypes.c_int32()
        self.checked(self.IsProcessInJob(process, job, ctypes.byref(member)), "IsProcessInJob")
        return bool(member.value)

    def query(self, job: int, kind: int, record: ctypes.Structure) -> None:
        returned = ctypes.c_uint32()
        self.checked(self.QueryInformationJobObject(
            job, kind, ctypes.byref(record), ctypes.sizeof(record), ctypes.byref(returned)
        ), "QueryInformationJobObject")
        if returned.value != ctypes.sizeof(record):
            raise RuntimeError("partial job information")

    def flags(self, job: int) -> int:
        limits = self.Limits()
        self.query(job, 2, limits)  # JobObjectBasicLimitInformation
        return limits.flags

    def accounting(self, job: int) -> dict[str, int]:
        record = self.Accounting()
        self.query(job, 1, record)  # JobObjectBasicAccountingInformation
        return {"totalProcesses": record.total, "activeProcesses": record.active,
                "totalTerminatedProcesses": record.terminated}

    def times(self, process: int) -> tuple[int, int]:
        creation, exit_time, kernel, user = (self.FileTime() for _ in range(4))
        self.checked(self.GetProcessTimes(process, ctypes.byref(creation), ctypes.byref(exit_time),
                                         ctypes.byref(kernel), ctypes.byref(user)), "GetProcessTimes")
        return creation.value(), exit_time.value()

    def peak(self, process: int) -> int:
        counters = self.MemoryCounters()
        counters.size = ctypes.sizeof(counters)
        self.checked(self.GetProcessMemoryInfo(process, ctypes.byref(counters), counters.size),
                     "GetProcessMemoryInfo")
        if counters.peak == 0:
            raise RuntimeError("zero process peak working set")
        return counters.peak

    def close(self, handle: int) -> None:
        self.checked(self.CloseHandle(handle), "CloseHandle")

    def terminate(self, job: int) -> None:
        self.checked(self.TerminateJobObject(job, 1), "TerminateJobObject")


def validate_windows_observation(record: dict[str, object], *, allow_children: bool = False) -> None:
    before, after = record["before"], record["after"]
    if (before != {"totalProcesses": 1, "activeProcesses": 1, "totalTerminatedProcesses": 0}
            or record["limitFlagsBefore"] != 0 or record["limitFlagsAfter"] != 0
            or not record["helperInJob"] or not record["processInJob"]
            or not 0 < record["helperProcessId"] <= 0xFFFFFFFF
            or not 0 < record["processId"] <= 0xFFFFFFFF
            or record["helperProcessId"] == record["processId"]
            or not 0 < record["helperCreationTime100ns"] <= record["processCreationTime100ns"]
            <= record["processExitTime100ns"] <= 0xFFFFFFFFFFFFFFFF
            or not 2 <= after["totalProcesses"] <= 0xFFFFFFFF
            or not 1 <= after["activeProcesses"] <= after["totalProcesses"]
            or after["totalTerminatedProcesses"] != 0):
        raise RuntimeError("contradictory Windows process observation")
    if not allow_children and after["totalProcesses"] != 2:
        raise RuntimeError(f"measured product launched analysis child processes: job total {after['totalProcesses']}")


class WindowsJob:
    def __init__(self, api: WindowsApi | None = None) -> None:
        self.api = api if api is not None else WindowsApi()
        self.handle = None
        self.confirmed = False
        self.record = {"schemaVersion": "lumin.windows-process-observation.v1",
                       "method": "private-inherited-job.v1", "helperProcessId": os.getpid()}

    def admit(self) -> None:
        self.handle = self.api.create()
        helper = self.api.GetCurrentProcess()
        self.api.assign(self.handle, helper)
        self.confirmed = self.api.member(self.handle, helper)
        if not self.confirmed:
            raise RuntimeError("measurement helper is not in its private job")
        self.record.update(helperInJob=True, helperCreationTime100ns=self.api.times(helper)[0],
                           limitFlagsBefore=self.api.flags(self.handle), before=self.api.accounting(self.handle))
        if (self.record["helperCreationTime100ns"] == 0 or self.record["limitFlagsBefore"] != 0
                or self.record["before"] != {"totalProcesses": 1, "activeProcesses": 1,
                                            "totalTerminatedProcesses": 0}):
            raise RuntimeError("invalid private job before product launch")

    def bind(self, process: subprocess.Popen[bytes]) -> None:
        handle = int(process._handle)
        self.record.update(processId=process.pid, processCreationTime100ns=self.api.times(handle)[0],
                           processInJob=self.api.member(self.handle, handle))
        if not self.record["processInJob"]:
            raise RuntimeError("measured product did not inherit its private job")

    def finish(self, process: subprocess.Popen[bytes], output: Path) -> None:
        creation, exit_time = self.api.times(int(process._handle))
        if creation != self.record["processCreationTime100ns"]:
            raise RuntimeError("held product creation time changed")
        self.record.update(processExitTime100ns=exit_time, limitFlagsAfter=self.api.flags(self.handle),
                           after=self.api.accounting(self.handle))
        write_new(output.with_name("windows-process-observation.json"), self.record)
        validate_windows_observation(self.record)

    def close(self) -> None:
        if self.handle is not None:
            self.api.close(self.handle)
            self.handle = None

    def abort(self, error: BaseException) -> None:
        # A private helper is intentionally terminated too. Flush diagnostics first;
        # do not claim that asynchronous subtree cleanup has completed.
        try:
            print(f"benchmark observer failed: {error}; private-job cleanup "
                  + ("requested" if self.confirmed else "not authorized"), file=sys.stderr, flush=True)
        finally:
            if self.confirmed and self.handle is not None:
                try:
                    self.api.terminate(self.handle)
                except OSError as cleanup_error:
                    print(f"private-job cleanup unknown: {cleanup_error}", file=sys.stderr, flush=True)
            self.close()


def descendants(root: int, tree: dict[int, int]) -> set[int]:
    found: set[int] = set()
    frontier = {root}
    while frontier:
        next_frontier = {pid for pid, parent in tree.items() if parent in frontier and pid not in found}
        found.update(next_frontier)
        frontier = next_frontier
    return found


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

    job = WindowsJob() if os.name == "nt" else None
    try:
        if job is not None:
            job.admit()
        measure_in_job(args, command, job)
        if job is not None:
            job.close()
    except BaseException as error:
        if job is not None:
            job.abort(error)
        raise


def measure_in_job(args: argparse.Namespace, command: list[str], job: WindowsJob | None) -> None:
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

        errors: list[BaseException] = []

        def observe() -> None:
            nonlocal peak_windows
            try:
                while not stop.is_set():
                    if job is not None:
                        peak_windows = max(peak_windows, job.api.peak(int(process._handle)))
                    else:
                        child_pids.update(descendants(process.pid, linux_process_tree()))
                    stop.wait(0.001)
            except BaseException as error:
                errors.append(error)

        observer = threading.Thread(target=observe, name="lumin-process-observer", daemon=True)
        try:
            if job is not None:
                job.bind(process)
            observer.start()
            exit_code = process.wait()
            elapsed = time.perf_counter_ns() - started
            stop.set()
            observer.join()
            if errors:
                raise RuntimeError(f"process observer thread failed: {errors[0]}") from errors[0]
            if job is not None:
                peak_rss = max(peak_windows, job.api.peak(int(process._handle)))
                rss_source = "GetProcessMemoryInfo.PeakWorkingSetSize"
                job.finish(process, args.output)
            else:
                import resource

                peak_rss = int(resource.getrusage(resource.RUSAGE_CHILDREN).ru_maxrss) * 1024
                rss_source = "wait4-rusage-ru_maxrss-kib"
        finally:
            stop.set()
            if observer.ident is not None:
                observer.join()
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
