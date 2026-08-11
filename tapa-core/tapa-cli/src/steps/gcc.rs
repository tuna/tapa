//! `tapa g++` — invokes `g++` with TAPA include and link flags.

use std::path::PathBuf;
use std::process::Command;

use clap::Parser;

use crate::context::CliContext;
use crate::error::Result;
use crate::tapacc::cflags::{get_tapa_cflags, get_tapa_ldflags};

#[derive(Debug, Parser)]
#[command(
    name = "g++",
    about = "Invoke g++ with TAPA include and library paths."
)]
pub struct GccArgs {
    /// Run the specified executable instead of `g++`.
    #[arg(long = "executable", default_value = "g++")]
    pub executable: PathBuf,

    /// Pass-through arguments forwarded to `g++` verbatim.
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    pub argv: Vec<String>,
}

pub fn run(args: &GccArgs, _ctx: &CliContext) -> Result<()> {
    let mut cmd = Command::new(&args.executable);
    cmd.arg("-std=c++17");
    cmd.arg("-DHLS_NO_XIL_FPO_LIB");
    cmd.args(get_tapa_cflags());

    if let Some(include) = crate::util::vendor_hls_root().map(|r| r.join("include")) {
        cmd.arg(format!("-isystem{}", include.display()));
    }

    cmd.args(&args.argv);
    cmd.args(get_tapa_ldflags());

    // Documented behaviour: the wrapper shows the compiler invocation it
    // composed, so a user can reproduce or adapt the build by hand.
    log::info!("{}", format_command(&cmd));

    let status = cmd.status()?;
    if !status.success() {
        std::process::exit(status.code().unwrap_or(1));
    }
    Ok(())
}

/// Render a command as a copy-pasteable shell line.
fn format_command(cmd: &Command) -> String {
    let mut out = shell_quote(&cmd.get_program().to_string_lossy());
    for arg in cmd.get_args() {
        out.push(' ');
        out.push_str(&shell_quote(&arg.to_string_lossy()));
    }
    out
}

/// Single-quote an argument unless it is made only of characters every shell
/// leaves alone.
fn shell_quote(arg: &str) -> String {
    let safe = !arg.is_empty()
        && arg
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_=+:,./@".contains(c));
    if safe {
        arg.to_owned()
    } else {
        format!("'{}'", arg.replace('\'', r"'\''"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_command_quotes_only_what_needs_it() {
        let mut cmd = Command::new("g++");
        cmd.args(["-std=c++17", "-isystem/opt/tapa/usr/include", "my file.cpp"]);
        assert_eq!(
            format_command(&cmd),
            "g++ -std=c++17 -isystem/opt/tapa/usr/include 'my file.cpp'"
        );
    }

    #[test]
    fn format_command_escapes_embedded_quotes() {
        let mut cmd = Command::new("g++");
        cmd.arg("-DNAME=\"it's\"");
        assert_eq!(format_command(&cmd), r#"g++ '-DNAME="it'\''s"'"#);
    }
}
