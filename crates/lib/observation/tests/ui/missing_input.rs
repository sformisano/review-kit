#[review_kit_observation::operations]
mod managed {
    #[operation(name = "test.missing_input", output = |_| review_kit_observation::Completion::success(review_kit_observation::Fields::new()))]
    pub fn run() {}
}

fn main() {}
