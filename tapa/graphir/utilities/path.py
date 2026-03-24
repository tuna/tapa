"""Get the paths of tapa graphir."""

import inspect
import pathlib


def get_relative_path_and_func() -> str:
    """Get the relative path and line number of the caller."""
    frame = inspect.currentframe()
    caller_frame = frame.f_back if frame is not None else None
    if caller_frame is None:
        msg = "Cannot get the caller's frame."
        raise RuntimeError(msg)

    file_name = pathlib.Path(caller_frame.f_code.co_filename).name
    function_name = caller_frame.f_code.co_name
    return f"{file_name}:{function_name}"
