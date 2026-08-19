#!/usr/bin/env python3
"""Fail-closed CI change classification and Markdown policy checks."""

from __future__ import annotations

import argparse
import os
from pathlib import Path, PurePosixPath
import re
import subprocess
import sys
from typing import Iterable, Mapping, Sequence
from urllib.parse import unquote, urlsplit


EVIDENCE_COMPONENT = b"/evidence/"
EVIDENCE_PREFIX = b"reviews/probes/"
MARKDOWN_SUFFIX = b".md"


class PolicyError(RuntimeError):
    pass


def _git(root: Path, *arguments: str) -> bytes:
    completed = subprocess.run(
        ("git", "-C", str(root), *arguments),
        check=False,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    if completed.returncode != 0:
        detail = completed.stderr.decode("utf-8", "replace").strip()
        raise PolicyError(f"git {' '.join(arguments)} failed: {detail}")
    return completed.stdout


def repository_root() -> Path:
    raw = _git(Path.cwd(), "rev-parse", "--show-toplevel").rstrip(b"\r\n")
    return Path(os.fsdecode(raw)).resolve()


def changed_paths(root: Path, base: str, head: str) -> tuple[bytes, ...]:
    raw = _git(
        root,
        "diff",
        "--no-renames",
        "--name-only",
        "-z",
        "--diff-filter=ACDMRTUXB",
        base,
        head,
        "--",
    )
    return tuple(path for path in raw.split(b"\0") if path)


def change_scope(paths: Iterable[bytes]) -> str:
    materialized = tuple(paths)
    if not materialized:
        return "full"
    if all(path.lower().endswith(MARKDOWN_SUFFIX) for path in materialized):
        return "documentation"
    return "full"


def _tracked_documents(root: Path) -> tuple[Path, ...]:
    raw = _git(root, "ls-files", "-z")
    documents: list[Path] = []
    for encoded in raw.split(b"\0"):
        if not encoded or not encoded.lower().endswith(MARKDOWN_SUFFIX):
            continue
        portable = encoded.replace(b"\\", b"/")
        if portable.startswith(EVIDENCE_PREFIX) and EVIDENCE_COMPONENT in portable:
            continue
        documents.append(Path(os.fsdecode(encoded)))
    return tuple(documents)


def inline_link_destinations(text: str) -> tuple[str, ...]:
    destinations: list[str] = []
    cursor = 0
    while True:
        marker = text.find("](", cursor)
        if marker < 0:
            break
        start = marker + 2
        depth = 1
        escaped = False
        index = start
        while index < len(text):
            character = text[index]
            if escaped:
                escaped = False
            elif character == "\\":
                escaped = True
            elif character == "(":
                depth += 1
            elif character == ")":
                depth -= 1
                if depth == 0:
                    destinations.append(text[start:index])
                    cursor = index + 1
                    break
            index += 1
        else:
            cursor = start
    return tuple(destinations)


def _destination_path(raw: str) -> str | None:
    value = raw.strip()
    if value.startswith("<"):
        closing = value.find(">")
        if closing < 0:
            return None
        value = value[1:closing]
    else:
        value = re.split(r"\s+", value, maxsplit=1)[0]
    value = re.sub(r"\\([\\() ])", r"\1", value)
    if not value or value.startswith("#") or value.startswith("//"):
        return None
    parsed = urlsplit(value)
    if parsed.scheme:
        return None
    return unquote(parsed.path) or None


def document_errors(root: Path, documents: Iterable[Path]) -> tuple[str, ...]:
    canonical_root = root.resolve()
    errors: list[str] = []
    for relative in documents:
        document = root / relative
        try:
            text = document.read_text(encoding="utf-8")
        except (OSError, UnicodeError) as error:
            errors.append(f"{relative.as_posix()}: cannot read UTF-8 Markdown: {error}")
            continue
        if text.startswith("\ufeff"):
            errors.append(f"{relative.as_posix()}: UTF-8 BOM is not allowed")
        for raw_destination in inline_link_destinations(text):
            destination = _destination_path(raw_destination)
            if destination is None:
                continue
            parts = PurePosixPath(destination).parts
            candidate = (document.parent.joinpath(*parts)).resolve()
            try:
                candidate.relative_to(canonical_root)
            except ValueError:
                errors.append(
                    f"{relative.as_posix()}: local link escapes repository: {raw_destination}"
                )
                continue
            if not candidate.exists():
                errors.append(
                    f"{relative.as_posix()}: local link target is missing: {raw_destination}"
                )
    return tuple(errors)


def check_documentation(root: Path, base: str, head: str) -> int:
    completed = subprocess.run(
        ("git", "-C", str(root), "diff", "--check", base, head, "--"),
        check=False,
    )
    if completed.returncode != 0:
        return completed.returncode
    documents = _tracked_documents(root)
    errors = document_errors(root, documents)
    if errors:
        for error in errors:
            print(f"[ci-policy] {error}", file=sys.stderr)
        return 1
    print(f"[ci-policy] documentation PASS ({len(documents)} live Markdown files)")
    return 0


def _required_environment(environment: Mapping[str, str], name: str) -> str:
    value = environment.get(name, "")
    if not value:
        raise PolicyError(f"required environment variable is missing: {name}")
    return value


def run_github_policy(root: Path, environment: Mapping[str, str]) -> int:
    event = _required_environment(environment, "GITHUB_EVENT_NAME")
    if event == "pull_request":
        base = _required_environment(environment, "PR_BASE_SHA")
        head = _required_environment(environment, "PR_HEAD_SHA")
        scope = change_scope(changed_paths(root, base, head))
    elif event == "workflow_dispatch":
        head = _required_environment(environment, "CURRENT_SHA")
        base = _git(root, "rev-parse", f"{head}^").decode("ascii").strip()
        scope = "full"
    else:
        raise PolicyError(f"unsupported GitHub event: {event}")
    result = check_documentation(root, base, head)
    if result != 0:
        return result
    output_path = Path(_required_environment(environment, "GITHUB_OUTPUT"))
    if not output_path.is_absolute() or not output_path.parent.is_dir():
        raise PolicyError("GITHUB_OUTPUT must name a file below an existing absolute parent")
    run_full = "true" if scope == "full" else "false"
    with output_path.open("a", encoding="utf-8", newline="\n") as output:
        output.write(f"scope={scope}\nrun_full={run_full}\n")
    return 0


def _parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser()
    subcommands = parser.add_subparsers(dest="command", required=True)
    subcommands.add_parser("github")
    for name in ("classify", "check-documentation"):
        command = subcommands.add_parser(name)
        command.add_argument("--base", required=True)
        command.add_argument("--head", required=True)
    return parser


def main(arguments: Sequence[str] | None = None) -> int:
    try:
        options = _parser().parse_args(arguments)
        root = repository_root()
        if options.command == "github":
            return run_github_policy(root, os.environ)
        if options.command == "classify":
            print(change_scope(changed_paths(root, options.base, options.head)))
            return 0
        return check_documentation(root, options.base, options.head)
    except PolicyError as error:
        print(f"[ci-policy] {error}", file=sys.stderr)
        return 2
    except KeyboardInterrupt:
        return 130


if __name__ == "__main__":
    raise SystemExit(main())
