pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

#[cfg(test)]
mod tests {
    #[test]
    fn version_matches_the_manifest() {
        assert_eq!(super::version(), "0.1.0");
    }
}
