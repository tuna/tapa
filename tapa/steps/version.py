"""Print TAPA version to standard output."""

import sys

import click

from tapa import __version__


@click.command()
def version() -> None:
    """Print TAPA version to standard output."""
    sys.stdout.write(__version__)
