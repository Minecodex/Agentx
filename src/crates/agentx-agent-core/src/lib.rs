//! Infrastructure-free Agent Core for Plan3 P3-02.
//!
//! The crate deliberately contains no Worker adapter. Model, tool, state,
//! event, clock, and budget effects can only cross the port traits; the
//! Workflow Worker owns the concrete Provider, State and Lease adapters.

mod context;
mod contracts;
mod driver;
mod message;
mod operation;
mod ports;
mod session;
mod session_store;
mod tool;

pub use context::*;
pub use contracts::*;
pub use driver::*;
pub use message::*;
pub use operation::*;
pub use ports::*;
pub use session::*;
pub use session_store::*;
pub use tool::*;

pub const CORE_CONTRACT_VERSION: &str = "1.2";
pub const PI_REFERENCE_REPOSITORY: &str = "https://github.com/earendil-works/pi";
pub const PI_REFERENCE_COMMIT: &str = "a69bef789bc95abf0acee16f7b4660b70b650bb9";
pub const PI_REFERENCE_PACKAGE_VERSION: &str = "0.84.2";
