#[review_kit_observation::operations]
mod managed {
    #[operation(name = "test.run", input = review_kit_observation::Fields::new(), output = |_| review_kit_observation::Completion::success(review_kit_observation::Fields::new()))]
    pub fn run() {}
}

fn main() {
    managed::run_raw();
}
