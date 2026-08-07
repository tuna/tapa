#include <cstdlib>
#include <string>

#include <gflags/gflags.h>

// C++ owns every user-facing FRT flag. Rust never defines a flag; it only
// consumes the `FRT_*` environment variables that `ForwardFlagsToEnv` sets.
DEFINE_bool(xsim_start_gui, false,
            "open the Vivado GUI for interactive xsim debugging");
DEFINE_bool(xsim_save_waveform, false,
            "save xsim waveform output in the work directory");
DEFINE_string(cosim_work_dir, "",
              "if not empty, keep cosim artifacts in the specified directory");
DEFINE_bool(cosim_work_dir_parallel, false,
            "create a unique work directory per concurrent cosim instance");
DEFINE_string(xsim_part_num, "",
              "if not empty, override the FPGA part number for xsim");
DEFINE_string(cosim_simulator, "",
              "simulator backend to use: 'xsim' (default) or 'verilator'");
DEFINE_bool(cosim_setup_only, false,
            "generate the cosim work directory but do not run the simulator");
DEFINE_bool(cosim_resume_from_post_sim, false,
            "skip re-running the simulator and execute only post-sim checks");
DEFINE_string(xocl_bdf, "",
              "if not empty, use the specified PCIe Bus:Device:Function for "
              "XRT/OpenCL device selection");

namespace tapa {
namespace internal {
namespace frt {

namespace {

void SetEnvIf(const char* name, const std::string& val) {
  if (!val.empty()) {
    setenv(name, val.c_str(), 1);
  }
}

void SetBoolEnvIf(const char* name, bool val) {
  // Only set the env var when the flag is true; when false, preserve any
  // user-provided env var instead of silently clearing it.
  if (val) {
    setenv(name, "1", 1);
  }
}

}  // namespace

// Which backend runs a bitstream is decided once, in Rust
// (`ExecutionMode::of`), from the same path this flag accompanies. Passing
// the simulator name for a hardware bitstream is harmless: it is ignored.
const char* SimulatorFlag() {
  return FLAGS_cosim_simulator.empty() ? nullptr
                                       : FLAGS_cosim_simulator.c_str();
}

// Forwarded unconditionally: every one of these is read only by the cosim
// backend, so no forwarding decision has to know the execution mode.
void ForwardFlagsToEnv() {
  SetEnvIf("FRT_XOCL_BDF", FLAGS_xocl_bdf);
  SetBoolEnvIf("FRT_XSIM_START_GUI", FLAGS_xsim_start_gui);
  SetBoolEnvIf("FRT_XSIM_SAVE_WAVEFORM", FLAGS_xsim_save_waveform);
  SetEnvIf("FRT_COSIM_WORK_DIR", FLAGS_cosim_work_dir);
  SetBoolEnvIf("FRT_COSIM_WORK_DIR_PARALLEL", FLAGS_cosim_work_dir_parallel);
  SetEnvIf("FRT_XSIM_PART_NUM", FLAGS_xsim_part_num);
  SetBoolEnvIf("FRT_COSIM_SETUP_ONLY", FLAGS_cosim_setup_only);
  SetBoolEnvIf("FRT_COSIM_RESUME_FROM_POST_SIM",
               FLAGS_cosim_resume_from_post_sim);
}

bool CosimSetupOnly() { return FLAGS_cosim_setup_only; }

}  // namespace frt
}  // namespace internal
}  // namespace tapa
