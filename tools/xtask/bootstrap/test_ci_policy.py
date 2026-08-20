#!/usr/bin/env python3
"""Focused tests for the fail-closed CI policy."""

from __future__ import annotations

import importlib.util
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest


sys.dont_write_bytecode = True
SCRIPT = Path(__file__).with_name("ci_policy.py").resolve()
SPEC = importlib.util.spec_from_file_location("lumin_ci_policy", SCRIPT)
if SPEC is None or SPEC.loader is None:
    raise RuntimeError(f"cannot load {SCRIPT}")
POLICY = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = POLICY
SPEC.loader.exec_module(POLICY)


class ChangeScopeTests(unittest.TestCase):
    def test_only_markdown_is_documentation_scope(self) -> None:
        self.assertEqual(
            POLICY.change_scope((b"README.md", b"docs/native-\xff.MD")),
            "documentation",
        )

    def test_code_consumed_markdown_is_full_scope(self) -> None:
        owner = b"specs/001-foundation-slice.md"
        self.assertEqual(POLICY.change_scope((owner,)), "full")
        self.assertEqual(POLICY.change_scope((b"README.md", owner)), "full")

    def test_empty_code_and_mixed_changes_are_full_scope(self) -> None:
        self.assertEqual(POLICY.change_scope(()), "full")
        self.assertEqual(POLICY.change_scope((b"src/lib.rs",)), "full")
        self.assertEqual(
            POLICY.change_scope((b"README.md", b".github/workflows/ci.yml")),
            "full",
        )

    def test_code_to_markdown_rename_keeps_the_deleted_code_path(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            subprocess.run(("git", "init", "-q", str(root)), check=True)
            subprocess.run(
                ("git", "-C", str(root), "config", "user.email", "ci@example.invalid"),
                check=True,
            )
            subprocess.run(
                ("git", "-C", str(root), "config", "user.name", "CI Policy"),
                check=True,
            )
            source = root / "policy.py"
            source.write_text("value = 1\n", encoding="utf-8")
            subprocess.run(("git", "-C", str(root), "add", "policy.py"), check=True)
            subprocess.run(
                ("git", "-C", str(root), "commit", "-q", "-m", "base"), check=True
            )
            base = subprocess.check_output(
                ("git", "-C", str(root), "rev-parse", "HEAD"), text=True
            ).strip()
            source.rename(root / "policy.md")
            subprocess.run(("git", "-C", str(root), "add", "-A"), check=True)
            subprocess.run(
                ("git", "-C", str(root), "commit", "-q", "-m", "rename"), check=True
            )
            head = subprocess.check_output(
                ("git", "-C", str(root), "rev-parse", "HEAD"), text=True
            ).strip()
            paths = POLICY.changed_paths(root, base, head)
            self.assertEqual(paths, (b"policy.md", b"policy.py"))
            self.assertEqual(POLICY.change_scope(paths), "full")

    def test_github_policy_writes_only_validated_documentation_scope(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            subprocess.run(("git", "init", "-q", str(root)), check=True)
            subprocess.run(
                ("git", "-C", str(root), "config", "user.email", "ci@example.invalid"),
                check=True,
            )
            subprocess.run(
                ("git", "-C", str(root), "config", "user.name", "CI Policy"),
                check=True,
            )
            document = root / "README.md"
            document.write_text("base\n", encoding="utf-8")
            subprocess.run(("git", "-C", str(root), "add", "README.md"), check=True)
            subprocess.run(
                ("git", "-C", str(root), "commit", "-q", "-m", "base"), check=True
            )
            base = subprocess.check_output(
                ("git", "-C", str(root), "rev-parse", "HEAD"), text=True
            ).strip()
            document.write_text("updated\n", encoding="utf-8")
            subprocess.run(("git", "-C", str(root), "commit", "-qam", "docs"), check=True)
            head = subprocess.check_output(
                ("git", "-C", str(root), "rev-parse", "HEAD"), text=True
            ).strip()
            output = root / "github-output"
            result = POLICY.run_github_policy(
                root,
                {
                    "GITHUB_EVENT_NAME": "pull_request",
                    "PR_BASE_SHA": base,
                    "PR_HEAD_SHA": head,
                    "GITHUB_OUTPUT": str(output),
                },
            )
            self.assertEqual(result, 0)
            self.assertEqual(output.read_text(encoding="utf-8"), "scope=documentation\nrun_full=false\n")

            dispatch_output = root / "github-output-dispatch"
            result = POLICY.run_github_policy(
                root,
                {
                    "GITHUB_EVENT_NAME": "workflow_dispatch",
                    "CURRENT_SHA": head,
                    "GITHUB_OUTPUT": str(dispatch_output),
                },
            )
            self.assertEqual(result, 0)
            self.assertEqual(
                dispatch_output.read_text(encoding="utf-8"),
                "scope=full\nrun_full=true\n",
            )

    def test_github_policy_runs_full_for_code_consumed_markdown(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            subprocess.run(("git", "init", "-q", str(root)), check=True)
            subprocess.run(
                ("git", "-C", str(root), "config", "user.email", "ci@example.invalid"),
                check=True,
            )
            subprocess.run(
                ("git", "-C", str(root), "config", "user.name", "CI Policy"),
                check=True,
            )
            specs = root / "specs"
            specs.mkdir()
            owner = specs / "001-foundation-slice.md"
            owner.write_text("base\n", encoding="utf-8")
            subprocess.run(("git", "-C", str(root), "add", "specs"), check=True)
            subprocess.run(
                ("git", "-C", str(root), "commit", "-q", "-m", "base"), check=True
            )
            base = subprocess.check_output(
                ("git", "-C", str(root), "rev-parse", "HEAD"), text=True
            ).strip()
            owner.write_text("changed\n", encoding="utf-8")
            subprocess.run(("git", "-C", str(root), "commit", "-qam", "spec"), check=True)
            head = subprocess.check_output(
                ("git", "-C", str(root), "rev-parse", "HEAD"), text=True
            ).strip()
            output = root / "github-output"
            result = POLICY.run_github_policy(
                root,
                {
                    "GITHUB_EVENT_NAME": "pull_request",
                    "PR_BASE_SHA": base,
                    "PR_HEAD_SHA": head,
                    "GITHUB_OUTPUT": str(output),
                },
            )
            self.assertEqual(result, 0)
            self.assertEqual(
                output.read_text(encoding="utf-8"),
                "scope=full\nrun_full=true\n",
            )

    def test_unknown_github_event_fails_closed(self) -> None:
        with tempfile.TemporaryDirectory() as temporary, self.assertRaises(POLICY.PolicyError):
            POLICY.run_github_policy(
                Path(temporary),
                {"GITHUB_EVENT_NAME": "push", "GITHUB_OUTPUT": str(Path(temporary) / "out")},
            )


class DocumentationTests(unittest.TestCase):
    def test_parentheses_in_local_link_paths_are_preserved(self) -> None:
        self.assertEqual(
            POLICY.inline_link_destinations(
                "[contract](\uBB38\uC11C(\uD55C\uAE00)/AGENTS.ko.md)"
            ),
            ("\uBB38\uC11C(\uD55C\uAE00)/AGENTS.ko.md",),
        )

    def test_valid_links_and_external_links_pass(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            docs = root / "docs"
            docs.mkdir()
            (docs / "target.md").write_text("target\n", encoding="utf-8")
            (root / "README.md").write_text(
                "[local](docs/target.md#section) [web](https://example.com)\n",
                encoding="utf-8",
            )
            self.assertEqual(POLICY.document_errors(root, (Path("README.md"),)), ())

    def test_reference_definition_targets_are_validated(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            docs = root / "docs"
            docs.mkdir()
            (docs / "target.md").write_text("target\n", encoding="utf-8")
            (root / "README.md").write_text(
                "\n".join(
                    (
                        "[contract][owner] [continued][next] [web][external]",
                        '[owner]: <docs/target.md#section> "Owner"',
                        "[next]:",
                        "  docs/target.md 'Continued'",
                        "[external]: https://example.com/spec",
                        "[missing]: missing.md \"Missing\"",
                    )
                )
                + "\n",
                encoding="utf-8",
            )
            errors = POLICY.document_errors(root, (Path("README.md"),))
            self.assertEqual(len(errors), 1)
            self.assertIn("target is missing: missing.md", errors[0])

    def test_missing_and_escaping_links_fail(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            (root / "README.md").write_text(
                "[missing](missing.md) [escape](../outside.md)\n",
                encoding="utf-8",
            )
            errors = POLICY.document_errors(root, (Path("README.md"),))
            self.assertEqual(len(errors), 2)
            self.assertIn("target is missing", errors[0])
            self.assertIn("escapes repository", errors[1])


if __name__ == "__main__":
    unittest.main()
