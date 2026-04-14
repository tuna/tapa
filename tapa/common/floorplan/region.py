"""Region format conversion utilities."""


def convert_region_format(region: str | None) -> str | None:
    """Convert region format from 'x:y' to 'x_TO_y'."""
    if region is None:
        return None
    return region.replace(":", "_TO_")
