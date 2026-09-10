#!/usr/bin/env python3
"""Verify the retained WSL /mnt namespace primitive observation."""

from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("packet", type=Path)
    args = parser.parse_args()
    packet = args.packet.resolve(strict=True)
    evidence_path = packet / "evidence" / "wsl2-mnt-drvfs.json"
    evidence_bytes = evidence_path.read_bytes()
    evidence = json.loads(evidence_bytes)
    expected_sha256 = (packet / "evidence" / "SHA256SUMS").read_text(encoding="utf-8").split()[0]
    assert hashlib.sha256(evidence_bytes).hexdigest() == expected_sha256
    assert evidence["schemaVersion"] == "lumin.phase1-wsl-mnt-namespace-probe.v1"
    assert evidence["host"]["wsl"] is True
    assert evidence["mount"]["filesystemType"] == "9p"
    assert any(option.startswith("aname=drvfs") for option in evidence["mount"]["superOptions"])
    for key in ("regularFileRenameNoReplace", "directoryRenameNoReplace"):
        result = evidence["results"][key]
        assert result["status"] == "error"
        assert result["errnoName"] == "EINVAL"
        assert result["sourceStillExists"] is True
        assert result["destinationAbsent"] is True
    assert evidence["results"]["directoryRenameFlagsZero"]["status"] == "success"
    assert evidence["results"]["regularFileHardLink"]["status"] == "success"
    assert evidence["results"]["regularFileHardLink"]["sourceLinkCount"] == 2
    assert evidence["results"]["directoryHardLink"]["status"] == "error"
    print("PASS: retained WSL /mnt primitive evidence matches the reviewed observation")


if __name__ == "__main__":
    main()
