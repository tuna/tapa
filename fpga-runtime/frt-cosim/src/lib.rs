pub mod context;
pub mod control;
pub mod error;
pub mod metadata;
pub mod runner;
pub mod tb;

pub(crate) fn has_ext(path: &std::path::Path, exts: &[&str], case_insensitive: bool) -> bool {
    let Some(ext) = path.extension().and_then(|e| e.to_str()) else {
        return false;
    };
    exts.iter().any(|x| {
        if case_insensitive {
            ext.eq_ignore_ascii_case(x)
        } else {
            ext == *x
        }
    })
}
