//! nnx-core: the Unified Project Model (single source of truth) for NanoNexus.
//!
//! All edits go through [`Command`]s grouped into atomic transactions on a
//! [`Document`], which provides undo/redo. [`persist`] reads and writes `.nnx` files.

pub mod analysis;
pub mod audits;
pub mod certificate;
pub mod compliance;
pub mod boq;
pub mod briefai;
pub mod building;
pub mod clash;
pub mod command;
pub mod drawing;
pub mod dxf;
pub mod elevation;
pub mod expr;
pub mod history;
pub mod ifc;
pub mod insulation;
pub mod layout;
pub mod mep;
pub mod model;
pub mod malath;
pub mod materials;
pub mod model3d;
pub mod persist;
pub mod printdoc;
pub mod project;
pub mod presets;
pub mod qto;
pub mod rebar;
pub mod report;
pub mod routing;
pub mod rules;
pub mod sbc;
pub mod furniture;
pub mod revisions;
pub mod site;
pub mod site_layout;
pub mod sizing;
pub mod style;
pub mod templates;
pub mod thermal;
pub mod vertical;
pub mod schedule;
pub mod structural;
pub mod typed;
pub mod xlsx;
pub mod walls;

pub use command::{Command, CommandError, Origin};
pub use history::{Document, TxError};
pub use model::{Discipline, Element, Id, Model, Source};
