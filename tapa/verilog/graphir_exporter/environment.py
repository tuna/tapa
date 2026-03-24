"""Jinja2 environment for template rendering."""

from jinja2 import BaseLoader, Environment

env = Environment(
    loader=BaseLoader(),
)
