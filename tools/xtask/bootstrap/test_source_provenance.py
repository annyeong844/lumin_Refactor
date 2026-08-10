"""Isolated stdlib tests for the Cargo source-provenance bootstrap."""

from __future__ import annotations

import importlib.util
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest


SCRIPT = Path(__file__).with_name("source_provenance.py").resolve()
SPEC = importlib.util.spec_from_file_location("lumin_source_provenance", SCRIPT)
if SPEC is None or SPEC.loader is None:
    raise RuntimeError(f"cannot load {SCRIPT}")
PROVENANCE = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = PROVENANCE
SPEC.loader.exec_module(PROVENANCE)


class Fixture:
    def __init__(self, base: Path) -> None:
        self.root = base / "repo"
        self.member = self.root / "member"
        self.cargo_home = base / "cargo-home"
        self.member.mkdir(parents=True)
        self.cargo_home.mkdir()
        self.write_root()
        (self.member / "Cargo.toml").write_text(
            '[package]\nname = "fixture-member"\nversion = "0.0.0"\nedition = "2024"\n',
            encoding="utf-8",
        )

    def write_root(self, *, resolver: str = "3", suffix: str = "") -> None:
        (self.root / "Cargo.toml").write_text(
            f'[workspace]\nresolver = "{resolver}"\nmembers = ["member"]\n{suffix}',
            encoding="utf-8",
        )

    def validate(self, command: tuple[str, ...] = ("cargo", "--version"), **environment: str) -> None:
        PROVENANCE.validate_invocation(
            self.root,
            command,
            environment,
            self.root,
            self.cargo_home,
        )


class SourceProvenanceTests(unittest.TestCase):
    def fixture(self) -> tuple[tempfile.TemporaryDirectory[str], Fixture]:
        temporary = tempfile.TemporaryDirectory()
        return temporary, Fixture(Path(temporary.name))

    def test_clean_explicit_workspace_is_accepted(self) -> None:
        temporary, fixture = self.fixture()
        with temporary:
            fixture.validate()

    def test_both_command_line_config_forms_are_rejected(self) -> None:
        temporary, fixture = self.fixture()
        with temporary:
            for command in (
                ("cargo", "--config", "source.crates-io.replace-with=vendored", "metadata"),
                ("cargo", "--config=source.crates-io.replace-with=vendored", "metadata"),
            ):
                with self.subTest(command=command):
                    with self.assertRaisesRegex(PROVENANCE.ProvenanceError, "configuration argument"):
                        fixture.validate(command)

    def test_source_environment_is_rejected_without_mutating_process_state(self) -> None:
        temporary, fixture = self.fixture()
        with temporary:
            for name in (
                "CARGO_SOURCE_CRATES_IO_REPLACE_WITH",
                "CARGO_PATHS",
                "CARGO_REGISTRIES_CRATES_IO_INDEX",
            ):
                with self.subTest(name=name):
                    with self.assertRaisesRegex(PROVENANCE.ProvenanceError, name):
                        fixture.validate(**{name: "replacement"})

    def test_repository_and_cargo_home_config_files_are_rejected(self) -> None:
        temporary, fixture = self.fixture()
        with temporary:
            candidates = (
                fixture.root / ".cargo" / "config.toml",
                fixture.member / ".cargo" / "config.toml",
                fixture.root.parent / ".cargo" / "config",
                fixture.cargo_home / "config",
            )
            for candidate in candidates:
                with self.subTest(candidate=candidate):
                    candidate.parent.mkdir(parents=True, exist_ok=True)
                    candidate.write_text("[source.crates-io]\nreplace-with = 'vendored'\n", encoding="utf-8")
                    with self.assertRaisesRegex(PROVENANCE.ProvenanceError, "configuration"):
                        fixture.validate()
                    candidate.unlink()

    def test_repository_owned_cargo_home_is_rejected(self) -> None:
        temporary, fixture = self.fixture()
        with temporary:
            with self.assertRaisesRegex(PROVENANCE.ProvenanceError, "Cargo home"):
                PROVENANCE.validate_invocation(
                    fixture.root,
                    ("cargo", "--version"),
                    {},
                    fixture.root,
                    fixture.root / "cargo-home",
                )

    def test_empty_cargo_home_environment_is_rejected(self) -> None:
        temporary, fixture = self.fixture()
        with temporary:
            with self.assertRaisesRegex(PROVENANCE.ProvenanceError, "must not be empty"):
                PROVENANCE.active_cargo_home({"CARGO_HOME": ""}, fixture.root)

    def test_patch_replace_and_resolver_drift_are_rejected(self) -> None:
        temporary, fixture = self.fixture()
        with temporary:
            fixture.write_root(suffix='\n[patch.crates-io]\nserde = { path = "vendor/serde" }\n')
            with self.assertRaisesRegex(PROVENANCE.ProvenanceError, r"\[patch\]"):
                fixture.validate()
            fixture.write_root(resolver="1")
            with self.assertRaisesRegex(PROVENANCE.ProvenanceError, "resolver"):
                fixture.validate()

    def test_malformed_zero_member_and_escape_inputs_fail_closed(self) -> None:
        temporary, fixture = self.fixture()
        with temporary:
            cases = (
                '[workspace\nresolver = "3"\n',
                '[workspace]\nresolver = "3"\nmembers = []\n',
                '[workspace]\nresolver = "3"\nmembers = ["../outside"]\n',
            )
            for source in cases:
                with self.subTest(source=source):
                    (fixture.root / "Cargo.toml").write_text(source, encoding="utf-8")
                    with self.assertRaises(PROVENANCE.ProvenanceError):
                        fixture.validate()

    def test_real_entrypoint_requires_isolated_mode_and_runs_exact_cargo(self) -> None:
        root = SCRIPT.parents[3]
        completed = subprocess.run(
            [sys.executable, "-I", "-S", str(SCRIPT), "--", "cargo", "--version"],
            cwd=root,
            env=os.environ.copy(),
            check=False,
            capture_output=True,
            text=True,
        )
        self.assertEqual(completed.returncode, 0, completed.stderr)
        self.assertTrue(completed.stdout.startswith("cargo "), completed.stdout)


if __name__ == "__main__":
    unittest.main()
