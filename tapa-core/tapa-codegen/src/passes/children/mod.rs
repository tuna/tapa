//! Child task instantiation with FSM/port wiring: per-instance FSM
//! generation, argument pipelines, handshake wiring, and portarg
//! assembly.

mod fsm;
mod instance;
mod signals;

pub use fsm::{
    generate_autorun_start, generate_child_fsm, generate_is_done_assign, generate_start_assign,
    STATE_DONE, STATE_IDLE, STATE_RUNNING, STATE_WAITING,
};
pub use instance::{build_child_instance, mmap_wire_prefix, ChildMmapBindings};
pub(crate) use signals::generate_child_signals;
