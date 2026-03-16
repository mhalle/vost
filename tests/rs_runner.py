"""Subprocess-based CLI runner for the Rust vost binary.

Provides a drop-in replacement for Click's CliRunner so the same
test code can run against either the Python or Rust CLI.

Usage:
    VOST_CLI=rust pytest tests/test_cli.py
"""

from __future__ import annotations

import os
import subprocess
import sys
from dataclasses import dataclass, field


# Resolve the Rust binary path once at import time.
_DEFAULT_BINARY = os.path.join(
    os.path.dirname(__file__), "..", "rs", "target", "debug", "vost"
)
VOST_BINARY = os.environ.get("VOST_BINARY", _DEFAULT_BINARY)


@dataclass
class RustResult:
    """Mimics click.testing.Result enough for the test suite."""
    exit_code: int
    output: str  # stdout + stderr combined (matches Click default)
    stdout: str = ""
    stderr: str = ""
    exception: Exception | None = None
    stdout_bytes: bytes = field(default=b"")

    @property
    def output_bytes(self) -> bytes:
        """Raw bytes from stdout (matches Click's Result.output_bytes)."""
        return self.stdout_bytes


class RustCliRunner:
    """Drop-in replacement for click.testing.CliRunner.

    ``runner.invoke(main, args, input=...)`` shells out to the Rust
    binary instead of calling Python Click code.
    """

    def __init__(self, env: dict[str, str] | None = None):
        self._env = dict(os.environ)
        # Clear VOST_REPO so tests that pass --repo explicitly aren't
        # affected by the outer environment.
        self._env.pop("VOST_REPO", None)
        if env:
            self._env.update(env)

    def invoke(self, cli_main, args: list[str], input: bytes | str | None = None, catch_exceptions: bool = True, env: dict[str, str] | None = None) -> RustResult:
        """Run the Rust binary with *args*.

        *cli_main* is accepted for signature compatibility but ignored —
        the Rust binary is always used.
        """
        cmd = [VOST_BINARY] + list(args)

        run_env = dict(self._env)
        if env:
            run_env.update(env)

        stdin_data: bytes | None = None
        if input is not None:
            stdin_data = input if isinstance(input, bytes) else input.encode()

        try:
            proc = subprocess.run(
                cmd,
                input=stdin_data,
                capture_output=True,
                env=run_env,
                timeout=30,
            )
        except FileNotFoundError:
            return RustResult(
                exit_code=127,
                output=f"Rust binary not found: {VOST_BINARY}\n"
                       f"Build it with: cd rs && cargo build --features cli\n",
            )
        except subprocess.TimeoutExpired:
            return RustResult(exit_code=124, output="Command timed out\n")

        stdout = proc.stdout.decode("utf-8", errors="replace")
        stderr = proc.stderr.decode("utf-8", errors="replace")

        # Click's CliRunner mixes stdout and stderr into .output by default.
        combined = stdout + stderr

        return RustResult(
            exit_code=proc.returncode,
            output=combined,
            stdout=stdout,
            stderr=stderr,
            stdout_bytes=proc.stdout,
        )
