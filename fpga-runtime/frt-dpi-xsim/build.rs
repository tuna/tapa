fn main() {
    #[cfg(target_os = "linux")]
    {
        let variant = std::env::var("TAPA_XSIM_VARIANT").unwrap_or_else(|_| "xv".into());
        if let Ok(vivado) = std::env::var("XILINX_VIVADO") {
            let lib_dir = format!("{vivado}/data/xsim/lib/lnx64.o");
            if std::path::Path::new(&lib_dir).join("libsvdpi.so").exists() {
                println!("cargo:rustc-link-search=native={lib_dir}");
                println!("cargo:rustc-link-lib=svdpi");
            }
        }
        // Allow unresolved svdpi symbols (svGetArrayPtr etc.) — they are
        // provided by the xsim runtime when the DPI library is loaded.
        println!("cargo:rustc-cdylib-link-arg=-Wl,--allow-shlib-undefined");
        println!("cargo:rustc-env=TAPA_XSIM_VARIANT={variant}");
    }
}
