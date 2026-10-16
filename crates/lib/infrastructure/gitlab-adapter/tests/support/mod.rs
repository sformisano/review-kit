mod model;
mod server;

pub use model::{Renderer, finding, request, revision, transition};
pub use server::{Fault, Fixture, Response};

pub const TOKEN: &str = "fixture-private-secret";
pub const MR_PATH: &str = "/projects/11/merge_requests/7";

pub use review_kit_application::use_cases::publishing::PublicationProgress;
