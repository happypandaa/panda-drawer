//! NTFS read-only permission is a native fact, never a fabricated Unix mode.
use crate::{
    file_identity::PersistedIdentity as FileIdentity,
    projects::data_restore_registration::strict_identity,
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct FileProof {
    #[serde(deserialize_with = "strict_identity::deserialize")]
    pub(crate) identity: FileIdentity,
    pub(crate) sha256: String,
    pub(crate) bytes: u64,
    pub(crate) read_only: bool,
}

impl FileProof {
    pub(crate) fn matches(&self, observed: &Self) -> bool {
        self.identity.matches_captured(&observed.identity)
            && self.sha256 == observed.sha256
            && self.bytes == observed.bytes
            && self.read_only == observed.read_only
    }
}
