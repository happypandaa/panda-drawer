//! Credential-free parent/helper handoff. No installer, updater, network or
//! runtime entry point is provided. Every observation has install_authorized=false.
//!
//! DrainProof is an adapter contract, not a cryptographic proof. The desktop
//! adapter fixes P to its private, non-deserializable coordinator receipt.
#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
pub use macos::{
    current_parent_matches, helper_from_inherited_descriptors, owner_state, ArmedForParentExit,
    AwaitingParentExit, DrainProof, Error, OwnerState, ParentEndpoint, SessionBinding,
    SessionDirectory,
};
