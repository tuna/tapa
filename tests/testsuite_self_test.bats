# Self test for the testsuite if the environment is set correctly
#
# Justification for using bats instead of Bazel for the testsuite:
# Bats mimics the behavior of a user running the tests manually when
# installed on the system, and better reflects the user experience.

# Copyright (c) 2024 RapidStream Design Automation, Inc. and contributors.
# All rights reserved. The contributor(s) of this file has/have agreed to the
# RapidStream Contributor License Agreement.

@test "testsuite: TAPA_HOME is set" {
  [ -d "${TAPA_HOME}" ]
}

@test "testsuite: TAPA_HOME/usr/include exists" {
  [ -d "${TAPA_HOME}/usr/include" ]
}

@test "testsuite: TAPA_HOME/usr/lib exists" {
  [ -d "${TAPA_HOME}/usr/lib" ]
}

@test "testsuite: FRT DPI backends are installed" {
  [ -f "${TAPA_HOME}/usr/lib/libfrt_dpi_verilator.so" ]
  [ -f "${TAPA_HOME}/usr/lib/libfrt_dpi_xsim.so" ]
}

@test "testsuite: FRT host libraries are installed for tapa g++" {
  # The frt_cpp C++ shim is folded into libtapa; libfrt.a is the Rust FRT.
  [ -f "${TAPA_HOME}/usr/lib/libtapa.a" ]
  [ -f "${TAPA_HOME}/usr/lib/libfrt.a" ]
}

@test "testsuite: tapa is runnable" {
  tapa --help
}

@test "testsuite: tapa version contains the repo VERSION string" {
  run tapa version
  [ "${status}" -eq 0 ]
  [[ "${output}" == *"$(cat "${BATS_TEST_DIRNAME}/../VERSION")"* ]]
}

@test "testsuite: tapa floorplan without prior state fails with an actionable error" {
  # Smoke-level wiring check only: no synthesized state exists here, so a
  # real floorplan run is impossible (and there is no Vitis in this env).
  cd "${BATS_TEST_TMPDIR}"
  run tapa floorplan
  [ "${status}" -ne 0 ]
  [[ "${output}" == *"Usage"* || "${output}" == *"error"* || "${output}" == *"missing"* ]]
}

@test "testsuite: XILINX_HLS is set" {
  [ -d "${XILINX_HLS}" ]
}

@test "testsuite: vitis_hls is runnable" {
  vitis_hls --version
}
