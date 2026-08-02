//! Declarative preconditions for pipeline steps.
//!
//! Concrete prerequisites are represented explicitly in [`Precondition`] and
//! are validated by the dispatcher before the step body runs.

use std::path::Path;

use tapa_ir::WorkState;

use crate::error::{CliError, Result};

use super::pack::PackArgs;

/// Error identity used when a step requires completed synthesis.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SynthedFlowError {
    Floorplan,
    Pack,
}

/// A prerequisite checked before dispatching a step body.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Precondition {
    RequiresSynthedFlow(SynthedFlowError),
    RejectsCustomRtlAfterFloorplan,
}

/// Static prerequisite declaration for one pipeline step.
pub struct StepSpec {
    pub preconditions: &'static [Precondition],
}

const ANALYZE_SPEC: StepSpec = StepSpec { preconditions: &[] };

const SYNTH_SPEC: StepSpec = StepSpec { preconditions: &[] };

const FLOORPLAN_SPEC: StepSpec = StepSpec {
    preconditions: &[Precondition::RequiresSynthedFlow(
        SynthedFlowError::Floorplan,
    )],
};

const PACK_SPEC: StepSpec = StepSpec {
    preconditions: &[
        Precondition::RequiresSynthedFlow(SynthedFlowError::Pack),
        Precondition::RejectsCustomRtlAfterFloorplan,
    ],
};

pub fn analyze() -> &'static StepSpec {
    &ANALYZE_SPEC
}

pub fn synth() -> &'static StepSpec {
    &SYNTH_SPEC
}

pub fn floorplan() -> &'static StepSpec {
    &FLOORPLAN_SPEC
}

pub fn pack() -> &'static StepSpec {
    &PACK_SPEC
}

/// Validate a step's declared preconditions against an already-loaded state.
pub fn validate(
    spec: &StepSpec,
    state: &WorkState,
    state_path: &Path,
    args: Option<&PackArgs>,
) -> Result<()> {
    for precondition in spec.preconditions {
        match precondition {
            Precondition::RequiresSynthedFlow(error) if !state.flow.synthed => {
                return Err(missing_synthed_flow(*error, state_path));
            }
            Precondition::RejectsCustomRtlAfterFloorplan => {
                validate_pack_overlay(state, args)?;
            }
            Precondition::RequiresSynthedFlow(_) => {}
        }
    }
    Ok(())
}

fn missing_synthed_flow(error: SynthedFlowError, state_path: &Path) -> CliError {
    match error {
        SynthedFlowError::Floorplan => CliError::Floorplan(
            "run `synth` before `floorplan`: the placement needs per-task areas".to_string(),
        ),
        SynthedFlowError::Pack => CliError::MissingState {
            name: "completed synthesis (run `tapa synth` first)".to_string(),
            path: state_path.to_path_buf(),
        },
    }
}

fn validate_pack_overlay(state: &WorkState, args: Option<&PackArgs>) -> Result<()> {
    let Some(args) = args else {
        return Ok(());
    };
    if state.floorplan.is_some() && !args.custom_rtl.is_empty() {
        return Err(CliError::InvalidArg(
            "`--custom-rtl` cannot modify RTL after floorplanning; omit it or rerun synthesis and packaging without the active floorplan"
                .to_string(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    use tapa_ir::{Target, TaskGraph};

    use super::*;

    fn state(synthed: bool, floorplanned: bool) -> WorkState {
        let mut state = WorkState::new(TaskGraph {
            top: "Top".to_string(),
            target: Target::XilinxVitis,
            tasks: BTreeMap::new(),
            cflags: Vec::new(),
        });
        state.flow.synthed = synthed;
        if floorplanned {
            state.floorplan = Some(crate::testutil::mock_floorplan_result(
                "test-device",
                (1, 1),
            ));
        }
        state
    }

    fn pack_args(custom_rtl: &[&str]) -> PackArgs {
        PackArgs {
            output: None,
            bitstream_script: None,
            connectivity: None,
            custom_rtl: custom_rtl.iter().map(PathBuf::from).collect(),
        }
    }

    #[test]
    fn floorplan_synthed_flow_precondition_accepts_completed_synthesis() {
        validate(
            floorplan(),
            &state(true, false),
            Path::new("/work/tapa.json"),
            None,
        )
        .expect("completed synthesis must satisfy floorplan");
    }

    #[test]
    fn floorplan_synthed_flow_precondition_preserves_exact_error() {
        let error = validate(
            floorplan(),
            &state(false, false),
            Path::new("/work/tapa.json"),
            None,
        )
        .expect_err("floorplan must require synthesis");
        assert!(matches!(
            error,
            CliError::Floorplan(ref message)
                if message == "run `synth` before `floorplan`: the placement needs per-task areas"
        ));
    }

    #[test]
    fn pack_synthed_flow_precondition_accepts_completed_synthesis() {
        let args = pack_args(&[]);
        validate(
            pack(),
            &state(true, false),
            Path::new("/work/tapa.json"),
            Some(&args),
        )
        .expect("completed synthesis must satisfy pack");
    }

    #[test]
    fn pack_synthed_flow_precondition_preserves_exact_error() {
        let args = pack_args(&[]);
        let error = validate(
            pack(),
            &state(false, false),
            Path::new("/work/tapa.json"),
            Some(&args),
        )
        .expect_err("pack must require synthesis");
        assert!(matches!(
            error,
            CliError::MissingState { ref name, ref path }
                if name == "completed synthesis (run `tapa synth` first)"
                    && path == Path::new("/work/tapa.json")
        ));
    }

    #[test]
    fn pack_overlay_precondition_accepts_non_conflicting_states() {
        let no_overlay = pack_args(&[]);
        validate(
            pack(),
            &state(true, true),
            Path::new("/work/tapa.json"),
            Some(&no_overlay),
        )
        .expect("floorplan without custom RTL must be accepted");

        let overlay = pack_args(&["replacement.v"]);
        validate(
            pack(),
            &state(true, false),
            Path::new("/work/tapa.json"),
            Some(&overlay),
        )
        .expect("custom RTL without a floorplan must be accepted");
    }

    #[test]
    fn pack_overlay_precondition_preserves_exact_error() {
        let args = pack_args(&["replacement.v"]);
        let error = validate(
            pack(),
            &state(true, true),
            Path::new("/work/tapa.json"),
            Some(&args),
        )
        .expect_err("custom RTL must not modify floorplanned RTL");
        assert!(matches!(
            error,
            CliError::InvalidArg(ref message)
                if message == "`--custom-rtl` cannot modify RTL after floorplanning; omit it or rerun synthesis and packaging without the active floorplan"
        ));
    }
}
