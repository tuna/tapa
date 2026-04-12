// Copyright (c) 2024 RapidStream Design Automation, Inc. and contributors.
// All rights reserved. The contributor(s) of this file has/have agreed to the
// RapidStream Contributor License Agreement.

#include "frt/devices/tapa_fast_cosim_device.h"

#include <cstdlib>

#include <algorithm>
#include <chrono>
#include <fstream>
#include <iomanip>
#include <ios>
#include <memory>
#include <optional>
#include <sstream>
#include <string>
#include <string_view>
#include <unordered_map>
#include <vector>

#include <tinyxml2.h>
#include <unistd.h>
#include <boost/asio.hpp>
#include <boost/process/v2.hpp>

#include <gflags/gflags.h>
#include <glog/logging.h>
#include <yaml-cpp/yaml.h>
#include <nlohmann/json.hpp>

#include <mz.h>
#include <mz_strm.h>
#include <mz_zip.h>
#include <mz_zip_rw.h>

#include "frt/arg_info.h"
#include "frt/devices/filesystem.h"
#include "frt/devices/shared_memory_stream.h"
#include "frt/devices/xilinx_environ.h"
#include "frt/stream_arg.h"

DEFINE_bool(xsim_start_gui, false, "start Vivado GUI for simulation");
DEFINE_bool(xsim_save_waveform, false, "save waveform in the work directory");
DEFINE_string(cosim_work_dir, "",
              "if not empty, use the specified work directory instead of a "
              "temporary one");
DEFINE_bool(cosim_work_dir_parallel, false,
            "create a work directory for each parallel cosim instance");
DEFINE_string(cosim_executable, "",
              "if not empty, use the specified executable instead of "
              "`tapa-fast-cosim`");
DEFINE_string(xsim_part_num, "",
              "if not empty, use the specified part number for Vivado");
DEFINE_string(cosim_simulator, "",
              "simulator backend to use: 'xsim' (default) or 'verilator'");
DEFINE_bool(cosim_setup_only, false, "only setup the simulation");
DEFINE_bool(cosim_resume_from_post_sim, false,
            "skip simulation and do post-sim checking");

namespace fpga {
namespace internal {

namespace {

using clock = std::chrono::steady_clock;

std::string GetWorkDirectory() {
  if (FLAGS_cosim_work_dir.empty()) {
    std::string dir =
        (fs::temp_directory_path() / "tapa-fast-cosim.XXXXXX").string();
    LOG_IF(FATAL, ::mkdtemp(&dir[0]) == nullptr)
        << "failed to create work directory";
    return fs::absolute(dir).string();
  }
  fs::path work_dir = FLAGS_cosim_work_dir;
  if (!fs::exists(work_dir))
    LOG_IF(INFO, fs::create_directories(work_dir))
        << "created directory '" << work_dir << "'";
  if (FLAGS_cosim_work_dir_parallel) {
    std::string dir = (work_dir / "XXXXXX").string();
    LOG_IF(FATAL, ::mkdtemp(&dir[0]) == nullptr)
        << "failed to create work directory";
    work_dir = dir;
  }
  return fs::absolute(work_dir).string();
}

std::string GetInputDataPath(const std::string& work_dir, int index) {
  return work_dir + "/" + std::to_string(index) + ".bin";
}

std::string GetOutputDataPath(const std::string& work_dir, int index) {
  return work_dir + "/" + std::to_string(index) + "_out.bin";
}

std::string GetConfigPath(const std::string& work_dir) {
  return work_dir + "/config.json";
}

}  // namespace

struct TapaFastCosimDevice::Context {
  std::chrono::time_point<std::chrono::steady_clock> start_timestamp;
  boost::asio::io_context ioc;
  std::optional<boost::process::v2::process> proc;
  // Cached exit code: set by Finish() or IsFinished() after the process exits.
  // Avoids double-waitpid (running() reaps child; a later wait() would ECHILD).
  std::optional<int> exit_code;
};

TapaFastCosimDevice::TapaFastCosimDevice(std::string_view xo_path)
    : xo_path(fs::absolute(xo_path)), work_dir(GetWorkDirectory()) {
  if (xo_path.compare(xo_path.size() - 3, 3, ".xo") == 0) {
    LoadArgsFromKernelXml();
  } else if (xo_path.compare(xo_path.size() - 4, 4, ".zip") == 0) {
    LoadArgsFromTapaYaml();
  } else {
    LOG(FATAL) << "Unknown file extension: " << xo_path;
  }

  LOG(INFO) << "Running hardware simulation with TAPA fast cosim";
}

static std::string ReadFileInZip(const std::string& zip_path,
                                 const std::string& filename) {
  void* reader = mz_zip_reader_create();
  if (mz_zip_reader_open_file(reader, zip_path.c_str()) != MZ_OK) {
    mz_zip_reader_delete(&reader);
    LOG(FATAL) << "Cannot open zip: " << zip_path;
  }

  const std::string suffix = "/" + filename;
  int32_t rc = mz_zip_reader_goto_first_entry(reader);
  while (rc == MZ_OK) {
    mz_zip_file* info = nullptr;
    mz_zip_reader_entry_get_info(reader, &info);
    const std::string entry(info->filename);
    if (entry == filename ||
        (entry.size() >= suffix.size() &&
         std::equal(suffix.rbegin(), suffix.rend(), entry.rbegin()))) {
      if (mz_zip_reader_entry_open(reader) != MZ_OK) {
        mz_zip_reader_close(reader);
        mz_zip_reader_delete(&reader);
        LOG(FATAL) << "Cannot open entry '" << filename << "' in " << zip_path;
      }
      std::string content;
      char buf[4096];
      int32_t bytes_read;
      while ((bytes_read = mz_zip_reader_entry_read(reader, buf, sizeof(buf))) >
             0) {
        content.append(buf, bytes_read);
      }
      mz_zip_reader_entry_close(reader);
      mz_zip_reader_close(reader);
      mz_zip_reader_delete(&reader);
      return content;
    }
    rc = mz_zip_reader_goto_next_entry(reader);
  }

  mz_zip_reader_close(reader);
  mz_zip_reader_delete(&reader);
  LOG(FATAL) << "Missing '" << filename << "' in '" << zip_path << "'";
  return "";
}

void TapaFastCosimDevice::LoadArgsFromKernelXml() {
  std::string kernel_xml = ReadFileInZip(xo_path, "kernel.xml");
  tinyxml2::XMLDocument doc;
  doc.Parse(kernel_xml.data());
  for (const tinyxml2::XMLElement* xml_arg = doc.FirstChildElement("root")
                                                 ->FirstChildElement("kernel")
                                                 ->FirstChildElement("args")
                                                 ->FirstChildElement("arg");
       xml_arg != nullptr; xml_arg = xml_arg->NextSiblingElement("arg")) {
    ArgInfo arg;
    arg.index = atoi(xml_arg->Attribute("id"));
    LOG_IF(FATAL, arg.index < 0) << "Invalid argument index: " << arg.index;
    LOG_IF(FATAL, size_t(arg.index) != args_.size())
        << "Expecting argument #" << args_.size() << ", got #" << arg.index;
    arg.name = xml_arg->Attribute("name");
    arg.type = xml_arg->Attribute("type");
    switch (int cat = atoi(xml_arg->Attribute("addressQualifier")); cat) {
      case 0:
        arg.cat = ArgInfo::kScalar;
        break;
      case 1:
        arg.cat = ArgInfo::kMmap;
        break;
      case 4:
        arg.cat = ArgInfo::kStream;
        break;
      default:
        LOG(WARNING) << "Unknown argument category: " << cat;
    }
    args_.push_back(arg);
  }
}

void TapaFastCosimDevice::LoadArgsFromTapaYaml() {
  std::string graph_yaml = ReadFileInZip(xo_path, "graph.yaml");
  YAML::Node graph = YAML::Load(graph_yaml);
  auto ports = graph["tasks"][graph["top"].as<std::string>()]["ports"];

  size_t index = 0;
  for (const auto& port : ports) {
    const auto port_name = port["name"].as<std::string>();
    const auto port_type = port["type"].as<std::string>();
    const auto port_cat = port["cat"].as<std::string>();

    if (port_cat == "mmaps" || port_cat == "hmap") {
      for (int i = 0, n = port["chan_count"].as<int>(); i < n; ++i)
        args_.push_back({(int)index++, port_name + "_" + std::to_string(i),
                         port_type, ArgInfo::kMmap});
      continue;
    }

    ArgInfo::Cat cat;
    if (port_cat == "scalar")
      cat = ArgInfo::kScalar;
    else if (port_cat == "mmap" || port_cat == "async_mmap")
      cat = ArgInfo::kMmap;
    else if (port_cat == "istream" || port_cat == "ostream")
      cat = ArgInfo::kStream;
    else if (port_cat == "istreams" || port_cat == "ostreams")
      cat = ArgInfo::kStreams;
    else {
      LOG(FATAL) << "Unknown argument category: " << port_cat;
      cat = ArgInfo::kScalar;  // Unreachable; silences uninitialized warning.
    }
    args_.push_back({(int)index++, port_name, port_type, cat});
  }
}

TapaFastCosimDevice::~TapaFastCosimDevice() {
  if (FLAGS_cosim_work_dir.empty()) fs::remove_all(work_dir);
}

std::unique_ptr<Device> TapaFastCosimDevice::New(std::string_view path,
                                                 std::string_view content) {
  constexpr std::string_view kZipMagic("PK\3\4", 4);
  if (content.size() < kZipMagic.size() ||
      memcmp(content.data(), kZipMagic.data(), kZipMagic.size()) != 0) {
    return nullptr;
  }
  return std::make_unique<TapaFastCosimDevice>(path);
}

void TapaFastCosimDevice::SetScalarArg(size_t index, const void* arg,
                                       int size) {
  LOG_IF(FATAL, index >= args_.size())
      << "Cannot set argument #" << index << "; there are only " << args_.size()
      << " arguments";
  LOG_IF(FATAL, args_[index].cat != ArgInfo::kScalar)
      << "Cannot set argument '" << args_[index].name
      << "' as a scalar; it is a " << args_[index].cat;
  const auto* arg_bytes = reinterpret_cast<const unsigned char*>(arg);
  std::stringstream ss;
  ss << "'h";
  // Assuming little-endian.
  for (int i = size - 1; i >= 0; --i) {
    ss << std::setfill('0') << std::setw(2) << std::hex << int(arg_bytes[i]);
  }
  scalars_[index] = ss.str();
}

void TapaFastCosimDevice::SetBufferArg(size_t index, Tag tag,
                                       const BufferArg& arg) {
  LOG_IF(FATAL, index >= args_.size())
      << "Cannot set argument #" << index << "; there are only " << args_.size()
      << " arguments";
  LOG_IF(FATAL, args_[index].cat != ArgInfo::kMmap)
      << "Cannot set argument '" << args_[index].name
      << "' as an mmap; it is a " << args_[index].cat;
  buffer_table_.insert({index, arg});
  if (tag == Tag::kReadOnly || tag == Tag::kReadWrite) {
    store_indices_.insert(index);
  }
  if (tag == Tag::kWriteOnly || tag == Tag::kReadWrite) {
    load_indices_.insert(index);
  }
}

void TapaFastCosimDevice::SetStreamArg(size_t index, Tag tag, StreamArg& arg) {
  stream_table_[index] = arg.get<std::shared_ptr<SharedMemoryStream>>();
}

size_t TapaFastCosimDevice::SuspendBuffer(size_t index) {
  return load_indices_.erase(index) + store_indices_.erase(index);
}

void TapaFastCosimDevice::WriteToDevice() {
  is_write_to_device_scheduled_ = true;
}

void TapaFastCosimDevice::WriteToDeviceImpl() {
  auto tic = clock::now();
  for (const auto& [idx, arg] : buffer_table_)
    std::ofstream(GetInputDataPath(work_dir, idx), std::ios::binary)
        .write(arg.Get(), arg.SizeInBytes());
  load_time_ = clock::now() - tic;
}

void TapaFastCosimDevice::ReadFromDevice() {
  is_read_from_device_scheduled_ = true;
}

void TapaFastCosimDevice::ReadFromDeviceImpl() {
  auto tic = clock::now();
  for (int idx : store_indices_) {
    auto arg = buffer_table_.at(idx);
    std::ifstream(GetOutputDataPath(work_dir, idx), std::ios::binary)
        .read(arg.Get(), arg.SizeInBytes());
  }
  store_time_ = clock::now() - tic;
}

void TapaFastCosimDevice::Exec() {
  if (is_write_to_device_scheduled_) {
    WriteToDeviceImpl();
  }

  auto tic = clock::now();

  nlohmann::json json;
  json["xo_path"] = xo_path;
  json["scalar_to_val"] = nlohmann::json::object();
  for (const auto& [idx, scalar] : scalars_)
    json["scalar_to_val"][std::to_string(idx)] = scalar;
  json["axi_to_c_array_size"] = nlohmann::json::object();
  json["axi_to_data_file"] = nlohmann::json::object();
  for (const auto& [idx, content] : buffer_table_) {
    json["axi_to_c_array_size"][std::to_string(idx)] = content.SizeInCount();
    json["axi_to_data_file"][std::to_string(idx)] =
        GetInputDataPath(work_dir, idx);
  }
  json["axis_to_data_file"] = nlohmann::json::object();
  for (const auto& [idx, stream] : stream_table_) {
    VLOG(1) << "arg[" << idx << "] is a stream backed by " << stream->path();
    json["axis_to_data_file"][std::to_string(idx)] = stream->path();
  }

  std::ofstream(GetConfigPath(work_dir)) << json.dump(2);

  std::vector<std::string> argv = {FLAGS_cosim_executable.empty()
                                       ? "tapa-fast-cosim"
                                       : FLAGS_cosim_executable};
  argv.insert(argv.end(), {
                              "--config-path=" + GetConfigPath(work_dir),
                              "--tb-output-dir=" + work_dir + "/output",
                          });
  if (FLAGS_xsim_start_gui) {
    argv.push_back("--start-gui");
  }
  if (FLAGS_xsim_save_waveform) {
    argv.push_back("--save-waveform");
  }
  if (!FLAGS_cosim_setup_only) {
    argv.push_back("--launch-simulation");
  }
  if (!FLAGS_xsim_part_num.empty()) {
    argv.push_back("--part-num=" + FLAGS_xsim_part_num);
  }
  if (!FLAGS_cosim_simulator.empty()) {
    argv.push_back("--simulator=" + FLAGS_cosim_simulator);
  }

  // launch simulation as a noop if resume from post sim
  if (FLAGS_cosim_resume_from_post_sim) {
    argv = {"/bin/sh", "-c", ":"};
  }

  auto env = xilinx::GetEnviron();

  // Boost.Process v2 default_launcher uses execve (no PATH search), unlike
  // subprocess::Popen which used execvpe. Resolve bare names via PATH search.
  boost::process::v2::filesystem::path exe(argv[0]);
  if (!exe.has_parent_path()) {
    auto found = boost::process::v2::environment::find_executable(argv[0]);
    if (!found.empty()) exe = found;
  }

  context_ = std::make_unique<Context>();
  context_->start_timestamp = tic;
  context_->proc.emplace(context_->ioc, exe,
                         std::vector<std::string>(argv.begin() + 1, argv.end()),
                         boost::process::v2::process_environment(env));
}

void TapaFastCosimDevice::Finish() {
  LOG_IF(FATAL, context_ == nullptr) << "Exec() must be called before Finish()";

  // Use cached exit code if IsFinished() already reaped the process via
  // running(); otherwise wait() now and cache for any later IsFinished() call.
  if (!context_->exit_code.has_value()) {
    context_->exit_code = context_->proc->wait();
  }
  if (*context_->exit_code != 0) {
    LOG(ERROR) << "TAPA fast cosim failed with exit code "
               << *context_->exit_code;
    std::terminate();
  }
  LOG(INFO) << "TAPA fast cosim finished successfully";

  if (FLAGS_cosim_setup_only) exit(0);

  compute_time_ = clock::now() - context_->start_timestamp;
  if (is_read_from_device_scheduled_) ReadFromDeviceImpl();
}

void TapaFastCosimDevice::Kill() {
  if (context_ != nullptr) {
    context_->proc->interrupt();  // sends SIGINT, propagates to child processes
    context_ = nullptr;
    LOG(INFO) << "TAPA fast cosim process killed";
  }
}

bool TapaFastCosimDevice::IsFinished() const {
  if (context_ == nullptr || !context_->proc.has_value()) return false;
  // If Finish() already called wait(), exit_code is set — skip running().
  if (context_->exit_code.has_value()) return true;
  // running() calls waitpid(WNOHANG); if process exited it reaps the child and
  // caches exit status internally. Call wait() immediately after to retrieve it
  // before a later Finish() call would see ECHILD.
  boost::system::error_code ec;
  if (!context_->proc->running(ec)) {
    context_->exit_code = context_->proc->wait(ec);
    return true;
  }
  return false;
}

std::vector<ArgInfo> TapaFastCosimDevice::GetArgsInfo() const { return args_; }

int64_t TapaFastCosimDevice::LoadTimeNanoSeconds() const {
  return load_time_.count();
}
int64_t TapaFastCosimDevice::ComputeTimeNanoSeconds() const {
  return compute_time_.count();
}
int64_t TapaFastCosimDevice::StoreTimeNanoSeconds() const {
  return store_time_.count();
}

size_t TapaFastCosimDevice::LoadBytes() const {
  size_t total = 0;
  for (auto& [idx, arg] : buffer_table_) total += arg.SizeInBytes();
  return total;
}

size_t TapaFastCosimDevice::StoreBytes() const {
  size_t total = 0;
  for (int idx : store_indices_) total += buffer_table_.at(idx).SizeInBytes();
  return total;
}

}  // namespace internal
}  // namespace fpga
