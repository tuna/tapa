#include "xilinx_environ.h"

#include <sstream>
#include <string>
#include <string_view>

#include <boost/asio.hpp>
#include <boost/process/v2.hpp>

namespace fpga::xilinx {

namespace {

namespace bp = boost::process::v2;

// Boost.Process v2 default_launcher uses execve (no PATH search). Resolve bare
// executable names (no '/' in the name) to full paths via the current PATH.
bp::filesystem::path ResolveExe(const std::string& name) {
  bp::filesystem::path p(name);
  if (p.has_parent_path()) return p;
  auto found = bp::environment::find_executable(name);
  return found.empty() ? p : found;
}

// Run a shell command with the given environment, capture stdout, return it.
// The captured output is NUL-delimited (env -0 output), so we return the raw
// bytes as a std::string.
std::string RunCapture(const std::vector<std::string>& args,
                       const Environ& environ) {
  boost::asio::io_context ioc;
  boost::asio::readable_pipe rp{ioc};
  boost::system::error_code ec;
  bp::process proc{ioc, ResolveExe(args[0]),
                   std::vector<std::string>(args.begin() + 1, args.end()),
                   bp::process_stdio{{}, rp, {}},
                   bp::process_environment(environ)};
  std::string out;
  boost::asio::read(rp, boost::asio::dynamic_buffer(out),
                    boost::asio::transfer_all(), ec);
  proc.wait();
  // ec == boost::asio::error::eof is normal (pipe closed after process exits)
  return out;
}

// Same but inherits the parent process environment (no custom env map).
std::string RunCaptureDefaultEnv(const std::vector<std::string>& args) {
  boost::asio::io_context ioc;
  boost::asio::readable_pipe rp{ioc};
  boost::system::error_code ec;
  bp::process proc{ioc, ResolveExe(args[0]),
                   std::vector<std::string>(args.begin() + 1, args.end()),
                   bp::process_stdio{{}, rp, {}}};
  std::string out;
  boost::asio::read(rp, boost::asio::dynamic_buffer(out),
                    boost::asio::transfer_all(), ec);
  proc.wait();
  return out;
}

void UpdateEnviron(std::string_view script, Environ& environ) {
  std::string output = RunCapture(
      {
          "bash",
          "-c",
          "source \"$0\" >/dev/null 2>&1 && env -0",
          std::string(script),
      },
      environ);

  for (size_t n = 0; n < output.size();) {
    std::string_view line = output.data() + n;
    n += line.size() + 1;
    auto pos = line.find('=');
    environ[std::string(line.substr(0, pos))] = line.substr(pos + 1);
  }
}

}  // namespace

Environ GetEnviron() {
  std::string xilinx_tool;
  for (const char* env :
       {"XILINX_VITIS", "XILINX_SDX", "XILINX_HLS", "XILINX_VIVADO"}) {
    if (const char* value = getenv(env)) {
      xilinx_tool = value;
      break;
    }
  }

  if (xilinx_tool.empty()) {
    for (const std::string& hls : {"vitis_hls", "vivado_hls"}) {
      std::string out = RunCaptureDefaultEnv({
          "bash",
          "-c",
          "\"$0\" -version -help -l /dev/null 2>/dev/null",
          hls,
      });
      std::istringstream lines(out);
      const std::string_view prefix = "source ";
      const std::string suffix = "/scripts/" + hls + "/hls.tcl -notrace";
      for (std::string line; getline(lines, line);) {
        if (line.size() > prefix.size() + suffix.size() &&
            line.compare(0, prefix.size(), prefix) == 0 &&
            line.compare(line.size() - suffix.size(), suffix.size(), suffix) ==
                0) {
          xilinx_tool = line.substr(
              prefix.size(), line.size() - prefix.size() - suffix.size());
          break;
        }
      }
    }
  }

  // The old subprocess.h implementation used setenv() inside the forked child,
  // which merged Xilinx vars on top of the inherited parent environment. Match
  // that behavior: seed the environment from the parent so PATH,
  // LD_LIBRARY_PATH, etc. (including user-installed tools like verilator) are
  // preserved, and then let the Xilinx settings scripts extend/override on top.
  Environ environ;
  for (char** ep = ::environ; *ep != nullptr; ++ep) {
    std::string_view entry(*ep);
    auto eq = entry.find('=');
    if (eq != std::string_view::npos)
      environ[std::string(entry.substr(0, eq))] = entry.substr(eq + 1);
  }
  UpdateEnviron(xilinx_tool + "/settings64.sh", environ);
  if (const char* xrt = getenv("XILINX_XRT"))
    UpdateEnviron(std::string(xrt) + "/setup.sh", environ);
  return environ;
}

}  // namespace fpga::xilinx
