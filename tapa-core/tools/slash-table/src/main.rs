//! Development-only census of a SLASH static shell. Does not register tables.

mod bd;
mod census;

use std::error::Error;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use clap::{Parser, ValueEnum};
use serde::Serialize;
use tapa_xilinx::runtime::process::{LocalToolRunner, ToolInvocation, ToolRunner};

type Result<T> = std::result::Result<T, Box<dyn Error>>;

#[derive(Clone, Copy, Debug, Serialize, ValueEnum)]
#[serde(rename_all = "snake_case")]
enum Shell {
    Service,
    Compute,
}

impl Shell {
    fn name(self) -> &'static str {
        match self {
            Self::Service => "service",
            Self::Compute => "compute",
        }
    }
}

#[derive(Parser)]
#[command(about = "Generate reviewable TAPA tables from a SLASH static DCP and its top-level BD")]
struct Args {
    /// Routed `static_shell_slash.dcp` with its RM black-boxed.
    #[arg(long)]
    checkpoint: PathBuf,
    /// Matching top.bd from the static-shell build, not `slash_base.bd`.
    #[arg(long)]
    block_design: PathBuf,
    #[arg(long, value_enum)]
    shell: Shell,
    /// New output directory; existing results are never overwritten.
    #[arg(long)]
    output: PathBuf,
    #[arg(long, default_value = "pblock_slash")]
    pblock: String,
    #[arg(long, default_value = "vivado")]
    vivado: String,
}

#[derive(Serialize)]
struct Manifest {
    shell: Shell,
    checkpoint: PathBuf,
    block_design: PathBuf,
    part_num: String,
    vivado_version: String,
    bd_version: String,
    rm_cell: String,
    pblock: String,
    pblock_ranges: Vec<String>,
    interfaces: Vec<bd::Interface>,
    clocks: Vec<bd::Clock>,
}

fn main() -> std::process::ExitCode {
    match run(Args::parse()) {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("slash-table: {error}");
            std::process::ExitCode::FAILURE
        }
    }
}

fn run(args: Args) -> Result<()> {
    let checkpoint = fs::canonicalize(&args.checkpoint)?;
    let block_design = fs::canonicalize(&args.block_design)?;
    let bd = bd::read(&block_design)?;
    fs::create_dir(&args.output)?;
    let output = fs::canonicalize(&args.output)?;
    let script = output.join("collect.tcl");
    fs::write(&script, include_str!("collect.tcl"))?;
    bd.write_requests(&output)?;
    let mut invocation = ToolInvocation::new(args.vivado)
        .arg("-mode")
        .arg("batch")
        .arg("-source")
        .arg(utf8(&script)?)
        .arg("-log")
        .arg("vivado.log")
        .arg("-journal")
        .arg("vivado.jou")
        .arg("-tclargs")
        .arg(utf8(&checkpoint)?)
        .arg(args.pblock.clone());
    invocation.cwd = Some(utf8(&output)?.into());
    eprintln!(
        "Collecting shell data with Vivado; logs: {}",
        output.display()
    );
    let result = LocalToolRunner::new().run(&invocation)?;
    fs::write(output.join("vivado.stdout"), &result.stdout)?;
    fs::write(output.join("vivado.stderr"), &result.stderr)?;
    if result.exit_code != 0 {
        return Err(format!(
            "Vivado failed; inspect {}",
            output.join("vivado.log").display()
        )
        .into());
    }
    let census = census::read(&output)?;
    if census.part != bd.part {
        return Err(format!("DCP part {} differs from BD part {}", census.part, bd.part).into());
    }
    let (interfaces, clocks) = bd.describe(&census)?;
    let device = census.device(args.shell, &interfaces, &args.pblock)?;
    device.validate()?;
    let manifest = Manifest {
        shell: args.shell,
        checkpoint,
        block_design,
        part_num: census.part.clone(),
        vivado_version: census.vivado.clone(),
        bd_version: bd.version,
        rm_cell: census.rm_cell.clone(),
        pblock: args.pblock,
        pblock_ranges: census.pblock_ranges,
        interfaces,
        clocks,
    };
    write_json(&output.join("device.json"), &device)?;
    write_json(&output.join("shell.json"), &manifest)?;
    println!(
        "Generated {} for review; no built-in table was changed.",
        output.display()
    );
    Ok(())
}

fn utf8(path: &Path) -> Result<&str> {
    path.to_str()
        .ok_or_else(|| format!("non-UTF-8 path: {}", path.display()).into())
}

fn write_json(path: &Path, value: &impl Serialize) -> Result<()> {
    let mut value = serde_json::to_value(value)?;
    value.sort_all_objects();
    let mut file = fs::File::create_new(path)?;
    let formatter = serde_json::ser::PrettyFormatter::with_indent(b"    ");
    let mut serializer = serde_json::Serializer::with_formatter(&mut file, formatter);
    value.serialize(&mut serializer)?;
    file.write_all(b"\n")?;
    Ok(())
}
