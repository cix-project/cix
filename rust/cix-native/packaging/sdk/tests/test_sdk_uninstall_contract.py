"""Focused removal-list regression, not installed SDK or ownership qualification."""

from pathlib import Path
import subprocess
import tempfile
import unittest


NATIVE_ROOT = Path(__file__).resolve().parents[3]


class SdkUninstallContract(unittest.TestCase):
    def test_current_header_and_notice_leaves_leave_unrelated_file(self):
        # Only the four newly installed leaves are selected. Do not mirror the
        # entire SDK install list or substitute this for an installed consumer.
        leaves = (
            "include/cix_presentation.h",
            "share/doc/cix/LICENSE",
            "share/doc/cix/THIRD_PARTY_NOTICES.txt",
            "share/doc/cix/THIRD_PARTY_NOTICES_RUST.md",
        )
        with tempfile.TemporaryDirectory(prefix="cix-sdk-uninstall-") as stage:
            prefix = Path(stage) / "sdk"
            for relative in leaves:
                target = prefix / relative
                target.parent.mkdir(parents=True, exist_ok=True)
                target.write_bytes(b"disposable removal-list fixture\n")
            sentinel = prefix / "share/doc/cix/unrelated.txt"
            sentinel.write_bytes(b"keep this unrelated file\n")
            result = subprocess.run(
                [
                    "make", "--no-print-directory", "-f", str(NATIVE_ROOT / "Makefile"),
                    "uninstall-sdk", "PREFIX=/sdk", f"DESTDIR={stage}",
                ],
                cwd=NATIVE_ROOT,
                check=False,
                capture_output=True,
                text=True,
                timeout=10,
            )
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            remaining = [relative for relative in leaves if (prefix / relative).exists()]
            self.assertEqual(remaining, [], f"SDK leaves remain: {remaining}")
            self.assertEqual(sentinel.read_bytes(), b"keep this unrelated file\n")
            self.assertTrue(sentinel.parent.is_dir())


if __name__ == "__main__":
    unittest.main()
