#![allow(clippy::unwrap_used, clippy::expect_used)]

#[path = "../../../lib/infrastructure/gitlab-adapter/tests/support/mod.rs"]
#[allow(
    dead_code,
    unused_imports,
    reason = "The shared HTTP fixture also exports adapter-only model helpers."
)]
mod support;

use support as server;

const TOKEN: &str = "fixture-private-secret";
const MR_PATH: &str = "/projects/11/merge_requests/7";

#[path = "render_profiles/configuration.rs"]
mod configuration;
#[path = "render_profiles/fixture.rs"]
mod fixture;
#[path = "render_profiles/paths.rs"]
mod paths;
#[path = "render_profiles/presentation.rs"]
mod presentation;
#[path = "render_profiles/recovery.rs"]
mod recovery;
#[path = "render_profiles/restoration.rs"]
mod restoration;
#[path = "render_profiles/safety.rs"]
mod safety;
