"""Data structure to represent a definition of a wire in a module."""

from tapa.graphir.types.commons import HierarchicalNamedModel
from tapa.graphir.types.expressions import Expression, Range, get_width_expr


class ModuleNet(HierarchicalNamedModel):
    """A net in a module definition.

    Attributes:
        name (str): Name of the net as defined in the implementation.  The
            name is unique inside a grouped module definition.
        hierarchical_name (HierarchicalName): Hierarchical name of the net in the
            original design.  This is useful when a net has been renamed (= original
            name), has been flattened into the parent module (= f"{chile module name}/
            {wire name}"), or does not represent any hierarchical net in the original
            design (= None).
        range (int): Range of the net.

    Example:
        >>> print(ModuleNet(name="test_wire", hierarchical_name=None).model_dump_json())
        {"name":"test_wire","hierarchical_name":null,"range":null}
    """

    range: Range | None = None

    def rewritten(self, idmap: dict[str, Expression]) -> "ModuleNet":
        """Rewrite the expression of the net."""
        if self.range is None:
            return self
        return self.updated(range=self.range.rewrite(idmap))

    def get_width_expr(self) -> Expression:
        """Get the expression for the width of the wire."""
        return get_width_expr(self.range)
