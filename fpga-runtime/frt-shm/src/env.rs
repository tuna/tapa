//! Centralized environment variable names for the FRT runtime.
//!
//! All `FRT_*` and `TAPA_*` env var names live here to avoid scattered string
//! literals and keep the full set discoverable in one place.

// ── Cosim runtime options (read by frt/src/cosim/mod.rs) ────────────────
pub const FRT_COSIM_SETUP_ONLY: &str = "FRT_COSIM_SETUP_ONLY";
pub const FRT_COSIM_RESUME_FROM_POST_SIM: &str = "FRT_COSIM_RESUME_FROM_POST_SIM";
pub const FRT_COSIM_WORK_DIR: &str = "FRT_COSIM_WORK_DIR";
pub const FRT_COSIM_WORK_DIR_PARALLEL: &str = "FRT_COSIM_WORK_DIR_PARALLEL";
pub const FRT_COSIM_YIELD: &str = "FRT_COSIM_YIELD";

// ── XSim-specific options ───────────────────────────────────────────────
pub const FRT_XSIM_LEGACY: &str = "FRT_XSIM_LEGACY";
pub const FRT_XSIM_START_GUI: &str = "FRT_XSIM_START_GUI";
pub const FRT_XSIM_SAVE_WAVEFORM: &str = "FRT_XSIM_SAVE_WAVEFORM";
pub const FRT_XSIM_PART_NUM: &str = "FRT_XSIM_PART_NUM";

// ── Verilator-specific options ──────────────────────────────────────────
pub const FRT_VERILATOR_BUILD_LOCK: &str = "FRT_VERILATOR_BUILD_LOCK";

// ── Diagnostics and debug ───────────────────────────────────────────────
pub const FRT_STREAM_DEBUG: &str = "FRT_STREAM_DEBUG";

// ── Shared memory ───────────────────────────────────────────────────────
pub const FRT_SHM_MIN_DEPTH: &str = "FRT_SHM_MIN_DEPTH";

// ── XRT / OpenCL device selection ───────────────────────────────────────
pub const FRT_XOCL_BDF: &str = "FRT_XOCL_BDF";
pub const XOCL_BDF: &str = "XOCL_BDF";

// ── Internal (set by runners, read by DPI libraries) ────────────────────
pub const TAPA_DPI_CONFIG: &str = "TAPA_DPI_CONFIG";
