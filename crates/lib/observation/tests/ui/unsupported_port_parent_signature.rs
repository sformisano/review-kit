struct Value;

mod ports {
    #[review_kit_observation::observed_port(wrapper = Managed, ownership = "borrowed")]
    trait Raw {
        #[helper]
        fn read(&self) -> super::Value;
    }
}

fn main() {}
