#!/usr/bin/env bash
set -euo pipefail

exec "$1" check-xo-reports "$2"
