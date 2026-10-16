#[review_kit_observation::operations]
mod managed {
    #[operation(name = "test.missing_output", input = review_kit_observation::Fields::new())]
    pub fn run() {}
}

fn main() {}
