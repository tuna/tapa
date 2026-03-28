fn main() {
    #[cfg(target_os = "linux")]
    {
        let variant = std::env::var("TAPA_XSIM_VARIANT").unwrap_or_else(|_| "xv".into());
        // svGetArrayPtr is resolved at runtime via dlsym(RTLD_DEFAULT, ...)
        // from the xsim process that loads this DPI library, so no link-time
        // dependency on libsvdpi is needed.
        println!("cargo:rustc-env=TAPA_XSIM_VARIANT={variant}");
    }
}
