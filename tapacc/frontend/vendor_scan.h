#ifndef TAPA_FRONTEND_VENDOR_SCAN_H_
#define TAPA_FRONTEND_VENDOR_SCAN_H_

namespace clang {
class ASTContext;
class Preprocessor;
}  // namespace clang

namespace tapa::cc {

// Vendor-usage soft warnings ("tapa analyze" remarks, decision: warn, never
// fail): scan a translation unit for constructs that tie a program to a
// specific vendor — vendor headers, vendor pragmas, vendor wait intrinsics —
// and point at the portable TAPA alternative for each. AttachVendorScan is
// called once per frontend action (preprocessor phase: includes and pragmas);
// ScanVendorAsts is called once the AST is built (wait-intrinsic calls).
// Both only report locations in the user's own (non-system) source.

// Registers the preprocessor hooks. Call from BeginSourceFileAction.
void AttachVendorScan(clang::Preprocessor& pp);

// Reports vendor wait-intrinsic calls in the built translation unit.
void ScanVendorAsts(clang::ASTContext& ctx);

}  // namespace tapa::cc

#endif  // TAPA_FRONTEND_VENDOR_SCAN_H_
