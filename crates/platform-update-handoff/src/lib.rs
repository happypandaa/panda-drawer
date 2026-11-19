//! Credential-free parent/helper handoff. No installer, updater, network or
//! runtime entry point is provided. Every observation has install_authorized=false.
//!
//! DrainProof is an adapter contract, not a cryptographic proof. The desktop
//! adapter fixes P to its private, non-deserializable coordinator receipt.
#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
pub use macos::{
    current_parent_matches, helper_from_inherited_descriptors,
    observe_parent_exit_from_inherited_descriptors, owner_state, ArmedForParentExit,
    AwaitingParentExit, DrainProof, Error, ObservedParentExit, OwnerState, ParentEndpoint,
    SessionBinding, SessionDirectory,
};

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use windows::{
    cleanup_sessions, current_parent_matches, observe_parent_exit, observe_parent_exit_with,
    DrainProof, ObservedParentExit, ParentEndpoint, SessionBinding, SessionDirectory,
};
