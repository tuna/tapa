// Verilator marshalling: raw byte pointers, native `bool` flags/returns.
frt_dpi::dpi_export! {
    mut arr: *mut u8 as ::core::convert::identity;
    const arr: *const u8 as ::core::convert::identity;
    flag: bool as ::core::convert::identity;
    ret: bool as ::core::convert::identity;
}

// Floating-point DPI support for Xilinx IP behavioral models.
// Called from generated SystemVerilog via `import "DPI-C"`.
macro_rules! fp_op {
    ($name:ident, $uint:ty, $float:ty, $op:tt) => {
        #[no_mangle]
        pub extern "C" fn $name(a: $uint, b: $uint) -> $uint {
            (<$float>::from_bits(a) $op <$float>::from_bits(b)).to_bits()
        }
    };
}
fp_op!(fp32_add, u32, f32, +);
fp_op!(fp32_sub, u32, f32, -);
fp_op!(fp32_mul, u32, f32, *);
fp_op!(fp64_add, u64, f64, +);
fp_op!(fp64_sub, u64, f64, -);
fp_op!(fp64_mul, u64, f64, *);
