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

#[no_mangle]
pub extern "C" fn fp32_unfused_madd(a: u32, b: u32, c: u32) -> u32 {
    let product = (f32::from_bits(a) * f32::from_bits(b)).to_bits();
    (f32::from_bits(product) + f32::from_bits(c)).to_bits()
}

#[cfg(test)]
mod tests {
    #[test]
    fn unfused_multiply_add_rounds_the_product_before_addition() {
        let result = super::fp32_unfused_madd(0x3f80_0001, 0x3f7f_fffe, 0xbf80_0000);
        assert_eq!(result, 0);
    }
}
