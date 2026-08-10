"""Isolated stdlib tests for the Cargo source-provenance bootstrap."""

from __future__ import annotations

import importlib.util
import json
import os
from pathlib import Path
import shutil
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
PREVIOUS_DONT_WRITE_BYTECODE = sys.dont_write_bytecode
try:
    sys.dont_write_bytecode = True
    SPEC.loader.exec_module(PROVENANCE)
finally:
    sys.dont_write_bytecode = PREVIOUS_DONT_WRITE_BYTECODE


class Fixture:
    def __init__(self, base: Path) -> None:
        self.root = base / "repo"
        self.member = self.root / "member"
        self.cargo_home = base / "cargo-home"
        self.member.mkdir(parents=True)
        self.cargo_home.mkdir()
        workflow = self.root / ".github" / "workflows" / "ci.yml"
        workflow.parent.mkdir(parents=True)
        shutil.copy2(SCRIPT.parents[3] / ".github" / "workflows" / "ci.yml", workflow)
        self.write_policy([])
        self.write_root()
        (self.member / "Cargo.toml").write_text(
            '[package]\nname = "fixture-member"\nversion = "0.0.0"\nedition = "2024"\n',
            encoding="utf-8",
        )

    def write_policy(self, dependencies: list[dict[str, object]]) -> None:
        normalized_dependencies = []
        for dependency in dependencies:
            row = dependency.copy()
            row.setdefault("optional", False)
            row.setdefault("usesDefaultFeatures", True)
            row.setdefault("features", [])
            row.setdefault(
                "resolution",
                {
                    "kind": "third-party",
                    "package": row["package"],
                    "version": "1.0.0",
                    "source": PROVENANCE.CRATES_IO_SOURCE,
                },
            )
            normalized_dependencies.append(row)
        policy = self.root / "tools" / "xtask" / "dependency-surface-policy.v1.json"
        policy.parent.mkdir(parents=True, exist_ok=True)
        policy.write_text(
            json.dumps(
                {
                    "schemaVersion": 1,
                    "workspaceResolver": "3",
                    "packages": [
                        {
                            "name": "fixture-member",
                            "dependencies": normalized_dependencies,
                        }
                    ],
                }
            ),
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

    @unittest.skipUnless(os.name == "nt", "Windows extended paths are platform-specific")
    def test_windows_extended_root_matches_normal_working_directory(self) -> None:
        temporary, fixture = self.fixture()
        with temporary:
            extended_root = Path("\\\\?\\" + str(fixture.root))
            PROVENANCE.validate_invocation(
                extended_root,
                ("cargo", "--version"),
                {},
                fixture.root,
                fixture.cargo_home,
            )

    def test_import_does_not_write_repository_bytecode(self) -> None:
        self.assertFalse((SCRIPT.parent / "__pycache__").exists())

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

    def test_registry_root_lexical_physical_disagreement_is_rejected(self) -> None:
        temporary, fixture = self.fixture()
        with temporary:
            redirected = fixture.cargo_home / "redirected"
            with self.assertRaisesRegex(PROVENANCE.ProvenanceError, "lexical/physical"):
                PROVENANCE.validate_registry_root_identity(
                    fixture.cargo_home,
                    fixture.cargo_home,
                    fixture.cargo_home / "registry" / "src",
                    redirected,
                )

    @unittest.skipIf(os.name == "nt", "Windows runners may not grant symlink privileges")
    def test_absent_registry_source_below_redirected_parent_is_rejected(self) -> None:
        temporary, fixture = self.fixture()
        with temporary:
            redirected = fixture.cargo_home / "redirected-registry"
            redirected.mkdir()
            (fixture.cargo_home / "registry").symlink_to(
                redirected, target_is_directory=True
            )
            with self.assertRaisesRegex(PROVENANCE.ProvenanceError, "lexical/physical"):
                fixture.validate()

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

    def test_semantic_resolver_formatting_is_accepted(self) -> None:
        temporary, fixture = self.fixture()
        with temporary:
            (fixture.root / "Cargo.toml").write_text(
                'workspace.resolver="3" # exact semantic value\n'
                'workspace.members = ["member"]\n',
                encoding="utf-8",
            )
            fixture.validate()

    def test_authored_requirement_string_is_not_cargo_normalized(self) -> None:
        temporary, fixture = self.fixture()
        with temporary:
            fixture.write_root(
                suffix='\n[workspace.dependencies]\nserde = "=1.0.0"\n'
            )
            (fixture.member / "Cargo.toml").write_text(
                '[package]\nname = "fixture-member"\nversion = "0.0.0"\n'
                '[dependencies]\nserde.workspace = true\n',
                encoding="utf-8",
            )
            fixture.write_policy(
                [
                    {
                        "package": "serde",
                        "rename": None,
                        "requirement": "=1.0.0",
                        "kind": "normal",
                        "target": None,
                    }
                ]
            )
            fixture.validate()

            fixture.write_root(
                suffix='\n[workspace.dependencies]\nserde = "= 1.0.0"\n'
            )
            with self.assertRaisesRegex(PROVENANCE.ProvenanceError, "contract drift"):
                fixture.validate()

    def test_dependency_source_substitutions_are_rejected_before_cargo(self) -> None:
        temporary, fixture = self.fixture()
        with temporary:
            rogue = fixture.root / "rogue"
            rogue.mkdir()
            (rogue / "Cargo.toml").write_text(
                '[package]\nname = "serde"\nversion = "1.0.0"\n',
                encoding="utf-8",
            )
            (rogue / "build.rs").write_text("fn main() {}\n", encoding="utf-8")
            (fixture.member / "Cargo.toml").write_text(
                '[package]\nname = "fixture-member"\nversion = "0.0.0"\n'
                '[dependencies]\nserde.workspace = true\n',
                encoding="utf-8",
            )
            fixture.write_policy(
                [
                    {
                        "package": "serde",
                        "rename": None,
                        "requirement": "=1.0.0",
                        "kind": "normal",
                        "target": None,
                    }
                ]
            )
            substitutions = (
                'serde = { version = "=1.0.0", path = "rogue" }',
                'serde = { version = "=1.0.0", git = "https://example.invalid/serde" }',
                'serde = { version = "=1.0.0", registry = "private" }',
            )
            for dependency in substitutions:
                with self.subTest(dependency=dependency):
                    fixture.write_root(
                        suffix=f"\n[workspace.dependencies]\n{dependency}\n"
                    )
                    with self.assertRaisesRegex(
                        PROVENANCE.ProvenanceError,
                        "workspace member|forbidden source selectors",
                    ):
                        fixture.validate()

    def test_root_package_dependencies_are_included_before_cargo(self) -> None:
        temporary, fixture = self.fixture()
        with temporary:
            rogue = fixture.root / "rogue"
            rogue.mkdir()
            (rogue / "Cargo.toml").write_text(
                '[package]\nname = "rogue"\nversion = "0.0.0"\n',
                encoding="utf-8",
            )
            (rogue / "build.rs").write_text("fn main() {}\n", encoding="utf-8")
            fixture.write_root(
                suffix=(
                    '\n[package]\nname = "root-member"\nversion = "0.0.0"\n'
                    '[dependencies]\nrogue = { path = "rogue" }\n'
                )
            )
            with self.assertRaisesRegex(PROVENANCE.ProvenanceError, "workspace member"):
                fixture.validate()

    def test_workflow_drift_and_additional_workflows_are_rejected(self) -> None:
        temporary, fixture = self.fixture()
        with temporary:
            workflow = fixture.root / ".github" / "workflows" / "ci.yml"
            original = workflow.read_bytes()
            workflow.write_bytes(original + b"\n")
            with self.assertRaisesRegex(PROVENANCE.ProvenanceError, "digest mismatch"):
                fixture.validate()
            workflow.write_bytes(original)

            extra = workflow.with_name("escape.yaml")
            extra.write_text("name: bypass\n", encoding="utf-8")
            with self.assertRaisesRegex(PROVENANCE.ProvenanceError, "only ci.yml"):
                fixture.validate()

    def test_workspace_build_scripts_are_rejected(self) -> None:
        temporary, fixture = self.fixture()
        with temporary:
            (fixture.member / "build.rs").write_text("fn main() {}\n", encoding="utf-8")
            with self.assertRaisesRegex(PROVENANCE.ProvenanceError, "build script"):
                fixture.validate()

    def test_root_workspace_package_build_scripts_are_rejected(self) -> None:
        temporary, fixture = self.fixture()
        with temporary:
            package = (
                '\n[package]\nname = "root-member"\nversion = "0.0.0"\n'
                'edition = "2024"\n'
            )
            fixture.write_root(suffix=package)
            build_script = fixture.root / "build.rs"
            build_script.write_text("fn main() {}\n", encoding="utf-8")
            with self.assertRaisesRegex(PROVENANCE.ProvenanceError, "build script"):
                fixture.validate()

            build_script.unlink()
            fixture.write_root(suffix=package + 'build = "scripts/build.rs"\n')
            with self.assertRaisesRegex(PROVENANCE.ProvenanceError, "build script"):
                fixture.validate()

    @unittest.skipIf(os.name == "nt", "Windows runners may not grant symlink privileges")
    def test_redirected_workspace_manifest_is_rejected(self) -> None:
        temporary, fixture = self.fixture()
        with temporary:
            outside = fixture.root.parent / "outside-Cargo.toml"
            outside.write_text(
                '[package]\nname = "fixture-member"\nversion = "0.0.0"\n',
                encoding="utf-8",
            )
            manifest = fixture.member / "Cargo.toml"
            manifest.unlink()
            manifest.symlink_to(outside)
            with self.assertRaisesRegex(PROVENANCE.ProvenanceError, "redirected or external"):
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

    def test_real_entrypoint_uses_scrubbed_host_state_and_controlled_cargo(self) -> None:
        cargo = shutil.which("cargo")
        self.assertIsNotNone(cargo, "cargo must be available for the bootstrap test")
        root = SCRIPT.parents[3]
        with tempfile.TemporaryDirectory() as raw_temporary:
            temporary = Path(raw_temporary)
            cargo_home = temporary / "cargo-home"
            binary_directory = temporary / "bin"
            cargo_home.mkdir()
            binary_directory.mkdir()
            cargo_name = "cargo.exe" if os.name == "nt" else "cargo"
            shutil.copy2(Path(cargo).resolve(strict=True), binary_directory / cargo_name)

            inherited = (
                "COMSPEC",
                "HOME",
                "PATHEXT",
                "RUSTUP_HOME",
                "RUSTUP_TOOLCHAIN",
                "SYSTEMDRIVE",
                "SYSTEMROOT",
                "TEMP",
                "TMP",
                "USERPROFILE",
                "WINDIR",
            )
            environment = {
                name: os.environ[name] for name in inherited if name in os.environ
            }
            environment["CARGO_HOME"] = str(cargo_home)
            environment["PATH"] = str(binary_directory)

            check_only = subprocess.run(
                [sys.executable, "-I", "-S", str(SCRIPT), "--check-only"],
                cwd=root,
                env=environment,
                check=False,
                capture_output=True,
                text=True,
            )
            self.assertEqual(check_only.returncode, 0, check_only.stderr)
            self.assertEqual(check_only.stdout, "")

            completed = subprocess.run(
                [sys.executable, "-I", "-S", str(SCRIPT), "--", "cargo", "--version"],
                cwd=root,
                env=environment,
                check=False,
                capture_output=True,
                text=True,
            )
            self.assertEqual(completed.returncode, 0, completed.stderr)
            self.assertTrue(completed.stdout.startswith("cargo "), completed.stdout)
            self.assertFalse(
                (root / "%SystemDrive%").exists(),
                "the isolated bootstrap must not write Windows cache state in the repository",
            )


if __name__ == "__main__":
    unittest.main()
