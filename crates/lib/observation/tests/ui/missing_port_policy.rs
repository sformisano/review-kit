#[review_kit_observation::observed_port(wrapper = Managed, ownership = "borrowed")]
trait Port {
    fn unchecked(&mut self) {}
}

fn main() {}
