#!/usr/bin/env python3
"""Probe the namespace primitives available on one WSL /mnt drvfs mount."""

from __future__ import annotations

import argparse
import ctypes
import errno
import json
import os
import platform
import shutil
import tempfile
from pathlib import Path


AT_FDCWD = -100
RENAME_NOREPLACE = 1


def mount_record(path: Path) -> dict[str, object]:
    resolved = str(path.resolve(strict=True))
    winner: tuple[str, list[str]] | None = None
    for line in Path("/proc/self/mountinfo").read_text(encoding="utf-8").splitlines():
        fields = line.split()
        separator = fields.index("-")
        mount_point = fields[4].replace("\\040", " ").replace("\\134", "\\")
        if resolved == mount_point or resolved.startswith(mount_point.rstrip("/") + "/"):
            if winner is None or len(mount_point) > len(winner[0]):
                winner = (mount_point, fields)
    if winner is None:
        raise RuntimeError(f"cannot identify mount for {resolved}")
    mount_point, fields = winner
    separator = fields.index("-")
    return {
        "mountPoint": mount_point,
        "filesystemType": fields[separator + 1],
        "mountSource": fields[separator + 2],
        "mountOptions": fields[5].split(","),
        "superOptions": fields[separator + 3].split(","),
    }


def outcome(call: object) -> dict[str, object]:
    try:
        call()  # type: ignore[operator]
    except OSError as error:
        return {
            "status": "error",
            "errno": error.errno,
            "errnoName": errno.errorcode.get(error.errno or 0, "UNKNOWN"),
            "message": error.strerror,
        }
    return {"status": "success"}


def renameat2(source: Path, destination: Path, flags: int) -> None:
    libc = ctypes.CDLL(None, use_errno=True)
    rename = libc.renameat2
    rename.argtypes = [ctypes.c_int, ctypes.c_char_p, ctypes.c_int, ctypes.c_char_p, ctypes.c_uint]
    rename.restype = ctypes.c_int
    result = rename(
        AT_FDCWD,
        os.fsencode(source),
        AT_FDCWD,
        os.fsencode(destination),
        flags,
    )
    if result != 0:
        code = ctypes.get_errno()
        raise OSError(code, os.strerror(code))


def probe(parent: Path) -> dict[str, object]:
    parent = parent.resolve(strict=True)
    scratch = Path(tempfile.mkdtemp(prefix="lumin-rename-noreplace-", dir=parent))
    try:
        file_source = scratch / "file-source"
        file_destination = scratch / "file-destination"
        file_source.write_bytes(b"source")
        file_noreplace = outcome(
            lambda: renameat2(file_source, file_destination, RENAME_NOREPLACE)
        )
        file_noreplace["sourceStillExists"] = file_source.is_file()
        file_noreplace["destinationAbsent"] = not file_destination.exists()

        directory_source = scratch / "directory-source"
        directory_destination = scratch / "directory-destination"
        directory_source.mkdir()
        (directory_source / "child").write_bytes(b"child")
        directory_noreplace = outcome(
            lambda: renameat2(directory_source, directory_destination, RENAME_NOREPLACE)
        )
        directory_noreplace["sourceStillExists"] = directory_source.is_dir()
        directory_noreplace["destinationAbsent"] = not directory_destination.exists()

        flags_zero_source = scratch / "flags-zero-source"
        flags_zero_destination = scratch / "flags-zero-destination"
        flags_zero_source.mkdir()
        flags_zero = outcome(lambda: renameat2(flags_zero_source, flags_zero_destination, 0))
        flags_zero["sourceAbsent"] = not flags_zero_source.exists()
        flags_zero["destinationExists"] = flags_zero_destination.is_dir()

        hard_link_source = scratch / "hard-link-source"
        hard_link_destination = scratch / "hard-link-destination"
        hard_link_source.write_bytes(b"source")
        file_hard_link = outcome(lambda: os.link(hard_link_source, hard_link_destination))
        file_hard_link["sourceLinkCount"] = hard_link_source.stat().st_nlink

        directory_link_source = scratch / "directory-link-source"
        directory_link_destination = scratch / "directory-link-destination"
        directory_link_source.mkdir()
        directory_hard_link = outcome(
            lambda: os.link(directory_link_source, directory_link_destination)
        )

        return {
            "schemaVersion": "lumin.phase1-wsl-mnt-namespace-probe.v1",
            "host": {
                "platform": platform.platform(),
                "kernelRelease": platform.release(),
                "machine": platform.machine(),
                "pythonVersion": platform.python_version(),
                "wsl": "microsoft" in (platform.release() + platform.version()).lower(),
            },
            "mount": mount_record(parent),
            "results": {
                "regularFileRenameNoReplace": file_noreplace,
                "directoryRenameNoReplace": directory_noreplace,
                "directoryRenameFlagsZero": flags_zero,
                "regularFileHardLink": file_hard_link,
                "directoryHardLink": directory_hard_link,
            },
        }
    finally:
        shutil.rmtree(scratch)


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--parent", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    args = parser.parse_args()
    report = probe(args.parent)
    encoded = (json.dumps(report, indent=2, sort_keys=True) + "\n").encode("utf-8")
    with args.output.open("xb") as stream:
        stream.write(encoded)
        stream.flush()
        os.fsync(stream.fileno())


if __name__ == "__main__":
    main()
