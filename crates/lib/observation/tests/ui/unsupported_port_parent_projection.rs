fn input() -> review_kit_observation::Fields {
    review_kit_observation::Fields::new()
}

mod ports {
    #[review_kit_observation::observed_port(wrapper = Managed, ownership = "borrowed")]
    trait Raw {
        #[operation(name = "relative", input = super::input(), output = |_| review_kit_observation::Completion::success(review_kit_observation::Fields::new()))]
        fn read(&self);
    }
}

fn main() {}
