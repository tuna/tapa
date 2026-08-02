//! Child task instantiation with FSM/port wiring: per-instance FSM
//! generation, argument pipelines, handshake wiring, and portarg
//! assembly.

mod fsm;
mod instance;
mod signals;

pub use instance::ChildMmapBindings;
pub use signals::generate_child_signals;
