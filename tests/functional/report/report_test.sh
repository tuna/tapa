#!/usr/bin/env bash
set -euo pipefail

exec "$1" check-xo-reports tests/functional/report/enable-synth-util.xo
