use std::ffi::OsStr;

pub(super) const RUNTIME_DIR: &str = ".review-kit";
pub(super) const GITIGNORE: &str = ".gitignore";
/// Ignores every runtime entry, including the ignore file itself.
pub(super) const GITIGNORE_CONTENT: &[u8] = b"*\n";
pub(super) const WORKSPACE_FILE: &str = "review-workspace.json";
pub(super) const LOCK_FILE: &str = "review-workspace.lock";
pub(super) const STAGING_DIR: &str = "staging";
pub(super) const RUNS_DIR: &str = "review-runs";
pub(super) const ACCEPTED_DIR: &str = "accepted";
pub(super) const DRAFT_HISTORY_DIR: &str = "draft-history";
pub(super) const RENDERED_DIR: &str = "rendered";
/// Reads and writes share this bound so every stored file stays readable.
pub(super) const MAX_FILE_BYTES: u64 = 64 * 1024 * 1024;

/// Temporary files of this crate's atomic writers use `.rkc-<unique>.tmp`.
const TEMPORARY_PREFIX: &str = ".rkc-";
const TEMPORARY_SUFFIX: &str = ".tmp";

#[review_kit_observation::operations]
#[helper]
pub(super) fn temporary_name(unique: &str) -> String {
    format!("{TEMPORARY_PREFIX}{unique}{TEMPORARY_SUFFIX}")
}

#[review_kit_observation::operations]
#[helper]
pub(super) fn is_temporary(name: &OsStr) -> bool {
    name.to_str().is_some_and(|name| {
        name.len() > TEMPORARY_PREFIX.len() + TEMPORARY_SUFFIX.len()
            && name.starts_with(TEMPORARY_PREFIX)
            && name.ends_with(TEMPORARY_SUFFIX)
    })
}

/// Names that `.review-kit/` may hold before `review-workspace.json` exists.
#[review_kit_observation::operations]
#[helper]
pub(super) fn is_recognized_runtime_entry(name: &OsStr) -> bool {
    [GITIGNORE, WORKSPACE_FILE, LOCK_FILE, STAGING_DIR, RUNS_DIR]
        .iter()
        .any(|known| name == *known)
        || is_temporary(name)
}
