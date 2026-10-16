//! Loopback HTTP tools for live fixture runs: a forwarding proxy that records
//! and can withhold responses, and a static file server for trusted skill URLs.

#[path = "harness/files.rs"]
pub mod files;
#[path = "harness/http.rs"]
pub mod http;
#[path = "harness/proxy.rs"]
pub mod proxy;
