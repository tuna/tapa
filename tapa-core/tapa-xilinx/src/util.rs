//! Small shared path and template helpers.

use camino::Utf8PathBuf;

use crate::error::{Result, XilinxError};

/// Lossy conversion to `Utf8PathBuf` (TAPA paths are UTF-8 in
/// practice; a non-UTF-8 path degrades to its lossy form rather than
/// failing the whole operation).
pub fn utf8(p: impl AsRef<std::path::Path>) -> Utf8PathBuf {
    Utf8PathBuf::from(p.as_ref().to_string_lossy().into_owned())
}

/// Absolute form of `p`: already-absolute paths pass through; relative
/// paths are joined onto the current directory and canonicalized when
/// they exist (a not-yet-created target keeps the plain joined form).
pub fn absolutize(p: &camino::Utf8Path) -> Utf8PathBuf {
    if p.is_absolute() {
        return p.to_path_buf();
    }
    let joined = std::env::current_dir()
        .map_or_else(|_| Utf8PathBuf::from(".").join(p), |cwd| utf8(cwd).join(p));
    std::fs::canonicalize(&joined).map_or_else(|_| joined.clone(), utf8)
}

/// Render the minijinja template `src` (registered under `name`) with
/// `ctx`, sharing the `Environment::new` -> `add_template` ->
/// `get_template` -> `render` boilerplate across the TCL/XML emitters.
/// Template engines keep stock settings (`Environment::new`, no custom
/// filters or autoescape changes); parse/render failures surface as
/// [`XilinxError::Template`] instead of panicking.
pub fn render_template(name: &str, src: &str, ctx: impl serde::Serialize) -> Result<String> {
    let mut env = minijinja::Environment::new();
    env.add_template(name, src)
        .map_err(|e| XilinxError::Template(format!("parse `{name}`: {e}")))?;
    let tmpl = env
        .get_template(name)
        .map_err(|e| XilinxError::Template(format!("load `{name}`: {e}")))?;
    tmpl.render(ctx)
        .map_err(|e| XilinxError::Template(format!("render `{name}`: {e}")))
}
