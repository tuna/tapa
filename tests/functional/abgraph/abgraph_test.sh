#!/usr/bin/env bash
set -euo pipefail

exec "$1" compare-abgraph "$2"
