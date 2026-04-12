#!/bin/bash

# Copyright (c) 2025 RapidStream Design Automation, Inc. and contributors.
# All rights reserved. The contributor(s) of this file has/have agreed to the
# RapidStream Contributor License Agreement.

# Tests that TAPA fast cosim can resume from generated work directory.

set -ex

"$@" --cosim_work_dir="${TEST_TMPDIR}" --cosim_setup_only
TAPA_DPI_CONFIG=$(cat "${TEST_TMPDIR}/dpi_config.json") \
  HOME="${TEST_TMPDIR}/run" \
  vivado -mode batch -source "${TEST_TMPDIR}/run_cosim.tcl"
"$@" --cosim_work_dir="${TEST_TMPDIR}" --cosim_resume_from_post_sim
