#!/usr/bin/env python3
from __future__ import annotations

import argparse
import hashlib
import json
import re
import subprocess
import sys
from pathlib import Path, PurePosixPath


CANDIDATE = "658b5c7334fed8f0e42dd14b9910c9b719f3e55b"
PARENT = "a1c9f2ee73da78f755fe967d174524f074799683"
TREE = "15b8777486f7dfe45a1d57650a230b287f21df7b"
SUBJECT = "Regenerate exact-bound provenance evidence"
GATE_BASE = "6539e9fa7e419bca494e6ab9f1910b6647b942ba"
RUNNER = "7e6ebd097cd69318669494fbd95acecbf627b5b4"
PACKET = "reviews/probes/phase0-pinned-upstream-provenance-2026-07-18"
CANDIDATE_MANIFEST_SHA256 = (
    "ca46f77997c696f8eeefc2feabdb9c1031a6e58e36fcb6f2a7ed4ad1bca84fcd"
)
PACKET_MANIFEST_SHA256 = "cc887e448e2a560801c09cd082f55304f689148a74d6f91e837582de70df65a3"
SOURCE_MANIFEST_SHA256 = "14185a4c6c74cac84283b89ce2002f4da8c4afb44f50e5f21e2f236aa299d7f3"
EVIDENCE_MANIFEST_SHA256 = (
    "439eff660625b3792c9c6438be6d063a94dce07f6a40802b2368a962e0509b68"
)
RUN_ID = 29642350675
JOB_ID = 88074824267
ARTIFACT_ID = 8428995583
ARTIFACT_SHA256 = "4688a9a192349efe7114fc823474732797ee6ee1f3cf49301056a101dc6857c9"
ARTIFACT_SIZE = 6257298
HEX_64 = re.compile(r"^[0-9a-f]{64}$")


def sha256(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def require(condition: bool, detail: str) -> None:
    if not condition:
        raise RuntimeError(detail)


def git(repository: Path, *arguments: str, check: bool = True) -> bytes:
    completed = subprocess.run(
        ["git", *arguments],
        cwd=repository,
        check=False,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    if check:
        require(completed.returncode == 0, completed.stderr.decode("utf-8", "replace"))
    return completed.stdout


def git_blob(repository: Path, revision: str, path: str) -> bytes:
    return git(repository, "show", f"{revision}:{path}")


def safe_path(value: str) -> None:
    path = PurePosixPath(value)
    require(value != "" and "\\" not in value, f"unsafe path: {value}")
    require(not path.is_absolute(), f"absolute path: {value}")
    require(all(part not in ("", ".", "..") for part in path.parts), f"unsafe path: {value}")


def parse_manifest(data: bytes, label: str) -> dict[str, str]:
    text = data.decode("utf-8")
    require(text.endswith("\n") and "\r" not in text, f"bad framing: {label}")
    result: dict[str, str] = {}
    for line in text.splitlines():
        require(len(line) > 66 and line[64:66] == "  ", f"bad line: {label}")
        digest, path = line[:64], line[66:]
        require(HEX_64.fullmatch(digest) is not None, f"bad digest: {label}")
        safe_path(path)
        require(path not in result, f"duplicate path: {path}")
        result[path] = digest
    require(
        list(result) == sorted(result, key=lambda value: value.encode("utf-8")),
        f"non-ordinal paths: {label}",
    )
    return result


def strict_json(data: bytes, label: str):
    def pairs_hook(pairs):
        result = {}
        for key, value in pairs:
            require(key not in result, f"duplicate JSON key {key}: {label}")
            result[key] = value
        return result

    return json.loads(data.decode("utf-8"), object_pairs_hook=pairs_hook)


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--repository-root", required=True, type=Path)
    parser.add_argument("--request-root", type=Path, default=Path(__file__).resolve().parent)
    parser.add_argument("--artifact-zip", type=Path)
    arguments = parser.parse_args()
    repository = arguments.repository_root.resolve()
    request_root = arguments.request_root.resolve()
    checks: list[dict[str, str]] = []

    def passed(name: str, condition: bool = True) -> None:
        require(condition, name)
        checks.append({"name": name, "status": "pass"})

    passed("candidate object", git(repository, "cat-file", "-t", CANDIDATE).strip() == b"commit")
    metadata = git(repository, "show", "-s", "--format=%H%n%P%n%T%n%s", CANDIDATE).decode().splitlines()
    passed("candidate SHA", metadata[0] == CANDIDATE)
    passed("candidate parent", metadata[1] == PARENT)
    passed("candidate tree", metadata[2] == TREE)
    passed("candidate subject", metadata[3] == SUBJECT)

    request = strict_json((request_root / "closure-request.json").read_bytes(), "request")
    passed("request candidate", request["candidate"] == CANDIDATE)
    passed("request tree", request["candidate_tree"] == TREE)
    passed("request runner", request["runner_commit"] == RUNNER)

    candidate_manifest_bytes = (request_root / "candidate-manifest.txt").read_bytes()
    candidate_manifest = parse_manifest(candidate_manifest_bytes, "candidate manifest")
    passed("candidate manifest digest", sha256(candidate_manifest_bytes) == CANDIDATE_MANIFEST_SHA256)
    passed("candidate manifest cardinality", len(candidate_manifest) == 16)
    for path, expected in candidate_manifest.items():
        passed(f"candidate blob: {path}", sha256(git_blob(repository, CANDIDATE, path)) == expected)

    packet_bytes = git_blob(repository, CANDIDATE, f"{PACKET}/SHA256SUMS")
    packet = parse_manifest(packet_bytes, "packet manifest")
    passed("packet manifest digest", sha256(packet_bytes) == PACKET_MANIFEST_SHA256)
    passed("packet manifest cardinality", len(packet) == 34)
    passed("request packet copy", (request_root / "packet-manifest.txt").read_bytes() == packet_bytes)
    packet_tree = set(
        git(repository, "ls-tree", "-r", "--name-only", CANDIDATE, PACKET)
        .decode("utf-8")
        .splitlines()
    )
    expected_packet_tree = {f"{PACKET}/{path}" for path in packet} | {f"{PACKET}/SHA256SUMS"}
    passed("packet exact inventory", packet_tree == expected_packet_tree)
    for path, expected in packet.items():
        passed(f"packet blob: {path}", sha256(git_blob(repository, CANDIDATE, f"{PACKET}/{path}")) == expected)

    source_bytes = git_blob(repository, CANDIDATE, f"{PACKET}/source/SHA256SUMS")
    source = parse_manifest(source_bytes, "source manifest")
    passed("source manifest digest", sha256(source_bytes) == SOURCE_MANIFEST_SHA256)
    passed("source manifest cardinality", len(source) == 4)
    passed("request source copy", (request_root / "source-manifest.txt").read_bytes() == source_bytes)
    for path, expected in source.items():
        candidate_path = f"{PACKET}/source/{path}"
        passed(f"source candidate blob: {path}", sha256(git_blob(repository, CANDIDATE, candidate_path)) == expected)
        passed(f"source runner blob: {path}", sha256(git_blob(repository, RUNNER, candidate_path)) == expected)

    evidence_prefix = f"{PACKET}/evidence/native-linux-clean"
    evidence_bytes = git_blob(repository, CANDIDATE, f"{evidence_prefix}/SHA256SUMS")
    evidence = parse_manifest(evidence_bytes, "evidence manifest")
    passed("evidence manifest digest", sha256(evidence_bytes) == EVIDENCE_MANIFEST_SHA256)
    passed("evidence manifest cardinality", len(evidence) == 18)
    passed("request evidence copy", (request_root / "evidence-manifest.txt").read_bytes() == evidence_bytes)
    retained: dict[str, bytes] = {}
    for path, expected in evidence.items():
        data = git_blob(repository, CANDIDATE, f"{evidence_prefix}/{path}")
        retained[path] = data
        passed(f"evidence blob: {path}", sha256(data) == expected)

    for path in [path for path in packet if path.endswith(".json")]:
        strict_json(git_blob(repository, CANDIDATE, f"{PACKET}/{path}"), path)
        passed(f"strict JSON: {path}")

    oracle = strict_json(retained["oracle.json"], "oracle")
    tag_ref = strict_json(retained["identity/node-tag-ref.json"], "node tag ref")
    tag_object = strict_json(retained["identity/node-tag-object.json"], "node tag object")
    tag_target = tag_ref["object"]
    expected_tag_url = (
        "https://api.github.com/repos/nodejs/node/git/tags/" + tag_target["sha"]
    )
    passed("annotated tag ref", tag_target["type"] == "tag")
    passed("tag object URL derivation", tag_target["url"] == expected_tag_url)
    passed("tag object identity", tag_object["sha"] == tag_target["sha"])
    passed("tag commit identity", tag_object["object"]["sha"] == oracle["node"]["commit"])

    descriptors = [
        ("typescript-npm-tarball", oracle["typeScript"]["npmTarball"], "objects/typescript-6.0.0-beta.tgz"),
        ("typescript-module-resolver", oracle["typeScript"]["moduleResolver"]["url"], "objects/typescript-moduleNameResolver.ts"),
        ("typescript-config-parser", oracle["typeScript"]["configParser"]["url"], "objects/typescript-commandLineParser.ts"),
        ("node-packages-document", oracle["node"]["packagesDocument"]["url"], "objects/node-packages.md"),
        ("node-esm-resolver", oracle["node"]["resolverSource"]["url"], "objects/node-resolve.js"),
        ("pnpm-workspace-document", oracle["pnpm"]["workspaceDocument"]["url"], "objects/pnpm-workspace_yaml.md"),
        ("node-tag-ref", oracle["node"]["tagRefApi"], "identity/node-tag-ref.json"),
        ("node-tag-object", expected_tag_url, "identity/node-tag-object.json"),
    ]
    fetch = strict_json(retained["fetch-metadata.json"], "fetch metadata")
    responses = fetch["responses"]
    passed("fetch schema", fetch["schemaVersion"] == "lumin-phase0-provenance-fetch.v2")
    passed("eight fetch responses", len(responses) == len(descriptors))
    passed("exact fetch IDs", [row["id"] for row in responses] == [item[0] for item in descriptors])
    for row, (identifier, url, path) in zip(responses, descriptors, strict=True):
        data = retained[path]
        passed(f"fetch URL: {identifier}", row["url"] == url and row["finalUrl"] == url)
        passed(f"fetch status: {identifier}", type(row["status"]) is int and row["status"] == 200)
        passed(f"fetch encoding: {identifier}", row["contentEncoding"] in (None, "", "identity"))
        passed(f"fetch retained path: {identifier}", row["retainedPath"] == path)
        passed(f"fetch byte length: {identifier}", row["sizeBytes"] == len(data))
        passed(f"fetch content length: {identifier}", int(row["contentLengthHeader"]) == len(data))
        passed(f"fetch digest: {identifier}", row["sha256"] == sha256(data))

    host = strict_json(retained["host.json"], "host")
    result = strict_json(retained["result.json"], "result")
    workflow = strict_json(git_blob(repository, CANDIDATE, f"{PACKET}/runner/workflow-run.json"), "workflow")
    environment = host["environment"]
    passed("host schema", host["schemaVersion"] == "lumin-phase0-provenance-host.v2")
    passed("result schema", result["schemaVersion"] == "lumin-phase0-pinned-upstream-provenance-result.v2")
    passed("host runner identity", host["repositoryHead"] == RUNNER)
    passed("GITHUB_SHA identity", environment["GITHUB_SHA"] == RUNNER)
    passed("result runner identity", result["runnerCommit"] == RUNNER)
    passed("result run identity", result["cleanRunner"]["workflowRunId"] == RUN_ID)
    passed("workflow runner identity", workflow["headSha"] == RUNNER)
    passed("workflow run identity", workflow["databaseId"] == RUN_ID)
    passed("workflow success", workflow["status"] == "completed" and workflow["conclusion"] == "success")
    passed("workflow job identity", len(workflow["jobs"]) == 1 and workflow["jobs"][0]["databaseId"] == JOB_ID)
    passed("workflow job success", workflow["jobs"][0]["conclusion"] == "success")
    passed("seven upstream checks", len(result["upstreamByteChecks"]) == 7)
    passed("compiler option count", result["compilerOptions"]["count"] == 122)
    passed(
        "compiler option digest",
        result["compilerOptions"]["keyShapeSha256"]
        == "f2fb5da0cf33ea694a8bf4ccae909a1526e7978693c15ac1a6b10b3cdfbc9d9a",
    )

    controls = strict_json(retained["negative-controls.json"], "negative controls")
    expected_controls = [
        ("one-byte-tarball-mutation", "byte-sha256-mismatch"),
        ("same-size-source-substitution", "byte-sha256-mismatch"),
        ("duplicate-tar-member", "tar-duplicate-member"),
        ("unsafe-tar-member", "unsafe-path"),
        ("oracle-mutation", "oracle-artifact-disagreement"),
        ("stale-evidence-directory", "stale-evidence"),
        ("redirected-fetch-metadata", "fetch-metadata-invalid"),
        ("forged-clean-runner-host", "host-runner-mismatch"),
        ("substituted-result-runner", "host-runner-mismatch"),
    ]
    passed("nine built-in controls", len(controls["controls"]) == 9)
    passed(
        "built-in control identities",
        [(row["id"], row["reasonCode"]) for row in controls["controls"]] == expected_controls,
    )
    passed("built-in control status", all(row["status"] == "pass" for row in controls["controls"]))

    adversarial = strict_json(
        git_blob(repository, CANDIDATE, f"{PACKET}/runner/adversarial-checks.json"),
        "adversarial checks",
    )
    expected_attacks = {
        "fetch-transport-metadata-forgery": "fetch-metadata-invalid",
        "clean-runner-host-forgery": "host-runner-mismatch",
        "result-runner-substitution": "host-runner-mismatch",
    }
    passed("nine resealed scenarios", adversarial["scenarioCount"] == 9)
    passed("all resealed scenarios", all(row["status"] == "pass" for row in adversarial["scenarios"]))
    attack_map = {row["id"]: row["actualReasonCode"] for row in adversarial["scenarios"]}
    passed("closure attack reasons", all(attack_map.get(key) == value for key, value in expected_attacks.items()))

    artifacts = strict_json(
        git_blob(repository, CANDIDATE, f"{PACKET}/runner/workflow-artifacts.json"),
        "workflow artifacts",
    )
    artifact = artifacts["artifacts"][0]
    passed("one workflow artifact", artifacts["total_count"] == 1 and len(artifacts["artifacts"]) == 1)
    passed("artifact ID", artifact["id"] == ARTIFACT_ID)
    passed("artifact API digest", artifact["digest"] == "sha256:" + ARTIFACT_SHA256)
    passed("artifact workflow head", artifact["workflow_run"]["head_sha"] == RUNNER)
    if arguments.artifact_zip is not None:
        artifact_bytes = arguments.artifact_zip.resolve().read_bytes()
        passed("detached artifact size", len(artifact_bytes) == ARTIFACT_SIZE)
        passed("detached artifact digest", sha256(artifact_bytes) == ARTIFACT_SHA256)

    range_paths = git(repository, "diff", "--name-only", f"{GATE_BASE}..{CANDIDATE}").decode().splitlines()
    passed("closure range is probe-only", bool(range_paths) and all(path.startswith(PACKET + "/") for path in range_paths))
    passed(
        "temporary workflow absent",
        git(repository, "ls-tree", "--name-only", CANDIDATE, ".github/workflows/phase0-pinned-provenance.yml") == b"",
    )
    ancestry = subprocess.run(["git", "merge-base", "--is-ancestor", RUNNER, CANDIDATE], cwd=repository)
    passed("runner ancestry", ancestry.returncode == 0)
    passed(
        "runner workflow exact copy",
        git_blob(repository, RUNNER, ".github/workflows/phase0-pinned-provenance.yml")
        == git_blob(repository, CANDIDATE, f"{PACKET}/runner/workflow.yml"),
    )

    packet_verifier = repository / PACKET / "runner/verify_packet.py"
    completed = subprocess.run(
        [sys.executable, str(packet_verifier), "--repository-root", str(repository)],
        cwd=repository,
        check=False,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
    )
    passed("packet verifier consistency", completed.returncode == 0)

    output = {
        "schemaVersion": "lumin-new-provenance-01-author-preflight.v1",
        "status": "pass",
        "candidate": CANDIDATE,
        "checkCount": len(checks),
        "checks": checks,
        "packetVerifier": json.loads(completed.stdout),
        "independenceBoundary": "Author preflight is not independent PASS evidence.",
    }
    print(json.dumps(output, indent=2, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
