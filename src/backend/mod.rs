//! Everything that touches disk, processes or the network, the Rust port of
//! the old Go `internal/` packages. Nothing here knows about the UI.

pub mod images;
pub mod history;
pub mod index;
pub mod latex;
pub mod notes;
pub mod document;
pub mod embedded;
pub mod outline;
pub mod papers;
pub mod pdf;
pub mod project;
pub mod references;
pub mod search;
pub mod synctex;
pub mod templates;
pub mod texparse;
pub mod todos;
pub mod watch;
