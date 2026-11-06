//! Native source acquisition. Shared data and URL policy never perform I/O;
//! unfinished native acquisition fails before accepting work.
#[path = "source_acquisition/inspection.rs"]
mod inspection;
mod download;
#[path = "source_publication/selected_directory.rs"]
pub(crate) mod selected_directory;
#[path = "source_acquisition/types.rs"]
mod types;
#[path = "source_acquisition/url.rs"]
mod url;
pub use types::{AcquireSourceInput, SourceCandidate, SourceInspection};
pub use selected_directory::{inspect_selected_directory, SelectedDirectory};
pub(crate) use selected_directory::validate_selected_directory;
pub(crate) use url::{validate_branch, validate_url};
#[cfg(unix)]
#[path = "source_acquisition/unix.rs"]
mod unix;
#[cfg(windows)]
#[path = "source_acquisition/windows.rs"]
mod windows;
#[cfg(unix)]
pub use unix::SourceAcquisitionManager;
#[cfg(windows)]
pub use windows::SourceAcquisitionManager;
