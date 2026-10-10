//! Terminal interface. Rendering and input handling stay inside this module.

mod event_loop;
mod requests;
mod theme;
mod types;

pub use event_loop::run;
