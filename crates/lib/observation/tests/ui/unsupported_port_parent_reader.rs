#[review_kit_observation::observed_port(
    wrapper = Managed,
    ownership = "boxed",
    reader = super::ManagedReader
)]
trait Raw {}

fn main() {}
