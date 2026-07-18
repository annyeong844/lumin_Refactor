#!/usr/bin/env python3
"""Verify local numeric-target packet bytes and deterministic derivations."""

from __future__ import annotations

import argparse
import hashlib
import json
import re
import subprocess
import sys
import tempfile
from pathlib import Path, PurePosixPath


EXPECTED_COMMIT = "35290cb683a37e83bc8c915d1d0f9ca0f3f96fd0"
EXPECTED_TARGETS = {
    "binarySizeBytesMax": 12_582_912,
    "coldFullAuditP50MsMax": 30_000,
    "coldPreWriteP50MsMax": 6_000,
    "defaultJobsFormula": "max(1,min(8,available_parallelism))",
    "defaultJobsVsJobs1ColdFullP50RatioMax": 0.75,
    "peakRssBytesMax": 536_870_912,
    "postWrite32FilesP50MsMax": 8_000,
    "postWriteOneFileP50MsMax": 4_000,
    "repetitionsPerEnvironment": 3,
    "warmPreWriteP50MsMax": 4_000,
    "warmUnchangedAuditP50MsMax": 8_000,
    "workerStackBytes": 4_194_304,
}

CANDIDATE_PATHS = (
    ".gitattributes",
    ".gitignore",
    "AGENTS.md",
    "README.md",
    "SDD.md",
    "WORKBOARD.md",
    "architecture/000-system-blueprint.md",
    "architecture/001-execution-and-ownership.md",
    "architecture/002-evidence-and-write-gate.md",
    "specs/000-product-contract.md",
    "specs/001-foundation-slice.md",
    "specs/inventory-config-semantics.v1.json",
    "specs/repo-path-semantics.v1.json",
    "specs/resolver-config-semantics.v1.json",
    "문서(한글)/AGENTS.ko.md",
    "문서(한글)/SDD.ko.md",
)


class DuplicateKey(ValueError):
    pass


def reject_duplicates(pairs: list[tuple[str, object]]) -> dict[str, object]:
    result: dict[str, object] = {}
    for key, value in pairs:
        if key in result:
            raise DuplicateKey(key)
        result[key] = value
    return result


def load_json(path: Path) -> dict[str, object]:
    value = json.loads(path.read_text(encoding="utf-8"), object_pairs_hook=reject_duplicates)
    if not isinstance(value, dict):
        raise ValueError(f"expected JSON object: {path}")
    return value


def sha256(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


class Checks:
    def __init__(self) -> None:
        self.rows: list[dict[str, str]] = []

    def check(self, name: str, condition: bool, detail: str = "") -> None:
        if not condition:
            raise AssertionError(f"{name}: {detail}")
        self.rows.append({"name": name, "status": "PASS"})


def verify_manifest(packet: Path, checks: Checks) -> None:
    manifest = packet / "SHA256SUMS"
    rows: list[tuple[str, str]] = []
    for line in manifest.read_text(encoding="utf-8").splitlines():
        match = re.fullmatch(r"([0-9a-f]{64})  ([^\r\n]+)", line)
        checks.check("manifest-line-format", match is not None, line)
        assert match is not None
        digest, relative = match.groups()
        path = PurePosixPath(relative)
        checks.check(
            "manifest-safe-path",
            not path.is_absolute() and ".." not in path.parts and "\\" not in relative,
            relative,
        )
        rows.append((digest, relative))

    paths = [relative for _, relative in rows]
    checks.check("manifest-order", paths == sorted(paths, key=lambda value: value.encode()))
    checks.check("manifest-unique", len(paths) == len(set(paths)))
    actual = sorted(
        path.relative_to(packet).as_posix()
        for path in packet.rglob("*")
        if path.is_file() and path.name != "SHA256SUMS"
    )
    checks.check("manifest-exact-inventory", paths == actual)
    for digest, relative in rows:
        checks.check(f"manifest-member:{relative}", sha256(packet / relative) == digest)


def verify_json_and_docs(repo: Path, packet: Path, checks: Checks) -> None:
    for path in sorted(packet.rglob("*.json")):
        load_json(path)
        checks.check(f"strict-json:{path.relative_to(packet).as_posix()}", True)

    for relative in CANDIDATE_PATHS:
        path = repo / relative
        data = path.read_bytes()
        checks.check(f"candidate-present:{relative}", path.is_file())
        checks.check(f"candidate-no-bom:{relative}", not data.startswith(b"\xef\xbb\xbf"))
        checks.check(f"candidate-no-cr:{relative}", b"\r" not in data)
        checks.check(f"candidate-final-lf:{relative}", data.endswith(b"\n"))
        if relative.endswith(".json"):
            load_json(path)
            checks.check(f"candidate-strict-json:{relative}", True)

    markdown_paths = (
        packet / "README.md",
        packet / "TARGET-CONTRACT.md",
        repo / "specs/001-foundation-slice.md",
    )
    link_pattern = re.compile(r"\[[^\]]*\]\(([^)]+)\)")
    for markdown in markdown_paths:
        for target in link_pattern.findall(markdown.read_text(encoding="utf-8")):
            if target.startswith(("http://", "https://", "#")):
                continue
            target_path = target.split("#", 1)[0]
            resolved = (markdown.parent / target_path).resolve()
            checks.check(
                f"local-link:{markdown.relative_to(repo).as_posix()}:{target}",
                resolved.exists(),
            )


def verify_scale_corpus(packet: Path, selection: dict[str, object], checks: Checks) -> None:
    retained_manifest = packet / "evidence/scale-corpus-manifest.json"
    retained_truth = packet / "evidence/scale-corpus-expected-truth.json"
    manifest = load_json(retained_manifest)
    truth = load_json(retained_truth)
    scale = selection["scaleCorpus"]
    assert isinstance(scale, dict)

    checks.check("scale-schema", manifest["schemaVersion"] == "phase1-scale-findings.v1")
    checks.check("scale-files", manifest["fileCount"] == scale["fileCount"] == 780)
    checks.check("scale-bytes", manifest["totalBytes"] == scale["totalBytes"])
    checks.check(
        "scale-content-digest",
        manifest["contentManifestSha256"] == scale["contentManifestSha256"],
    )
    checks.check("truth-filter", truth["filters"] == {})
    checks.check("truth-totals", truth["scopeTotal"] == truth["matchedTotal"] == 256)
    checks.check("truth-limitations", truth["limitations"] == [])
    checks.check("truth-breakdown", truth["breakdown"] == scale["findingBreakdown"])
    findings = truth["expectedFindings"]
    assert isinstance(findings, list)
    checks.check("truth-finding-count", len(findings) == 256)
    checks.check("truth-finding-tuples-unique", len({(row["path"], row["exportName"]) for row in findings}) == 256)

    with tempfile.TemporaryDirectory(prefix="lumin-scale-corpus-") as temp:
        temp_root = Path(temp)
        generated_manifest = temp_root / "manifest.json"
        generated_truth = temp_root / "truth.json"
        subprocess.run(
            [
                sys.executable,
                str(packet / "source/generate-scale-corpus.py"),
                "--output",
                str(temp_root / "corpus"),
                "--manifest",
                str(generated_manifest),
                "--truth",
                str(generated_truth),
            ],
            check=True,
        )
        checks.check("scale-manifest-reproduction", generated_manifest.read_bytes() == retained_manifest.read_bytes())
        checks.check("scale-truth-reproduction", generated_truth.read_bytes() == retained_truth.read_bytes())


def parse_gnu_time(path: Path) -> tuple[int, int]:
    text = path.read_text(encoding="utf-8")
    elapsed_match = re.search(r"Elapsed \(wall clock\) time \(h:mm:ss or m:ss\):\s*(\S+)", text)
    rss_match = re.search(r"Maximum resident set size \(kbytes\):\s*(\d+)", text)
    if elapsed_match is None or rss_match is None:
        raise AssertionError(f"missing GNU time fields: {path}")
    parts = [float(part) for part in elapsed_match.group(1).split(":")]
    seconds = parts[-1]
    if len(parts) == 2:
        seconds += parts[0] * 60
    elif len(parts) == 3:
        seconds += parts[0] * 3600 + parts[1] * 60
    return round(seconds * 1000), int(rss_match.group(1)) * 1024


def verify_baselines(packet: Path, selection: dict[str, object], checks: Checks) -> None:
    records = selection["legacyBaselines"]
    assert isinstance(records, list)
    checks.check("baseline-record-count", len(records) == 4)
    for record in records:
        assert isinstance(record, dict)
        platform = str(record["platform"])
        mode = str(record["mode"])
        root = packet / "evidence/legacy" / platform / mode
        host = load_json(root.parent / "host.json")
        measurement = load_json(root / "measurement.json")
        manifest = load_json(root / "manifest.json")
        triage = load_json(root / "triage.json")
        fix_plan = load_json(root / "fix-plan.json")
        prefix = f"baseline:{platform}:{mode}"

        checks.check(f"{prefix}:host-platform", host["platform"] == platform)
        checks.check(f"{prefix}:host-cpu", "i7-9750H" in host["cpu"])
        checks.check(f"{prefix}:host-workers", host["logicalProcessors"] == 12)
        checks.check(f"{prefix}:host-filesystem", platform.split("-", 1)[1] in host["filesystem"].lower())
        checks.check(f"{prefix}:commit", measurement["baselineCommit"] == EXPECTED_COMMIT)
        checks.check(f"{prefix}:mode", measurement["mode"] == mode)
        checks.check(f"{prefix}:files", manifest["scanRange"]["files"] == record["files"] == 2038)
        checks.check(f"{prefix}:loc", triage["shape"]["totalLoc"] == 400155)
        checks.check(f"{prefix}:js", triage["shape"]["jsFiles"] == 677)
        checks.check(f"{prefix}:ts", triage["shape"]["tsFiles"] == 28)
        checks.check(f"{prefix}:rust", triage["shape"]["rustFiles"] == 1333)
        checks.check(f"{prefix}:artifact-bytes", manifest["performance"]["totalArtifactBytes"] == record["artifactBytes"])
        checks.check(f"{prefix}:producer-wall", manifest["performance"]["totalWallMs"] == record["producerWallMs"])
        expected_counts = record["fixPlanCounts"]
        checks.check(
            f"{prefix}:fix-counts",
            all(fix_plan["summary"][key] == value for key, value in expected_counts.items()),
        )

        if platform == "windows-ntfs":
            elapsed = measurement["elapsedMs"]
            peak = measurement["processTreePeakRssBytes"]
            samples = measurement["samples"]
            assert isinstance(samples, list)
            checks.check(f"{prefix}:sample-count", measurement["sampleCount"] == len(samples) and len(samples) > 0)
            checks.check(f"{prefix}:sample-peak", peak == max(row["processTreeRssBytes"] for row in samples))
        else:
            elapsed, peak = parse_gnu_time(root / "time.txt")
            checks.check(f"{prefix}:gnu-time-owner", measurement["timeOwner"] == "GNU time -v")
            checks.check(f"{prefix}:gnu-time-elapsed", measurement["elapsedMs"] == elapsed)
            checks.check(f"{prefix}:gnu-time-rss", measurement["maxResidentSetBytes"] == peak)
        checks.check(f"{prefix}:selection-elapsed", elapsed == record["elapsedMs"])
        checks.check(f"{prefix}:selection-rss", peak == record["peakRssBytes"])

    lifecycle = selection["legacyPackagedLifecycleBaseline"]
    record = packet / "evidence/legacy/wsl-prewrite-discovery-measurement-2026-07-11.md"
    checks.check("legacy-lifecycle-record-sha", sha256(record) == lifecycle["recordSha256"])
    checks.check("legacy-lifecycle-record-path", lifecycle["recordPath"] == "docs/lab/wsl-prewrite-discovery-measurement-2026-07-11.md")
    checks.check("legacy-lifecycle-files", lifecycle["fileCount"] == 564)
    checks.check("legacy-lifecycle-cargo", lifecycle["cargoAvailable"] is False)
    checks.check("legacy-lifecycle-cold", lifecycle["coldPreWriteMs"] == 3460)
    checks.check("legacy-lifecycle-warm", lifecycle["warmPreWriteMsRange"] == [3070, 3620])
    checks.check("legacy-lifecycle-post", lifecycle["postWriteMs"] == 2870)


def verify_probe_inputs(repo: Path, selection: dict[str, object], checks: Checks) -> None:
    probe = selection["probeInputs"]
    assert isinstance(probe, dict)
    oxc_path = repo / "reviews/probes/phase0-oxc-memory-stack-windows-wsl2-x64-2026-07-17/evidence/summary.json"
    oxc = load_json(oxc_path)
    checks.check("oxc-status", oxc["status"] == "PASS")
    checks.check("oxc-corpus-files", oxc["corpus_file_count"] == probe["oxcCorpusFiles"] == 709)
    checks.check("oxc-corpus-bytes", oxc["corpus_bytes"] == probe["oxcCorpusBytes"] == 7_472_776)
    checks.check("oxc-corpus-digest", oxc["corpus_manifest_sha256"] == probe["oxcCorpusManifestSha256"])
    for platform in oxc["platforms"]:
        platform_name = platform["platform"]
        checks.check(f"oxc-stack-min:{platform_name}", platform["minimum_observed_passing_stack_bytes"] == 1_048_576)
        four_mib = next(row for row in platform["stack_results"] if row["stack_bytes"] == 4_194_304)
        checks.check(f"oxc-stack-4mib:{platform_name}", four_mib["status"] == "PASS")
        jobs = {row["workers"]: row for row in platform["jobs"]}
        ratio = jobs[8]["child_elapsed_micros"] / jobs[1]["child_elapsed_micros"]
        key = "windows-ntfs" if platform_name == "windows-x64" else "wsl2-ext4"
        checks.check(f"oxc-jobs-ratio:{platform_name}", round(ratio, 6) == probe["oxcJobs1ToJobs8Ratios"][key])
    checks.check(
        "oxc-stack-margin",
        EXPECTED_TARGETS["workerStackBytes"] // probe["oxcMinimumPassingStackBytes"] == probe["selectedStackMargin"] == 4,
    )

    static_root = repo / "reviews/probes/phase0-static-packaging-windows-wsl2-native-linux-x64-2026-07-18/evidence"
    sizes: list[int] = []
    for platform in ("windows-ntfs", "wsl2-ext4", "native-linux-ext4"):
        summary = load_json(static_root / platform / "summary.json")
        checks.check(f"static-status:{platform}", summary["status"] == "PASS")
        sizes.extend(int(row["sizeBytes"]) for row in summary["artifacts"])
    checks.check("static-largest-binary", max(sizes) == probe["largestStaticPackagingProbeBytes"] == 1_901_280)


def verify_selection(selection: dict[str, object], checks: Checks) -> None:
    checks.check("selection-schema", selection["schemaVersion"] == "phase0-numeric-target-selection.v1")
    checks.check("selection-status", selection["status"] == "candidate-awaiting-independent-review")
    checks.check("selection-legacy-commit", selection["legacyCommit"] == EXPECTED_COMMIT)
    architecture = selection["architecture"]
    checks.check(
        "selection-false-negative-baseline",
        architecture["falseNegativeBaselineCommit"] == "a1e07ed7b9e05181cd58bfba5f3846c1baab8a93",
    )
    checks.check(
        "selection-false-negative-manifest",
        architecture["falseNegativeBaselineManifestSha256"] == "8b0d2ceddb930533e6967c48e06954f09f53abf8f7a688f4dfb0baeb050a6339",
    )
    checks.check("selection-targets", selection["targets"] == EXPECTED_TARGETS)
    boundary = selection["claimBoundary"]
    checks.check("selection-not-achieved", boundary["achievedProductBudget"] is False)
    checks.check("selection-legacy-not-truth", boundary["legacyOutputIsProductTruth"] is False)
    checks.check("selection-review-required", boundary["phase1MayStartBeforeIndependentPass"] is False)


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--json-output", type=Path)
    args = parser.parse_args()

    packet = Path(__file__).resolve().parent.parent
    repo = packet.parents[2]
    checks = Checks()
    selection = load_json(packet / "evidence/target-selection.json")
    verify_manifest(packet, checks)
    verify_json_and_docs(repo, packet, checks)
    verify_selection(selection, checks)
    verify_scale_corpus(packet, selection, checks)
    verify_baselines(packet, selection, checks)
    verify_probe_inputs(repo, selection, checks)

    result = {
        "checks": checks.rows,
        "failed": 0,
        "passed": len(checks.rows),
        "schemaVersion": "phase0-numeric-target-checks.v1",
        "status": "PASS",
    }
    encoded = json.dumps(result, indent=2, sort_keys=True) + "\n"
    if args.json_output:
        args.json_output.write_text(encoded, encoding="utf-8")
    print(encoded, end="")


if __name__ == "__main__":
    main()
