"""Base class of mutable objects that is shared among TAPA graph IR types."""

from typing import TypeVar

from pydantic import BaseModel, RootModel

X = TypeVar("X")


class MutableModel(BaseModel):
    """The base model of tapa graph IR types."""


class MutableRootModel(RootModel[X]):
    """The base model of graph IR types."""
