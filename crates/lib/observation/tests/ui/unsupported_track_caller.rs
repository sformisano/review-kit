#[review_kit_observation::operations]
mod managed {
    #[track_caller]
    #[operation(name = "test.caller", input = review_kit_observation::Fields::new(), output = |_| review_kit_observation::Completion::success(review_kit_observation::Fields::new()))]
    pub fn caller() -> &'static std::panic::Location<'static> {
        std::panic::Location::caller()
    }
}

fn main() {}
