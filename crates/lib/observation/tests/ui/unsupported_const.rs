#[review_kit_observation::operations]
impl Counter {
    #[helper]
    pub const fn value(&self) -> usize { 1 }
}

struct Counter;
fn main() {}
