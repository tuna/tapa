"""Instance of a child Task in an upper-level task."""

__copyright__ = """
Copyright (c) 2024 RapidStream Design Automation, Inc. and contributors.
All rights reserved. The contributor(s) of this file has/have agreed to the
RapidStream Contributor License Agreement.
"""

import enum
from typing import TYPE_CHECKING

from tapa.util import (
    as_type,
    as_type_or_none,
    get_indexed_name,
    get_instance_name,
)
from tapa.verilog.util import sanitize_array_name

if TYPE_CHECKING:
    from tapa.task import Task


class Instance:
    """Instance of a child Task in an upper-level task.

    A task can be instantiated multiple times in the same upper-level task.
    Each object of this class corresponds to such a instance of a task.

    Attributes:
      task: Task, corresponding task of this instance.
      instance_id: int, index of the instance of the same task.
      step: int, bulk-synchronous step when instantiated.
      args: a dict mapping arg names to Arg.

    Properties:
      name: str, instance name, unique in the parent module.
    """

    class Arg:
        class Cat(enum.Enum):
            INPUT = 1 << 0
            OUTPUT = 1 << 1
            SCALAR = 1 << 2
            STREAM = 1 << 3
            MMAP = 1 << 4
            ASYNC = 1 << 5
            ASYNC_MMAP = (1 << 4) | (1 << 5)
            ISTREAM = (1 << 3) | (1 << 0)
            OSTREAM = (1 << 3) | (1 << 1)
            IMMAP = (1 << 4) | (1 << 0)
            OMMAP = (1 << 4) | (1 << 1)
            STREAMS = 1 << 6
            ISTREAMS = (1 << 6) | (1 << 0)
            OSTREAMS = (1 << 6) | (1 << 1)

            @property
            def is_scalar(self) -> bool:
                return self == self.SCALAR

            @property
            def is_istream(self) -> bool:
                return self == self.ISTREAM

            @property
            def is_ostream(self) -> bool:
                return self == self.OSTREAM

            @property
            def is_stream(self) -> bool:
                return bool(self.value & self.STREAM.value)

            @property
            def is_istreams(self) -> bool:
                return self == self.ISTREAMS

            @property
            def is_ostreams(self) -> bool:
                return self == self.OSTREAMS

            @property
            def is_streams(self) -> bool:
                return bool(self.value & self.STREAMS.value)

            @property
            def is_sync_mmap(self) -> bool:
                return self == self.MMAP

            @property
            def is_async_mmap(self) -> bool:
                return self == self.ASYNC_MMAP

            @property
            def is_mmap(self) -> bool:
                return bool(self.value & self.MMAP.value)

            @property
            def is_immap(self) -> bool:
                return self == self.IMMAP

            @property
            def is_ommap(self) -> bool:
                return self == self.OMMAP

        # Shared lookup for Arg and Port; hmap maps to MMAP like an upper-level mmap.
        _CAT_LOOKUP: "dict[str, Instance.Arg.Cat]"

        def __init__(
            self,
            name: str,
            instance: "Instance",
            cat: str | Cat,
            port: str,
            is_upper: bool = False,
        ) -> None:
            self.name = name
            self.instance = instance
            if isinstance(cat, str):
                self.cat = Instance.Arg._CAT_LOOKUP[cat]  # noqa: SLF001
                # only lower-level async_mmap is acknowledged
                if is_upper and self.cat == Instance.Arg.Cat.ASYNC_MMAP:
                    self.cat = Instance.Arg.Cat.MMAP
            else:
                self.cat = cat
            self.port = port
            self.width = None
            self.shared = False  # only set for (async) mmaps
            self.chan_count: int | None = None  # only set for hmap
            self.chan_size: int | None = None  # only set for hmap

        def __lt__(self, other: object) -> bool:
            if isinstance(other, Instance.Arg):
                return self.name < other.name
            return NotImplemented

        @property
        def mmap_name(self) -> str:
            return self.get_mmap_name()

        def get_mmap_name(self, idx: int | None = None) -> str:
            assert self.cat in {Instance.Arg.Cat.MMAP, Instance.Arg.Cat.ASYNC_MMAP}
            indexed_name = get_indexed_name(self.name, idx)
            if self.shared:
                return f"{indexed_name}___{self.instance.name}___{self.port}"
            return indexed_name

    def __init__(
        self,
        task: "Task",
        instance_id: int,
        **kwargs: object,
    ) -> None:
        self.task = task
        self.instance_id = instance_id
        self.step = as_type(int, kwargs["step"])
        self.args: tuple[Instance.Arg, ...] = tuple(
            sorted(
                Instance.Arg(
                    name=sanitize_array_name(arg["arg"]),
                    instance=self,
                    cat=arg["cat"],
                    port=port,
                    is_upper=task.is_upper,
                )
                for port, arg in as_type(dict, kwargs["args"]).items()
            ),
        )

    @property
    def name(self) -> str:
        return get_instance_name((self.task.name, self.instance_id))

    @property
    def is_autorun(self) -> bool:
        return self.step < 0

    def get_instance_arg(self, arg: str) -> str:
        if "'d" in arg:  # Constant literals are passed as-is.
            return arg
        return f"{self.name}___{arg}"


# Populate _CAT_LOOKUP after Instance.Arg.Cat is defined.
# Port uses this too; hmap and immap/ommap are Port-only entries.
Instance.Arg._CAT_LOOKUP = {  # noqa: SLF001
    "istream": Instance.Arg.Cat.ISTREAM,
    "ostream": Instance.Arg.Cat.OSTREAM,
    "istreams": Instance.Arg.Cat.ISTREAMS,
    "ostreams": Instance.Arg.Cat.OSTREAMS,
    "scalar": Instance.Arg.Cat.SCALAR,
    "mmap": Instance.Arg.Cat.MMAP,
    "immap": Instance.Arg.Cat.IMMAP,
    "ommap": Instance.Arg.Cat.OMMAP,
    "async_mmap": Instance.Arg.Cat.ASYNC_MMAP,
    "hmap": Instance.Arg.Cat.MMAP,
}


class Port:
    def __init__(self, obj: dict[str, str | int]) -> None:
        self.cat = Instance.Arg._CAT_LOOKUP[as_type(str, obj["cat"])]  # noqa: SLF001
        self.name = sanitize_array_name(as_type(str, obj["name"]))
        self.ctype = as_type(str, obj["type"])
        self.width = as_type(int, obj["width"])
        self.chan_count = as_type_or_none(int, obj.get("chan_count"))
        self.chan_size = as_type_or_none(int, obj.get("chan_size"))

    def __str__(self) -> str:
        return ", ".join(f"{k}: {v}" for k, v in self.__dict__.items())

    @property
    def is_istreams(self) -> bool:
        """If port is istreams."""
        return self.cat.is_istreams

    @property
    def is_ostreams(self) -> bool:
        """If port is ostreams."""
        return self.cat.is_ostreams

    @property
    def is_streams(self) -> bool:
        """If port is streams."""
        return self.cat.is_streams

    @property
    def is_istream(self) -> bool:
        """If port is istreams."""
        return self.cat.is_istream

    @property
    def is_ostream(self) -> bool:
        """If port is istreams."""
        return self.cat.is_ostream

    @property
    def is_immap(self) -> bool:
        """If port is immap."""
        return self.cat.is_immap

    @property
    def is_ommap(self) -> bool:
        """If port is ommap."""
        return self.cat.is_ommap
