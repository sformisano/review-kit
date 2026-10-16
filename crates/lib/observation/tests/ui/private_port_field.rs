#[review_kit_observation::observed_port(wrapper = Managed, ownership = "borrowed")]
trait Port {
    #[helper]
    fn value(&self) -> usize;
}

struct Implementation;
impl Port for Implementation {
    fn value(&self) -> usize { 1 }
}

fn main() {
    let mut implementation = Implementation;
    let managed = Managed::new(&mut implementation);
    let _ = managed.inner;
}
