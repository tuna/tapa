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

@test "testsuite: minizip-ng is installed for tapa g++" {
  find "${TAPA_HOME}/usr/lib" -maxdepth 1 \
    \( -name "libminizip_ng.a" -o -name "libminizip_ng.so" -o -name "libminizip_ng.dylib" \) \
    | grep -q .
}

@test "testsuite: tapa is runnable" {
  tapa --help
}

@test "testsuite: XILINX_HLS is set" {
  [ -d "${XILINX_HLS}" ]
}

@test "testsuite: vitis_hls is runnable" {
  vitis_hls --version
}
