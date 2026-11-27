"""Exercise the source ratchet on scratch crates, including its exemptions."""

import importlib.util
import tempfile
import unittest
from contextlib import redirect_stderr
from io import StringIO
from pathlib import Path

SPEC = importlib.util.spec_from_file_location(
    "guard", Path(__file__).parents[1] / "check-hdf5-no-spawn.py"
)
GUARD = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(GUARD)


class GuardTests(unittest.TestCase):
    def test_scratch_crates(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            for crate in GUARD.CRATES:
                base = root / "tessera/crates" / crate
                (base / "src").mkdir(parents=True)
                (base / "Cargo.toml").touch()
                (base / "src/lib.rs").write_text("std::thread::spawn(|| {});")
            with redirect_stderr(StringIO()):
                self.assertEqual(GUARD.check(root), 0)
                for crate in GUARD.CRATES:
                    path = root / "tessera/crates" / crate / "src/helper.rs"
                    for source in (
                        'std::process::Command::new("true").spawn();',
                        'let quote = \'"\'; std::process::Command::new("true").spawn();',
                        "use std::process::*; use Command as Launcher;",
                        "use libc::{fork as launch};",
                        "use std::process::{Command as Launcher};",
                        'tokio::process::Command::new("true").output();',
                        "unsafe { libc::fork(); }",
                        "use nix::unistd::{fork as launch};",
                        'duct::cmd("true", []).run();',
                        "unsafe { libc::posix_spawn(); }",
                    ):
                        with self.subTest(crate=crate, source=source):
                            path.write_text(source)
                            self.assertEqual(GUARD.check(root), 1)
                    path.unlink()
                cli = root / "tessera/crates/tessera-cli"
                (cli / "tests").mkdir()
                (cli / "tests/cli.rs").write_text(
                    'std::process::Command::new("tessera");'
                )
                self.assertEqual(GUARD.check(root), 0)
                ingest = root / "tessera/crates/tessera-ingest"
                (ingest / "tests").mkdir()
                (ingest / "tests/read.rs").write_text(
                    'std::process::Command::new("true");'
                )
                self.assertEqual(GUARD.check(root), 1)
                (ingest / "tests/read.rs").unlink()
                (ingest / "Cargo.toml").unlink()
                self.assertEqual(GUARD.check(root), 1)

    def test_literals_and_comments(self):
        source = """// Command
/* outer /* Command */ Command */
let s = r##"Command " inside"##;
let t = "Command";
std::thread::spawn(|| {});
"""
        self.assertIsNone(GUARD.FORBIDDEN.search(GUARD.code_only(source)))
        self.assertEqual(source.count("\n"), GUARD.code_only(source).count("\n"))


if __name__ == "__main__":
    unittest.main()
