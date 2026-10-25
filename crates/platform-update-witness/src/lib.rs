//! A macOS kernel witness that the registered helper's actual parent exited.
//!
//! This does not authorize installation, authenticate an application, prove a
//! clean shutdown or close windows/descendants. Nothing is wired to a runtime.

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
pub use macos::{
    ParentExitEvidence, ParentExitWitness, ProcessIdentity, RegistrationReceipt, SessionNonce,
    WaitOutcome, WitnessError, WitnessErrorKind,
};
