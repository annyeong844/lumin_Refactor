#!/usr/bin/env python3
from __future__ import annotations

import argparse
import base64
import copy
import datetime as dt
import hashlib
import io
import json
import os
import platform
import re
import shutil
import ssl
import subprocess
import sys
import tarfile
import tempfile
import urllib.error
import urllib.request
import uuid
from pathlib import Path, PurePosixPath
from typing import Any, Callable


SCHEMA = "lumin-phase0-pinned-upstream-provenance-result.v1"
ORACLE_SCHEMA = "lumin-phase0-pinned-upstream-provenance-oracle.v1"
FROZEN_COMMIT = "9a0dbe5c89463892c001e864c4f18eeab9e0eaed"
FROZEN_MANIFEST_SHA256 = (
    "e2ca379a8a659f2febbc4e277c89db67bb02035a6b10467cf78a5663f21cd99a"
)
FROZEN_PATHS = (
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
INVENTORY_PATH = "specs/inventory-config-semantics.v1.json"
INVENTORY_SHA256 = "ebca37c3b33f8e4d92ea29e0bcdc51b7cd5ea04a453c4c469a89072f3d2fac02"
RESOLVER_PATH = "specs/resolver-config-semantics.v1.json"
RESOLVER_SHA256 = "41ffa3dcc108e74dca351b4f3a5fa182090e1481ed6d8333235f38f0459a29a1"
MAX_RESPONSE_BYTES = 32 * 1024 * 1024
HEX_64 = re.compile(r"^[0-9a-f]{64}$")

SOURCE_ROOT = Path(__file__).resolve().parent
PACKET_ROOT = SOURCE_ROOT.parent
ORACLE_PATH = SOURCE_ROOT / "oracle.json"
SOURCE_MANIFEST_PATH = SOURCE_ROOT / "SHA256SUMS"
EXTRACTOR_PATH = SOURCE_ROOT / "extract_compiler_options.cjs"
EXPECTED_SOURCE_FILES = {
    "PROBE-CONTRACT.md",
    "extract_compiler_options.cjs",
    "oracle.json",
    "verify_provenance.py",
}


class ProbeError(RuntimeError):
    def __init__(self, code: str, detail: str):
        super().__init__(detail)
        self.code = code
        self.detail = detail


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):  # noqa: ANN001
        return None


def fail(code: str, detail: str) -> None:
    raise ProbeError(code, detail)


def require(condition: bool, code: str, detail: str) -> None:
    if not condition:
        fail(code, detail)


def sha256_bytes(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def json_bytes(value: Any) -> bytes:
    return (json.dumps(value, ensure_ascii=False, indent=2, sort_keys=True) + "\n").encode(
        "utf-8"
    )


def reject_duplicate_pairs(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
    result: dict[str, Any] = {}
    for key, value in pairs:
        if key in result:
            fail("duplicate-json-key", f"duplicate JSON key: {key}")
        result[key] = value
    return result


def strict_json(data: bytes, label: str) -> Any:
    try:
        return json.loads(data.decode("utf-8"), object_pairs_hook=reject_duplicate_pairs)
    except ProbeError:
        raise
    except (UnicodeDecodeError, json.JSONDecodeError) as exc:
        fail("invalid-json", f"{label}: {exc}")


def safe_relative_path(value: str) -> PurePosixPath:
    require(value != "", "unsafe-path", "empty path")
    require("\\" not in value, "unsafe-path", f"backslash in path: {value}")
    path = PurePosixPath(value)
    require(not path.is_absolute(), "unsafe-path", f"absolute path: {value}")
    require(
        all(part not in ("", ".", "..") for part in path.parts),
        "unsafe-path",
        f"non-canonical path: {value}",
    )
    return path


def ordinal(values: list[str] | tuple[str, ...] | set[str]) -> list[str]:
    return sorted(values, key=lambda value: value.encode("utf-8"))


def render_manifest(entries: dict[str, bytes]) -> bytes:
    return "".join(
        f"{sha256_bytes(entries[path])}  {path}\n" for path in ordinal(set(entries))
    ).encode("utf-8")


def parse_manifest(data: bytes, label: str) -> dict[str, str]:
    try:
        text = data.decode("utf-8")
    except UnicodeDecodeError as exc:
        fail("manifest-invalid", f"{label}: {exc}")
    require(text.endswith("\n"), "manifest-invalid", f"{label}: missing final LF")
    require("\r" not in text, "manifest-invalid", f"{label}: CR is forbidden")
    entries: dict[str, str] = {}
    for line in text.splitlines():
        require(
            len(line) > 66 and line[64:66] == "  ",
            "manifest-invalid",
            f"{label}: malformed line",
        )
        digest, path = line[:64], line[66:]
        require(HEX_64.fullmatch(digest) is not None, "manifest-invalid", label)
        safe_relative_path(path)
        require(path not in entries, "manifest-duplicate", f"{label}: {path}")
        entries[path] = digest
    require(
        list(entries) == ordinal(set(entries)),
        "manifest-order",
        f"{label}: paths are not in ordinal UTF-8 order",
    )
    return entries


def run_git(repository_root: Path, arguments: list[str], *, binary: bool = False):
    completed = subprocess.run(
        ["git", *arguments],
        cwd=repository_root,
        check=False,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    if completed.returncode != 0:
        fail(
            "git-command-failed",
            f"git {' '.join(arguments)}: {completed.stderr.decode('utf-8', 'replace').strip()}",
        )
    if binary:
        return completed.stdout
    return completed.stdout.decode("utf-8").strip()


def git_object_bytes(repository_root: Path, revision: str, path: str) -> bytes:
    return run_git(repository_root, ["show", f"{revision}:{path}"], binary=True)


def verify_source_manifest() -> tuple[bytes, dict[str, str]]:
    require(SOURCE_MANIFEST_PATH.is_file(), "source-manifest-missing", str(SOURCE_MANIFEST_PATH))
    manifest_bytes = SOURCE_MANIFEST_PATH.read_bytes()
    entries = parse_manifest(manifest_bytes, "source/SHA256SUMS")
    actual_files = {
        path.relative_to(SOURCE_ROOT).as_posix()
        for path in SOURCE_ROOT.rglob("*")
        if path.is_file() and path != SOURCE_MANIFEST_PATH
    }
    require(actual_files == EXPECTED_SOURCE_FILES, "source-inventory-mismatch", repr(actual_files))
    require(set(entries) == EXPECTED_SOURCE_FILES, "source-manifest-inventory", repr(set(entries)))
    for path, expected in entries.items():
        actual_path = SOURCE_ROOT / Path(*PurePosixPath(path).parts)
        require(not actual_path.is_symlink(), "source-symlink", path)
        require(sha256_bytes(actual_path.read_bytes()) == expected, "source-hash-mismatch", path)
    return manifest_bytes, entries


def github_raw(repository: str, commit: str, path: str) -> str:
    prefix = "https://github.com/"
    require(repository.startswith(prefix), "artifact-shape", repository)
    slug = repository[len(prefix) :].removesuffix(".git")
    require(slug.count("/") == 1, "artifact-shape", repository)
    return f"https://raw.githubusercontent.com/{slug}/{commit}/{path}"


def compiler_option_rows(resolver: dict[str, Any]) -> bytes:
    options = resolver.get("compilerOptions")
    require(isinstance(options, dict), "artifact-shape", "resolver compilerOptions")
    rows = bytearray()
    allowed_shapes = {"boolean", "enum", "list", "number", "object", "string"}
    for name in ordinal(set(options)):
        entry = options[name]
        require(isinstance(entry, dict), "artifact-shape", f"compiler option {name}")
        shape = entry.get("shape")
        require(shape in allowed_shapes, "artifact-shape", f"compiler option {name}")
        rows.extend(f"{name}\t{shape}\n".encode("utf-8"))
    return bytes(rows)


def projected_oracle(resolver_bytes: bytes, inventory_bytes: bytes) -> dict[str, Any]:
    require(sha256_bytes(resolver_bytes) == RESOLVER_SHA256, "owner-hash-mismatch", RESOLVER_PATH)
    require(
        sha256_bytes(inventory_bytes) == INVENTORY_SHA256,
        "owner-hash-mismatch",
        INVENTORY_PATH,
    )
    resolver = strict_json(resolver_bytes, RESOLVER_PATH)
    inventory = strict_json(inventory_bytes, INVENTORY_PATH)
    require(isinstance(resolver, dict) and isinstance(inventory, dict), "artifact-shape", "root")

    ts = resolver.get("typeScriptBaseline")
    node = resolver.get("nodePackageBaseline")
    package = inventory.get("packageBaseline")
    pnpm = inventory.get("pnpmWorkspaceBaseline")
    require(all(isinstance(value, dict) for value in (ts, node, package, pnpm)), "artifact-shape", "baseline")
    require(
        node["nodeCommit"] == package["nodeCommit"]
        and node["nodeTag"] == package["nodeTag"]
        and node["packagesDocument"] == package["packagesDocument"]
        and node["packagesDocumentSha256"] == package["packagesDocumentSha256"],
        "owner-artifact-disagreement",
        "Node package baseline",
    )

    rows = compiler_option_rows(resolver)
    require(len(resolver["compilerOptions"]) == ts["compilerOptionCount"], "artifact-digest", "option count")
    require(
        sha256_bytes(rows) == ts["compilerOptionKeyShapeSha256"],
        "artifact-digest",
        "compiler option rows",
    )

    return {
        "schemaVersion": ORACLE_SCHEMA,
        "frozenArchitecture": {
            "commit": FROZEN_COMMIT,
            "manifestSha256": FROZEN_MANIFEST_SHA256,
            "manifestPaths": list(FROZEN_PATHS),
        },
        "authorityArtifacts": {
            "inventory": {"path": INVENTORY_PATH, "sha256": INVENTORY_SHA256},
            "resolver": {"path": RESOLVER_PATH, "sha256": RESOLVER_SHA256},
        },
        "typeScript": {
            "package": ts["package"],
            "sourceRepository": ts["sourceRepository"],
            "npmTarball": ts["npmTarball"],
            "npmIntegrity": ts["npmIntegrity"],
            "tarballSha256": ts["tarballSha256"],
            "typescriptJsMember": "package/lib/typescript.js",
            "typescriptJsSha256": ts["typescriptJsSha256"],
            "packageJsonMember": "package/package.json",
            "sourceCommit": ts["sourceTagCommit"],
            "moduleResolver": {
                "path": ts["moduleResolverSource"],
                "url": github_raw(
                    ts["sourceRepository"], ts["sourceTagCommit"], ts["moduleResolverSource"]
                ),
                "sha256": ts["moduleResolverSourceSha256"],
            },
            "configParser": {
                "path": ts["configParserSource"],
                "url": github_raw(
                    ts["sourceRepository"], ts["sourceTagCommit"], ts["configParserSource"]
                ),
                "sha256": ts["configParserSourceSha256"],
            },
            "compilerOptions": {
                "count": ts["compilerOptionCount"],
                "keyShapeSha256": ts["compilerOptionKeyShapeSha256"],
            },
        },
        "node": {
            "sourceRepository": node["sourceRepository"],
            "tag": node["nodeTag"],
            "commit": node["nodeCommit"],
            "tagRefApi": "https://api.github.com/repos/nodejs/node/git/ref/tags/"
            + node["nodeTag"],
            "packagesDocument": {
                "path": node["packagesDocument"],
                "url": github_raw(
                    node["sourceRepository"], node["nodeCommit"], node["packagesDocument"]
                ),
                "sha256": node["packagesDocumentSha256"],
            },
            "resolverSource": {
                "path": node["resolverSource"],
                "url": github_raw(
                    node["sourceRepository"], node["nodeCommit"], node["resolverSource"]
                ),
                "sha256": node["resolverSourceSha256"],
            },
        },
        "pnpm": {
            "sourceRepository": pnpm["repository"],
            "commit": pnpm["commit"],
            "workspaceDocument": {
                "path": pnpm["document"],
                "url": github_raw(pnpm["repository"], pnpm["commit"], pnpm["document"]),
                "sha256": pnpm["documentSha256"],
            },
        },
    }


def validate_oracle(actual: Any, expected: dict[str, Any]) -> None:
    require(actual == expected, "oracle-artifact-disagreement", "oracle is not the artifact projection")


def verify_frozen_authority(repository_root: Path) -> dict[str, Any]:
    top = Path(run_git(repository_root, ["rev-parse", "--show-toplevel"])).resolve()
    require(top == repository_root.resolve(), "repository-root-mismatch", f"{top} != {repository_root}")
    run_git(repository_root, ["cat-file", "-e", f"{FROZEN_COMMIT}^{{commit}}"])
    head = run_git(repository_root, ["rev-parse", "HEAD"])
    completed = subprocess.run(
        ["git", "merge-base", "--is-ancestor", FROZEN_COMMIT, head], cwd=repository_root
    )
    require(completed.returncode == 0, "frozen-commit-not-ancestor", head)

    frozen_entries = {
        path: git_object_bytes(repository_root, FROZEN_COMMIT, path) for path in FROZEN_PATHS
    }
    manifest = render_manifest(frozen_entries)
    require(sha256_bytes(manifest) == FROZEN_MANIFEST_SHA256, "architecture-manifest-mismatch", FROZEN_COMMIT)

    resolver_bytes = frozen_entries[RESOLVER_PATH]
    inventory_bytes = frozen_entries[INVENTORY_PATH]
    expected_oracle = projected_oracle(resolver_bytes, inventory_bytes)
    oracle_bytes = ORACLE_PATH.read_bytes()
    oracle = strict_json(oracle_bytes, "source/oracle.json")
    validate_oracle(oracle, expected_oracle)

    for path, frozen in ((RESOLVER_PATH, resolver_bytes), (INVENTORY_PATH, inventory_bytes)):
        current = (repository_root / Path(*PurePosixPath(path).parts)).read_bytes()
        require(current == frozen, "current-owner-differs", path)

    return {
        "head": head,
        "manifestBytes": manifest,
        "oracle": oracle,
        "oracleBytes": oracle_bytes,
        "resolverRows": compiler_option_rows(strict_json(resolver_bytes, RESOLVER_PATH)),
    }


def require_digest(data: bytes, expected: str, label: str) -> None:
    require(sha256_bytes(data) == expected, "byte-sha256-mismatch", label)


def fetch_url(url: str, *, api: bool = False) -> tuple[bytes, dict[str, Any]]:
    headers = {
        "Accept-Encoding": "identity",
        "Cache-Control": "no-cache",
        "User-Agent": "lumin-phase0-provenance-probe/1",
    }
    if api:
        headers["Accept"] = "application/vnd.github+json"
        headers["X-GitHub-Api-Version"] = "2022-11-28"
    request = urllib.request.Request(url, headers=headers)
    opener = urllib.request.build_opener(NoRedirect())
    try:
        with opener.open(request, timeout=90) as response:
            status = response.status
            final_url = response.geturl()
            encoding = response.headers.get("Content-Encoding")
            chunks: list[bytes] = []
            size = 0
            while True:
                chunk = response.read(1024 * 1024)
                if not chunk:
                    break
                size += len(chunk)
                require(size <= MAX_RESPONSE_BYTES, "response-too-large", url)
                chunks.append(chunk)
            data = b"".join(chunks)
            content_length = response.headers.get("Content-Length")
            metadata = {
                "url": url,
                "finalUrl": final_url,
                "status": status,
                "sizeBytes": len(data),
                "sha256": sha256_bytes(data),
                "contentType": response.headers.get("Content-Type"),
                "contentLengthHeader": content_length,
                "contentEncoding": encoding,
                "etag": response.headers.get("ETag"),
                "lastModified": response.headers.get("Last-Modified"),
                "retrievedAtUtc": dt.datetime.now(dt.timezone.utc).isoformat(),
            }
    except urllib.error.HTTPError as exc:
        fail("http-failure", f"{url}: {exc.code} {exc.reason}")
    except urllib.error.URLError as exc:
        fail("network-failure", f"{url}: {exc.reason}")

    require(status == 200, "http-status", f"{url}: {status}")
    require(final_url == url, "http-redirect", f"{url} -> {final_url}")
    require(encoding in (None, "", "identity"), "content-encoding", f"{url}: {encoding}")
    if content_length is not None:
        require(content_length.isdigit(), "content-length", url)
        require(int(content_length) == len(data), "content-length", url)
    return data, metadata


def validate_tarball(tarball: bytes, oracle: dict[str, Any]) -> dict[str, bytes]:
    ts = oracle["typeScript"]
    try:
        with tarfile.open(fileobj=io.BytesIO(tarball), mode="r:gz") as archive:
            seen: set[str] = set()
            retained: dict[str, bytes] = {}
            required = {ts["typescriptJsMember"], ts["packageJsonMember"]}
            for member in archive.getmembers():
                path = member.name
                safe_relative_path(path)
                require(path not in seen, "tar-duplicate-member", path)
                seen.add(path)
                require(member.isfile(), "tar-nonregular-member", path)
                if path in required:
                    stream = archive.extractfile(member)
                    require(stream is not None, "tar-member-unreadable", path)
                    retained[path] = stream.read()
    except ProbeError:
        raise
    except (tarfile.TarError, EOFError, OSError) as exc:
        fail("tar-invalid", str(exc))
    require(set(retained) == required, "tar-required-member-count", repr(set(retained)))
    return retained


def validate_npm_package(package_bytes: bytes, oracle: dict[str, Any]) -> dict[str, Any]:
    package = strict_json(package_bytes, "package/package.json")
    require(isinstance(package, dict), "npm-package-identity", "root")
    ts = oracle["typeScript"]
    name, version = ts["package"].rsplit("@", 1)
    expected_repository = ts["sourceRepository"] + ".git"
    require(package.get("name") == name, "npm-package-identity", "name")
    require(package.get("version") == version, "npm-package-identity", "version")
    require(package.get("gitHead") == ts["sourceCommit"], "npm-package-identity", "gitHead")
    repository = package.get("repository")
    require(isinstance(repository, dict), "npm-package-identity", "repository")
    require(repository.get("type") == "git", "npm-package-identity", "repository.type")
    require(repository.get("url") == expected_repository, "npm-package-identity", "repository.url")
    return {
        "name": name,
        "version": version,
        "gitHead": ts["sourceCommit"],
        "repository": expected_repository,
    }


def validate_integrity(data: bytes, integrity: str) -> None:
    require(integrity.startswith("sha512-"), "npm-integrity-format", integrity)
    encoded = integrity.removeprefix("sha512-")
    try:
        expected = base64.b64decode(encoded, validate=True)
    except ValueError as exc:
        fail("npm-integrity-format", str(exc))
    require(hashlib.sha512(data).digest() == expected, "npm-integrity-mismatch", integrity)


def parse_node_tag(
    ref_bytes: bytes, tag_bytes: bytes, oracle: dict[str, Any]
) -> dict[str, str]:
    node = oracle["node"]
    ref = strict_json(ref_bytes, "node-tag-ref.json")
    require(isinstance(ref, dict), "node-tag-invalid", "ref root")
    require(ref.get("ref") == "refs/tags/" + node["tag"], "node-tag-invalid", "ref")
    target = ref.get("object")
    require(isinstance(target, dict), "node-tag-invalid", "ref object")
    require(target.get("type") == "tag", "node-tag-invalid", "expected annotated tag")
    tag_object_sha = target.get("sha")
    require(isinstance(tag_object_sha, str) and len(tag_object_sha) == 40, "node-tag-invalid", "tag sha")
    expected_url = f"https://api.github.com/repos/nodejs/node/git/tags/{tag_object_sha}"
    require(target.get("url") == expected_url, "node-tag-invalid", "tag URL")

    tag = strict_json(tag_bytes, "node-tag-object.json")
    require(isinstance(tag, dict), "node-tag-invalid", "tag root")
    require(tag.get("sha") == tag_object_sha, "node-tag-invalid", "tag object sha")
    require(tag.get("tag") == node["tag"], "node-tag-invalid", "tag name")
    commit = tag.get("object")
    require(isinstance(commit, dict), "node-tag-invalid", "commit object")
    require(commit.get("type") == "commit", "node-tag-invalid", "target type")
    require(commit.get("sha") == node["commit"], "node-tag-commit-mismatch", node["commit"])
    return {
        "tag": node["tag"],
        "tagObjectSha": tag_object_sha,
        "commit": node["commit"],
    }


def run_extractor(typescript_js: Path) -> tuple[bytes, str]:
    node = shutil.which("node")
    require(node is not None, "node-unavailable", "node executable was not found")
    environment = os.environ.copy()
    environment.pop("NODE_OPTIONS", None)
    environment.pop("NODE_PATH", None)
    environment["NO_COLOR"] = "1"
    completed = subprocess.run(
        [node, str(EXTRACTOR_PATH), str(typescript_js)],
        check=False,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        timeout=90,
        env=environment,
    )
    require(completed.returncode == 0, "compiler-option-extraction", completed.stderr.decode("utf-8", "replace"))
    require(completed.stderr == b"", "compiler-option-extraction", "unexpected stderr")
    version = subprocess.run(
        [node, "--version"], check=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE
    ).stdout.decode("ascii").strip()
    return completed.stdout, version


def make_test_tar(names: list[str]) -> bytes:
    output = io.BytesIO()
    with tarfile.open(fileobj=output, mode="w:gz") as archive:
        for index, name in enumerate(names):
            data = f"test-{index}".encode("ascii")
            member = tarfile.TarInfo(name)
            member.size = len(data)
            member.mtime = 0
            archive.addfile(member, io.BytesIO(data))
    return output.getvalue()


def expected_failure(
    control_id: str, expected_code: str, operation: Callable[[], None]
) -> dict[str, str]:
    try:
        operation()
    except ProbeError as exc:
        require(
            exc.code == expected_code,
            "negative-control-wrong-reason",
            f"{control_id}: {exc.code} != {expected_code}",
        )
        return {"id": control_id, "status": "pass", "reasonCode": exc.code}
    fail("negative-control-accepted", control_id)


def ensure_fresh_evidence(path: Path) -> None:
    require(not path.exists(), "stale-evidence", str(path))


def run_negative_controls(
    oracle: dict[str, Any], expected_oracle: dict[str, Any], tarball: bytes, source: bytes
) -> list[dict[str, str]]:
    mutated_tarball = bytearray(tarball)
    mutated_tarball[len(mutated_tarball) // 2] ^= 1
    substituted = bytes([source[0] ^ 1]) + source[1:]
    duplicate_tar = make_test_tar(
        [oracle["typeScript"]["typescriptJsMember"], oracle["typeScript"]["typescriptJsMember"]]
    )
    unsafe_tar = make_test_tar(["../escape", oracle["typeScript"]["typescriptJsMember"]])
    mutated_oracle = copy.deepcopy(expected_oracle)
    mutated_oracle["node"]["commit"] = "0" * 40

    controls = [
        expected_failure(
            "one-byte-tarball-mutation",
            "byte-sha256-mismatch",
            lambda: require_digest(
                bytes(mutated_tarball), oracle["typeScript"]["tarballSha256"], "mutated tarball"
            ),
        ),
        expected_failure(
            "same-size-source-substitution",
            "byte-sha256-mismatch",
            lambda: require_digest(
                substituted, oracle["typeScript"]["moduleResolver"]["sha256"], "substituted source"
            ),
        ),
        expected_failure(
            "duplicate-tar-member",
            "tar-duplicate-member",
            lambda: validate_tarball(duplicate_tar, oracle),
        ),
        expected_failure(
            "unsafe-tar-member",
            "unsafe-path",
            lambda: validate_tarball(unsafe_tar, oracle),
        ),
        expected_failure(
            "oracle-mutation",
            "oracle-artifact-disagreement",
            lambda: validate_oracle(mutated_oracle, expected_oracle),
        ),
    ]
    with tempfile.TemporaryDirectory(prefix="lumin-provenance-stale-") as directory:
        controls.append(
            expected_failure(
                "stale-evidence-directory",
                "stale-evidence",
                lambda: ensure_fresh_evidence(Path(directory)),
            )
        )
    return controls


def write_file(root: Path, relative: str, data: bytes) -> None:
    safe_relative_path(relative)
    target = root / Path(*PurePosixPath(relative).parts)
    target.parent.mkdir(parents=True, exist_ok=True)
    require(not target.exists(), "stale-evidence", str(target))
    target.write_bytes(data)


def evidence_inventory(root: Path) -> dict[str, bytes]:
    entries: dict[str, bytes] = {}
    for path in root.rglob("*"):
        if not path.is_file() or path.name == "SHA256SUMS":
            continue
        require(not path.is_symlink(), "evidence-symlink", str(path))
        relative = path.relative_to(root).as_posix()
        safe_relative_path(relative)
        entries[relative] = path.read_bytes()
    return entries


def host_record(repository_root: Path, head: str, node_version: str) -> dict[str, Any]:
    selected_environment = {
        key: os.environ[key]
        for key in (
            "GITHUB_ACTIONS",
            "GITHUB_JOB",
            "GITHUB_REF",
            "GITHUB_REPOSITORY",
            "GITHUB_RUN_ATTEMPT",
            "GITHUB_RUN_ID",
            "GITHUB_SHA",
            "RUNNER_ARCH",
            "RUNNER_OS",
        )
        if key in os.environ
    }
    return {
        "schemaVersion": "lumin-phase0-provenance-host.v1",
        "capturedAtUtc": dt.datetime.now(dt.timezone.utc).isoformat(),
        "platform": platform.platform(),
        "system": platform.system(),
        "release": platform.release(),
        "machine": platform.machine(),
        "python": sys.version,
        "pythonExecutable": sys.executable,
        "node": node_version,
        "openssl": ssl.OPENSSL_VERSION,
        "repositoryHead": head,
        "repositoryRoot": str(repository_root.resolve()),
        "environment": selected_environment,
    }


def expected_evidence_paths() -> set[str]:
    return {
        "architecture-manifest.txt",
        "derived/compiler-options.tsv",
        "derived/typescript-package.json",
        "fetch-metadata.json",
        "host.json",
        "identity/node-tag-object.json",
        "identity/node-tag-ref.json",
        "negative-controls.json",
        "objects/node-packages.md",
        "objects/node-resolve.js",
        "objects/pnpm-workspace_yaml.md",
        "objects/typescript-6.0.0-beta.tgz",
        "objects/typescript-commandLineParser.ts",
        "objects/typescript-moduleNameResolver.ts",
        "objects/typescript.js",
        "oracle.json",
        "result.json",
        "source-manifest.txt",
    }


def verify_evidence(repository_root: Path, evidence: Path) -> dict[str, Any]:
    source_manifest_bytes, source_entries = verify_source_manifest()
    authority = verify_frozen_authority(repository_root)
    oracle = authority["oracle"]
    expected_oracle = projected_oracle(
        git_object_bytes(repository_root, FROZEN_COMMIT, RESOLVER_PATH),
        git_object_bytes(repository_root, FROZEN_COMMIT, INVENTORY_PATH),
    )

    require(evidence.is_dir(), "evidence-missing", str(evidence))
    manifest_path = evidence / "SHA256SUMS"
    require(manifest_path.is_file(), "evidence-manifest-missing", str(manifest_path))
    manifest = parse_manifest(manifest_path.read_bytes(), "evidence/SHA256SUMS")
    require(set(manifest) == expected_evidence_paths(), "evidence-inventory-mismatch", repr(set(manifest)))
    actual_inventory = evidence_inventory(evidence)
    require(set(actual_inventory) == set(manifest), "evidence-extra-or-missing", repr(set(actual_inventory)))
    for path, expected in manifest.items():
        require(sha256_bytes(actual_inventory[path]) == expected, "evidence-hash-mismatch", path)

    require(actual_inventory["oracle.json"] == authority["oracleBytes"], "evidence-oracle-mismatch", "oracle")
    require(
        actual_inventory["architecture-manifest.txt"] == authority["manifestBytes"],
        "evidence-architecture-mismatch",
        "manifest",
    )
    require(actual_inventory["source-manifest.txt"] == source_manifest_bytes, "evidence-source-mismatch", "manifest")

    tarball = actual_inventory["objects/typescript-6.0.0-beta.tgz"]
    require_digest(tarball, oracle["typeScript"]["tarballSha256"], "TypeScript tarball")
    validate_integrity(tarball, oracle["typeScript"]["npmIntegrity"])
    members = validate_tarball(tarball, oracle)
    typescript_js = members[oracle["typeScript"]["typescriptJsMember"]]
    package_json = members[oracle["typeScript"]["packageJsonMember"]]
    require_digest(typescript_js, oracle["typeScript"]["typescriptJsSha256"], "typescript.js")
    require(typescript_js == actual_inventory["objects/typescript.js"], "derived-byte-mismatch", "typescript.js")
    require(package_json == actual_inventory["derived/typescript-package.json"], "derived-byte-mismatch", "package.json")
    package_identity = validate_npm_package(package_json, oracle)

    byte_checks = [
        ("typescript-npm-tarball", "objects/typescript-6.0.0-beta.tgz", oracle["typeScript"]["tarballSha256"]),
        ("typescript-js", "objects/typescript.js", oracle["typeScript"]["typescriptJsSha256"]),
        (
            "typescript-module-resolver",
            "objects/typescript-moduleNameResolver.ts",
            oracle["typeScript"]["moduleResolver"]["sha256"],
        ),
        (
            "typescript-config-parser",
            "objects/typescript-commandLineParser.ts",
            oracle["typeScript"]["configParser"]["sha256"],
        ),
        ("node-packages-document", "objects/node-packages.md", oracle["node"]["packagesDocument"]["sha256"]),
        ("node-esm-resolver", "objects/node-resolve.js", oracle["node"]["resolverSource"]["sha256"]),
        ("pnpm-workspace-document", "objects/pnpm-workspace_yaml.md", oracle["pnpm"]["workspaceDocument"]["sha256"]),
    ]
    for check_id, path, digest in byte_checks:
        require_digest(actual_inventory[path], digest, check_id)

    node_tag = parse_node_tag(
        actual_inventory["identity/node-tag-ref.json"],
        actual_inventory["identity/node-tag-object.json"],
        oracle,
    )

    extracted_rows, node_version = run_extractor(evidence / "objects/typescript.js")
    require(extracted_rows == authority["resolverRows"], "compiler-option-rows-mismatch", "rows")
    require(extracted_rows == actual_inventory["derived/compiler-options.tsv"], "derived-byte-mismatch", "compiler options")
    require(len(extracted_rows.splitlines()) == oracle["typeScript"]["compilerOptions"]["count"], "compiler-option-count", "rows")
    require_digest(
        extracted_rows,
        oracle["typeScript"]["compilerOptions"]["keyShapeSha256"],
        "compiler option digest",
    )

    negative = strict_json(actual_inventory["negative-controls.json"], "negative-controls.json")
    require(isinstance(negative, dict), "negative-controls-invalid", "root")
    controls = negative.get("controls")
    require(isinstance(controls, list) and len(controls) == 6, "negative-controls-invalid", "count")
    require(all(row.get("status") == "pass" for row in controls if isinstance(row, dict)), "negative-controls-invalid", "status")

    fetch_metadata = strict_json(actual_inventory["fetch-metadata.json"], "fetch-metadata.json")
    require(isinstance(fetch_metadata, dict), "fetch-metadata-invalid", "root")
    require(fetch_metadata.get("schemaVersion") == "lumin-phase0-provenance-fetch.v1", "fetch-metadata-invalid", "schema")
    require(len(fetch_metadata.get("responses", [])) == 8, "fetch-metadata-invalid", "response count")

    result = strict_json(actual_inventory["result.json"], "result.json")
    require(isinstance(result, dict), "result-invalid", "root")
    require(result.get("schemaVersion") == SCHEMA and result.get("status") == "pass", "result-invalid", "status")
    require(result.get("frozenArchitecture", {}).get("commit") == FROZEN_COMMIT, "result-invalid", "architecture")
    require(result.get("sourceManifestSha256") == sha256_bytes(source_manifest_bytes), "result-invalid", "source manifest")
    require(result.get("oracleSha256") == sha256_bytes(authority["oracleBytes"]), "result-invalid", "oracle")
    require(result.get("npmPackage") == package_identity, "result-invalid", "npm package")
    require(result.get("nodeTag") == node_tag, "result-invalid", "node tag")
    require(result.get("compilerOptions", {}).get("nodeVersion") == node_version, "result-invalid", "node version")
    require(len(result.get("upstreamByteChecks", [])) == 7, "result-invalid", "byte checks")
    require(result.get("negativeControlCount") == 6, "result-invalid", "negative controls")
    runner_commit = result.get("runnerCommit")
    require(isinstance(runner_commit, str) and len(runner_commit) == 40, "result-invalid", "runner commit")
    run_git(repository_root, ["cat-file", "-e", f"{runner_commit}^{{commit}}"])
    completed = subprocess.run(
        ["git", "merge-base", "--is-ancestor", runner_commit, "HEAD"], cwd=repository_root
    )
    require(completed.returncode == 0, "runner-commit-not-ancestor", runner_commit)
    for path, digest in source_entries.items():
        git_path = f"{PACKET_ROOT.relative_to(repository_root).as_posix()}/source/{path}"
        require_digest(git_object_bytes(repository_root, runner_commit, git_path), digest, git_path)

    return {
        "status": "pass",
        "evidenceManifestSha256": sha256_bytes(manifest_path.read_bytes()),
        "evidenceEntries": len(manifest),
        "upstreamByteChecks": 7,
        "negativeControls": 6,
        "compilerOptionCount": len(extracted_rows.splitlines()),
        "compilerOptionDigest": sha256_bytes(extracted_rows),
        "runnerCommit": runner_commit,
    }


def capture(repository_root: Path, evidence: Path) -> dict[str, Any]:
    ensure_fresh_evidence(evidence)
    source_manifest_bytes, source_entries = verify_source_manifest()
    authority = verify_frozen_authority(repository_root)
    oracle = authority["oracle"]
    expected_oracle = projected_oracle(
        git_object_bytes(repository_root, FROZEN_COMMIT, RESOLVER_PATH),
        git_object_bytes(repository_root, FROZEN_COMMIT, INVENTORY_PATH),
    )
    require(run_git(repository_root, ["status", "--porcelain"]) == "", "working-tree-dirty", "capture requires a clean checkout")

    responses: list[dict[str, Any]] = []
    objects: dict[str, bytes] = {}

    def retrieve(check_id: str, url: str, path: str, expected: str) -> bytes:
        data, metadata = fetch_url(url)
        require_digest(data, expected, check_id)
        metadata["id"] = check_id
        metadata["retainedPath"] = path
        responses.append(metadata)
        objects[path] = data
        return data

    ts = oracle["typeScript"]
    tarball = retrieve(
        "typescript-npm-tarball",
        ts["npmTarball"],
        "objects/typescript-6.0.0-beta.tgz",
        ts["tarballSha256"],
    )
    validate_integrity(tarball, ts["npmIntegrity"])
    members = validate_tarball(tarball, oracle)
    typescript_js = members[ts["typescriptJsMember"]]
    package_json = members[ts["packageJsonMember"]]
    require_digest(typescript_js, ts["typescriptJsSha256"], "typescript-js")
    package_identity = validate_npm_package(package_json, oracle)
    objects["objects/typescript.js"] = typescript_js

    retrieve(
        "typescript-module-resolver",
        ts["moduleResolver"]["url"],
        "objects/typescript-moduleNameResolver.ts",
        ts["moduleResolver"]["sha256"],
    )
    retrieve(
        "typescript-config-parser",
        ts["configParser"]["url"],
        "objects/typescript-commandLineParser.ts",
        ts["configParser"]["sha256"],
    )
    retrieve(
        "node-packages-document",
        oracle["node"]["packagesDocument"]["url"],
        "objects/node-packages.md",
        oracle["node"]["packagesDocument"]["sha256"],
    )
    retrieve(
        "node-esm-resolver",
        oracle["node"]["resolverSource"]["url"],
        "objects/node-resolve.js",
        oracle["node"]["resolverSource"]["sha256"],
    )
    retrieve(
        "pnpm-workspace-document",
        oracle["pnpm"]["workspaceDocument"]["url"],
        "objects/pnpm-workspace_yaml.md",
        oracle["pnpm"]["workspaceDocument"]["sha256"],
    )

    ref_bytes, ref_metadata = fetch_url(oracle["node"]["tagRefApi"], api=True)
    ref = strict_json(ref_bytes, "node tag ref")
    target = ref.get("object") if isinstance(ref, dict) else None
    require(isinstance(target, dict) and target.get("type") == "tag", "node-tag-invalid", "ref target")
    tag_url = target.get("url")
    require(isinstance(tag_url, str), "node-tag-invalid", "tag URL")
    tag_bytes, tag_metadata = fetch_url(tag_url, api=True)
    node_tag = parse_node_tag(ref_bytes, tag_bytes, oracle)
    ref_metadata.update({"id": "node-tag-ref", "retainedPath": "identity/node-tag-ref.json"})
    tag_metadata.update({"id": "node-tag-object", "retainedPath": "identity/node-tag-object.json"})
    responses.extend((ref_metadata, tag_metadata))

    negative_controls = run_negative_controls(
        oracle,
        expected_oracle,
        tarball,
        objects["objects/typescript-moduleNameResolver.ts"],
    )

    partial = evidence.with_name(evidence.name + ".partial-" + uuid.uuid4().hex)
    ensure_fresh_evidence(partial)
    try:
        partial.mkdir(parents=True)
        for path, data in objects.items():
            write_file(partial, path, data)
        write_file(partial, "derived/typescript-package.json", package_json)
        write_file(partial, "identity/node-tag-ref.json", ref_bytes)
        write_file(partial, "identity/node-tag-object.json", tag_bytes)
        write_file(partial, "oracle.json", authority["oracleBytes"])
        write_file(partial, "architecture-manifest.txt", authority["manifestBytes"])
        write_file(partial, "source-manifest.txt", source_manifest_bytes)

        extracted_rows, node_version = run_extractor(partial / "objects/typescript.js")
        require(extracted_rows == authority["resolverRows"], "compiler-option-rows-mismatch", "rows")
        require_digest(extracted_rows, ts["compilerOptions"]["keyShapeSha256"], "compiler option digest")
        write_file(partial, "derived/compiler-options.tsv", extracted_rows)

        write_file(
            partial,
            "negative-controls.json",
            json_bytes(
                {
                    "schemaVersion": "lumin-phase0-provenance-negative-controls.v1",
                    "controls": negative_controls,
                }
            ),
        )
        write_file(
            partial,
            "fetch-metadata.json",
            json_bytes(
                {
                    "schemaVersion": "lumin-phase0-provenance-fetch.v1",
                    "responses": responses,
                }
            ),
        )
        write_file(partial, "host.json", json_bytes(host_record(repository_root, authority["head"], node_version)))

        upstream_checks = [
            {
                "id": check_id,
                "path": path,
                "sha256": digest,
                "sizeBytes": len(objects[path]),
                "status": "pass",
            }
            for check_id, path, digest in (
                ("typescript-npm-tarball", "objects/typescript-6.0.0-beta.tgz", ts["tarballSha256"]),
                ("typescript-js", "objects/typescript.js", ts["typescriptJsSha256"]),
                (
                    "typescript-module-resolver",
                    "objects/typescript-moduleNameResolver.ts",
                    ts["moduleResolver"]["sha256"],
                ),
                (
                    "typescript-config-parser",
                    "objects/typescript-commandLineParser.ts",
                    ts["configParser"]["sha256"],
                ),
                (
                    "node-packages-document",
                    "objects/node-packages.md",
                    oracle["node"]["packagesDocument"]["sha256"],
                ),
                (
                    "node-esm-resolver",
                    "objects/node-resolve.js",
                    oracle["node"]["resolverSource"]["sha256"],
                ),
                (
                    "pnpm-workspace-document",
                    "objects/pnpm-workspace_yaml.md",
                    oracle["pnpm"]["workspaceDocument"]["sha256"],
                ),
            )
        ]
        result = {
            "schemaVersion": SCHEMA,
            "status": "pass",
            "claim": "clean-pinned-upstream-provenance",
            "claimBoundary": "frozen upstream identity only; no product implementation, package, behavior, or budget claim",
            "runnerCommit": authority["head"],
            "frozenArchitecture": {
                "commit": FROZEN_COMMIT,
                "manifestSha256": FROZEN_MANIFEST_SHA256,
            },
            "authorityArtifacts": oracle["authorityArtifacts"],
            "sourceManifestSha256": sha256_bytes(source_manifest_bytes),
            "sourceManifestEntries": len(source_entries),
            "oracleSha256": sha256_bytes(authority["oracleBytes"]),
            "npmPackage": package_identity,
            "nodeTag": node_tag,
            "compilerOptions": {
                "count": len(extracted_rows.splitlines()),
                "keyShapeSha256": sha256_bytes(extracted_rows),
                "nodeVersion": node_version,
            },
            "upstreamByteChecks": upstream_checks,
            "negativeControlCount": len(negative_controls),
        }
        write_file(partial, "result.json", json_bytes(result))
        manifest_bytes = render_manifest(evidence_inventory(partial))
        require(set(parse_manifest(manifest_bytes, "generated evidence manifest")) == expected_evidence_paths(), "evidence-inventory-mismatch", "generated")
        (partial / "SHA256SUMS").write_bytes(manifest_bytes)

        verification = verify_evidence(repository_root, partial)
        os.replace(partial, evidence)
        return verification
    except Exception:
        if partial.exists():
            shutil.rmtree(partial)
        raise


def parse_arguments() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="Verify frozen upstream provenance bytes")
    parser.add_argument("command", choices=("capture", "verify"))
    parser.add_argument("--repository-root", required=True, type=Path)
    parser.add_argument("--evidence", required=True, type=Path)
    return parser.parse_args()


def main() -> int:
    arguments = parse_arguments()
    repository_root = arguments.repository_root.resolve()
    evidence = arguments.evidence.resolve()
    try:
        if arguments.command == "capture":
            result = capture(repository_root, evidence)
        else:
            result = verify_evidence(repository_root, evidence)
    except ProbeError as exc:
        print(
            json.dumps(
                {"status": "fail", "reasonCode": exc.code, "detail": exc.detail},
                ensure_ascii=False,
                sort_keys=True,
            ),
            file=sys.stderr,
        )
        return 1
    print(json.dumps(result, ensure_ascii=False, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
