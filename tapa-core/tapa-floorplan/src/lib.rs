//! Coarse-grained floorplanning and latency-insensitive pipeline planning for
//! TAPA dataflow designs on multi-die AMD FPGAs.
//!
//! The planner assigns each flattened task/FIFO instance to a physical *slot*
//! on a rows×cols grid (rows = SLRs) by solving a wire-crossing-minimizing ILP
//! under per-slot resource and per-boundary wire-capacity constraints, then
//! plans register pipelining for every channel that crosses a slot boundary.
//! Its output is a [`tapa_ir::FloorplanResult`], the plain-data contract
//! codegen consumes.
//!
//! Module map (built out across phases):
//! - [`device`] — device model (`Area`/`Coor`/`Slot`/`Device`), embedded
//!   per-part JSON tables, and `part_num → Device` selection.
//! - `solver` — an `LpModel` + CPLEX-LP writer + `Solver` trait, with a first
//!   backend that spawns the external `cbc` binary.
//! - `graph`/`partition` — the `FloorGraph` and the floorplan ILP.
//! - `route`/`pipeline` — inter-slot routing and the pipeline plan.
//! - `xdc` — pblock/anchor XDC emission from a `FloorplanResult`.

pub mod device;
pub mod graph;
pub mod solver;
