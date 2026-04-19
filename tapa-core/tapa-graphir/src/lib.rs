//! TAPA `GraphIR` schema — serde structs for `graphir.json`.

pub mod blackbox;
pub mod expression;
pub mod interface;
pub mod module;
pub mod project;

mod error;

pub use error::ParseError;
pub use expression::{Expression, Token, TokenKind};
pub use module::definition::{
    AnyModuleDefinition, BaseFields, GroupedFields, HierarchicalName, VerilogFields,
};
pub use module::instantiation::{InstanceArea, ModuleConnection, ModuleInstantiation};
pub use module::support::{ModuleNet, ModuleParameter, ModulePort, Range};
pub use project::{Modules, Project};
