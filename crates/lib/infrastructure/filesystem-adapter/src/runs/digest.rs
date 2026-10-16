use review_kit_application::review_run::{ContentRef, InvalidContentRef};
use sha2::{Digest, Sha256};

use super::RunStorageError;

/// The `sha256:` reference of exact bytes.
///
/// A lowercase SHA-256 digest is always a valid reference; the error exists
/// because [`ContentRef`] offers only validating construction.
#[review_kit_observation::operations]
#[helper]
pub fn content_ref(bytes: &[u8]) -> Result<ContentRef, InvalidContentRef> {
    ContentRef::from_hex(&format!("{:x}", Sha256::digest(bytes)))
}

#[review_kit_observation::operations]
#[helper]
pub(super) fn digest(bytes: &[u8]) -> Result<ContentRef, RunStorageError> {
    content_ref(bytes).map_err(RunStorageError::Digest)
}
