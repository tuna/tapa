"""Base class of immutable objects that is shared among TAPA graph IR types."""

from typing import Self, TypeVar

from pydantic import BaseModel, ConfigDict
from pydantic import RootModel as PydanticRootModel

X = TypeVar("X")


class ModelMixin:
    """The base model for immutable and hashable tapa graph IR types."""

    def updated(self, **update: object) -> Self:
        """Return a new object attached to the original namespace with fields updated.

        For example, you can `module.updated(name='new_name')` to update the
        name of an immutable module, which returns the updated module.

        Note that if the model is a named model, you shall not use the original
        object after the update as its namespace will be detached.

        Args:
            **update (object): The fields to update.

        Returns:
            _X: The updated immutable object.
        """
        unknown = set(update) - set(self.model_fields)  # type: ignore[reportAttributeAccessIssue]
        if unknown:
            msg = f"Unknown fields to update: {unknown}"
            raise ValueError(msg)
        return self.model_copy(update=update, deep=False)  # type: ignore[reportAttributeAccessIssue]

    @staticmethod
    def get_name_of_object(inst: object | dict[str, object]) -> str:
        """Return the name of a named object or its dictionary representation."""
        if isinstance(inst, dict):
            return str(inst["name"])
        if hasattr(inst, "name"):
            return str(inst.name)  # type: ignore[reportAttributeAccessIssue]
        if isinstance(inst, str):
            return inst
        msg = f"Cannot get name of {inst}."
        raise ValueError(msg)

    @classmethod
    def sort_tuple_field(cls, kwargs: dict[str, object], field: str) -> None:
        """Sort the tuple `field` in `kwargs` by name and return the argument."""
        if field in kwargs and (data := kwargs[field]):
            data_seq = list(data)  # type: ignore[call-overload]
            # The items in data should have unique names.
            names = [cls.get_name_of_object(inst) for inst in data_seq]
            if len(set(names)) != len(names):
                msg = f"Duplicate names in {field}: {data}."
                raise ValueError(msg)
            kwargs[field] = tuple(sorted(data_seq, key=cls.get_name_of_object))

    model_config = ConfigDict(frozen=True)


class Model(BaseModel, ModelMixin):
    """The base model of immutable and hashable tapa graph IR types."""


class RootModel(PydanticRootModel[X], ModelMixin):
    """The base model of immutable and hashable tapa graph IR types."""
