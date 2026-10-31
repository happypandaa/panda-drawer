//! Explicit inherited/cleared environment policy for native process creation.
//! A stable std Command exposes overrides, but not its env_clear flag. Never
//! infer that flag from an empty override list or probe the global environment.
use tokio::process::Command;

pub(crate) struct OwnedCommand {
    pub(super) command: Command,
    pub(super) environment: Environment,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Environment {
    Unspecified,
    Inherit,
    Clear,
}

impl OwnedCommand {
    /// The actual command builder retained inheritance. Apply its explicit
    /// env/env_remove changes over a snapshot of the native parent environment.
    pub(crate) fn inherited(command: Command) -> Self {
        Self {
            command,
            environment: Environment::Inherit,
        }
    }

    /// The actual command builder called env_clear. Only its explicit env
    /// entries may enter the child; this wrapper does not change that policy.
    pub(crate) fn cleared(command: Command) -> Self {
        Self {
            command,
            environment: Environment::Clear,
        }
    }
}

impl From<Command> for OwnedCommand {
    fn from(command: Command) -> Self {
        Self {
            command,
            // Mac forwards the original Command, including its private policy.
            // Native Windows creation must explicitly reject this unknown
            // sidecar before creating a writer marker or a child.
            environment: Environment::Unspecified,
        }
    }
}
