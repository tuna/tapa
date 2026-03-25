"""Shared synthesis task runner for HLS and AIE backends."""

from __future__ import annotations

import logging
from typing import TYPE_CHECKING, Any

if TYPE_CHECKING:
    from collections.abc import Callable
    from contextlib import AbstractContextManager

_logger = logging.getLogger().getChild(__name__)
_DEFAULT_MAX_ATTEMPTS = 3


def run_synthesis_task(
    runner_factory: Callable[[], AbstractContextManager[Any]],
    *,
    task_name: str,
    work_dir: str,
    max_attempts: int = _DEFAULT_MAX_ATTEMPTS,
) -> tuple[bytes, bytes]:
    """Run a synthesis task with bounded retry.

    Args:
        runner_factory: Callable that returns a fresh context manager each call.
        task_name: Name used for logging.
        work_dir: Working directory (for logging/error messages).
        max_attempts: Max attempts before raising RuntimeError.

    Returns:
        (stdout, stderr) from the successful attempt.

    Raises:
        RuntimeError: If all attempts fail.
    """
    last_exc: Exception | None = None
    for attempt in range(1, max_attempts + 1):
        _logger.info(
            "Running synthesis for task %s (attempt %d/%d)",
            task_name,
            attempt,
            max_attempts,
        )
        with runner_factory() as proc:
            stdout, stderr = proc.communicate()
            if proc.returncode == 0:
                return stdout, stderr
            msg = (
                f"Synthesis failed for task {task_name} "
                f"(attempt {attempt}/{max_attempts}, returncode={proc.returncode})"
            )
            _logger.warning("%s\nstderr: %s", msg, stderr.decode(errors="replace"))
            last_exc = RuntimeError(msg)

    msg = f"All {max_attempts} attempts failed for task {task_name} in {work_dir}"
    raise RuntimeError(msg) from last_exc
