// Shared mismatch-reporting helpers for the test host programs.
//
// Every test host counts result mismatches the same way: report the first
// kErrorReportThreshold of them in detail, note when the threshold is
// crossed, count the rest silently, and finish with a PASS!/FAIL! verdict
// that is also the process exit code. These macros keep that contract in one
// place; the per-test loop, expected-value formula, and comparison stay at
// the call site.

#ifndef TESTS_HOST_UTIL_H_
#define TESTS_HOST_UTIL_H_

#include <cstdint>
#include <ostream>

namespace tapa_testing {

// Only report up to this many errors per run.
inline constexpr uint64_t kErrorReportThreshold = 10;

}  // namespace tapa_testing

// Records one mismatch in `num_errors`: logs
// "<prefix>expected: <expected>, actual: <actual>" through `log` for the
// first tapa_testing::kErrorReportThreshold mismatches, "..." when the
// threshold is crossed, and nothing afterwards. A macro so that a glog
// expression (e.g. LOG(ERROR)) passed as `log` keeps the call site's
// file/line and is only evaluated when a line is actually written.
#define TAPA_TEST_RECORD_MISMATCH(num_errors, log, prefix, expected, actual)  \
  do {                                                                        \
    if ((num_errors) < ::tapa_testing::kErrorReportThreshold) {               \
      log << prefix << "expected: " << (expected) << ", actual: " << (actual) \
          << std::endl;                                                       \
    } else if ((num_errors) == ::tapa_testing::kErrorReportThreshold) {       \
      log << "...";                                                           \
    }                                                                         \
    ++(num_errors);                                                           \
  } while (0)

// Logs the PASS!/FAIL! verdict for `num_errors` mismatches through
// `verdict_log`, with the overflow summary (" (+N more errors)") through
// `overflow_log` when the count exceeds the threshold, and returns the
// process exit code from the enclosing function.
#define TAPA_TEST_RETURN_VERDICT(num_errors, verdict_log, overflow_log)    \
  do {                                                                     \
    if ((num_errors) == 0) {                                               \
      verdict_log << "PASS!" << std::endl;                                 \
      return 0;                                                            \
    }                                                                      \
    if ((num_errors) > ::tapa_testing::kErrorReportThreshold) {            \
      overflow_log << " (+"                                                \
                   << (num_errors) - ::tapa_testing::kErrorReportThreshold \
                   << " more errors)" << std::endl;                        \
    }                                                                      \
    verdict_log << "FAIL!" << std::endl;                                   \
    return 1;                                                              \
  } while (0)

#endif  // TESTS_HOST_UTIL_H_
