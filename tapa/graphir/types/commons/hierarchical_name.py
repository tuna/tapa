"""Common feature all module related types should have."""

from abc import abstractmethod
from collections.abc import Generator

from tapa.graphir.types.commons.model import RootModel
from tapa.graphir.types.commons.name import NamedModel, NamespaceModel


class HierarchicalName(RootModel[tuple[str, ...] | None]):
    """A hierarchical name of an object in the original design.

    The full path of a namespace is the concatenation of the hierarchical names of the
    parent modules and the hierarchical name of the namespace, e.g.,
    `(top, flattened, sub)`.  If a namespace does not represent any hierarchical
    namespace in the original design, the root is `None`.
    """

    def __add__(self, other: "HierarchicalName") -> "HierarchicalName":
        """Concatenate two hierarchical names."""
        assert isinstance(other, HierarchicalName)
        if self.root is None or other.root is None:
            return HierarchicalName(root=None)
        return HierarchicalName(root=self.root + other.root)

    def __str__(self) -> str:
        """Get the string representation of the hierarchical name."""
        if not self.root:
            return "non existing"
        return "/".join(self.root)

    @staticmethod
    def get_nonexist() -> "HierarchicalName":
        """Get the name of an object that does not exist in the original design."""
        return HierarchicalName(root=None)

    @staticmethod
    def get_empty() -> "HierarchicalName":
        """Get the name of an additional hierarchy that is transparent to children."""
        return HierarchicalName(root=())

    @staticmethod
    def get_name(name: str) -> "HierarchicalName":
        """Get the trivial name of an object in the original design."""
        return HierarchicalName(root=(name,))

    def is_nonexist(self) -> bool:
        """Check if the name is non-existing."""
        return self.root is None

    def is_empty(self) -> bool:
        """Check if the name is empty."""
        return self.root is not None and len(self.root) == 0

    def get_leaf_name(self) -> str | None:
        """Get the leaf name of the hierarchical name."""
        if not self.root:
            return None
        return self.root[-1]


class HierarchicalNamespaceModel(NamespaceModel):
    """A namespace that represents a hierarchical namespace in the original design.

    Attributes:
        hierarchical_name (HierarchicalName): Hierarchical name of the namespace in the
            original design.  See `HierarchicalName` for more details.
    """

    hierarchical_name: HierarchicalName

    @abstractmethod
    def get_all_named(self) -> Generator[NamedModel]:
        """Yield all the named objects in the namespace.

        Yields:
            NamedModel: Nnamed objects in the namespace.
        """


class HierarchicalNamedModel(NamedModel):
    """A named object that represents a hierarchical object in the original design.

    Attributes:
        hierarchical_name (HierarchicalName): Hierarchical name of the named object in
            the original design.  See `HierarchicalName` for more details.
    """

    hierarchical_name: HierarchicalName
