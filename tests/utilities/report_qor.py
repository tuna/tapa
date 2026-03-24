import logging
from glob import glob
from os import environ

import click

_logger = logging.getLogger(__name__)

GITHUB_JOB_SUMMARY = environ.get("GITHUB_STEP_SUMMARY", "/tmp/github-job-summary")


@click.command()
@click.option(
    "--run-dir",
    type=click.Path(file_okay=False, resolve_path=True),
    help="The path to a (set of) run(s).",
    required=True,
)
def report_qor(run_dir: str) -> None:
    """Report the QoRs of an implemented design."""
    report_freq(run_dir)


def report_freq(run_dir: str) -> None:
    """Report the Fmax of an implemented design."""
    _logger.warning("Regression metrics are stored in %s", GITHUB_JOB_SUMMARY)

    with open(GITHUB_JOB_SUMMARY, "a", encoding="utf-8") as log_f:
        for sol_dir in glob(f"{run_dir}/dse/solution_*"):
            log_f.write(f"\n\n## Solution: {sol_dir}\n\n")
            _logger.warning("## Solution: %s", sol_dir)

            timing_rpt_g = glob(
                f"{sol_dir}/**/link/vivado/vpl/**/*_timing_summary_postroute_physopted.rpt",
                recursive=True,
            )
            if not timing_rpt_g:
                log_f.write("No timing report found.\n")
                _logger.critical("No timing report found for %s", sol_dir)
                continue
            if len(timing_rpt_g) > 1:
                log_f.write("Multiple timing reports found.\n")
                _logger.critical("Multiple timing reports found for %s", sol_dir)
                continue

            with open(timing_rpt_g[0], encoding="utf-8") as timing_rpt_f:
                lines = timing_rpt_f.readlines()

            wns_header = next(
                i for i, line in enumerate(lines) if line.lstrip().startswith("WNS(ns)")
            )
            wns = float(lines[wns_header + 2].split()[0])

            run_vitis_sh_g = glob(f"{sol_dir}/run_vitis.sh")
            assert len(run_vitis_sh_g) == 1, (
                f"Expected one run_vitis.sh, now: {run_vitis_sh_g}."
            )

            with open(run_vitis_sh_g[0], encoding="utf-8") as vitis_script:
                target_line = next(
                    line
                    for line in vitis_script
                    if line.strip().startswith("TARGET_FREQUENCY")
                )
            target = float(target_line.split("=")[1])
            assert target != 0

            fmax = 1000 / ((1000 / target) - wns)
            log_f.write(f"Fmax: {fmax:.2f}\n")
            _logger.warning("Fmax: %.2f", fmax)


if __name__ == "__main__":
    report_qor().main(standalone_mode=False)  # pylint: disable=E1120
