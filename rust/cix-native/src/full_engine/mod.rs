//! Native specialist ports under qualification.
//!
//! These modules expose exact transforms and explicit backend interfaces. They
//! are not admitted to CLI selection until their reference parity, resource
//! contracts and archive-only restoration have been qualified.
pub mod address_relations;
pub mod arithmetic;
pub mod backend_provider;
pub mod bounded_values;
pub mod catalogue;
pub mod conditional_values;
pub mod containers;
pub mod dispatch;
pub(crate) mod dynamic_library;
pub mod graph_residuals;
pub mod grid_context;
pub mod grid_counts;
pub mod installed;
pub mod interval_context;
pub mod io;
pub mod job_protocol;
pub mod jxl_provider;
pub mod mixed;
pub mod mixed_selection;
pub mod native_portfolio;
pub mod record_ordering;
pub mod regions;
pub mod selection;
pub mod spatial_frames;
pub mod spatial_provider;
pub mod strided_values;
pub mod structured_frames;
pub mod volume_lifting;
pub mod window_stream;
