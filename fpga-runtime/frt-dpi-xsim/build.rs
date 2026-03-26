fn main() {
    #[cfg(target_os = "linux")]
    {
        let variant = std::env::var("TAPA_XSIM_VARIANT").unwrap_or_else(|_| "xv".into());
        if let Ok(vivado) = std::env::var("XILINX_VIVADO") {
            let lib_dir = format!("{vivado}/data/xsim/lib/lnx64.o");
            println!("cargo:rustc-link-search=native={lib_dir}");
            println!("cargo:rustc-link-lib=svdpi");
        } else {
            println!(
                "cargo:warning=XILINX_VIVADO not set; building frt-dpi-xsim without svdpi linkage"
            );
        }
        println!("cargo:rustc-env=TAPA_XSIM_VARIANT={variant}");
    }
}
