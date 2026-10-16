#[review_kit_observation::observed_port(wrapper = Managed, ownership = "borrowed")]
trait Port {
    #[cfg_attr(all(), track_caller)]
    #[operation(name = "test.caller", input = review_kit_observation::Fields::new(), output = |_| review_kit_observation::Completion::success(review_kit_observation::Fields::new()))]
    fn caller(&self) -> &'static std::panic::Location<'static>;
}

fn main() {}
