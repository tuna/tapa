"""Gate build rules on the availability of the vendor tools they invoke."""

def incompatible_unless_available(available, kwargs):
    """Return rule kwargs, marked incompatible when the needed tool is missing.

    A target whose vendor tool is not installed cannot run; marking it
    incompatible lets Bazel report it as skipped instead of failing late in
    the action. Caller-supplied `target_compatible_with` entries are
    preserved.

    Args:
      available: Whether the required tool is installed.
      kwargs: Keyword arguments destined for the underlying rule.

    Returns:
      A copy of `kwargs` with `@platforms//:incompatible` appended to
      `target_compatible_with` when `available` is false.
    """
    kwargs = dict(kwargs)
    constraints = list(kwargs.get("target_compatible_with", []))
    if not available and "@platforms//:incompatible" not in constraints:
        constraints.append("@platforms//:incompatible")
    kwargs["target_compatible_with"] = constraints
    return kwargs
