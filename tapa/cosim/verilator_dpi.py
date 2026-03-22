"""DPI-C support code for Verilator cosimulation."""


def generate_dpi_support() -> str:
    """Generate C++ file with DPI-C behavioral models for Xilinx IPs."""
    return """\
#include <cstdint>
#include <cstring>

extern "C" {

// IEEE 754 single-precision floating-point addition
unsigned int fp32_add(unsigned int a, unsigned int b) {
    float fa, fb, fc;
    memcpy(&fa, &a, sizeof(float));
    memcpy(&fb, &b, sizeof(float));
    fc = fa + fb;
    unsigned int result;
    memcpy(&result, &fc, sizeof(unsigned int));
    return result;
}

// IEEE 754 single-precision floating-point subtraction
unsigned int fp32_sub(unsigned int a, unsigned int b) {
    float fa, fb, fc;
    memcpy(&fa, &a, sizeof(float));
    memcpy(&fb, &b, sizeof(float));
    fc = fa - fb;
    unsigned int result;
    memcpy(&result, &fc, sizeof(unsigned int));
    return result;
}

// IEEE 754 single-precision floating-point multiplication
unsigned int fp32_mul(unsigned int a, unsigned int b) {
    float fa, fb, fc;
    memcpy(&fa, &a, sizeof(float));
    memcpy(&fb, &b, sizeof(float));
    fc = fa * fb;
    unsigned int result;
    memcpy(&result, &fc, sizeof(unsigned int));
    return result;
}

// IEEE 754 double-precision floating-point addition
unsigned long long fp64_add(unsigned long long a, unsigned long long b) {
    double da, db, dc;
    memcpy(&da, &a, sizeof(double));
    memcpy(&db, &b, sizeof(double));
    dc = da + db;
    unsigned long long result;
    memcpy(&result, &dc, sizeof(unsigned long long));
    return result;
}

// IEEE 754 double-precision floating-point subtraction
unsigned long long fp64_sub(unsigned long long a, unsigned long long b) {
    double da, db, dc;
    memcpy(&da, &a, sizeof(double));
    memcpy(&db, &b, sizeof(double));
    dc = da - db;
    unsigned long long result;
    memcpy(&result, &dc, sizeof(unsigned long long));
    return result;
}

// IEEE 754 double-precision floating-point multiplication
unsigned long long fp64_mul(unsigned long long a, unsigned long long b) {
    double da, db, dc;
    memcpy(&da, &a, sizeof(double));
    memcpy(&db, &b, sizeof(double));
    dc = da * db;
    unsigned long long result;
    memcpy(&result, &dc, sizeof(unsigned long long));
    return result;
}

}  // extern "C"
"""
