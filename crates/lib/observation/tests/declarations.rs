#[test]
fn invalid_declarations_and_raw_bypasses_fail_compilation() {
    trybuild::TestCases::new().compile_fail("tests/ui/*.rs");
}
