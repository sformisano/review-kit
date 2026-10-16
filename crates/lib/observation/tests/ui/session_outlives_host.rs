#[review_kit_observation::observed_port(wrapper = Session, ownership = "boxed")]
trait RawSession {
    #[helper]
    fn value(&self) -> usize;
}

#[review_kit_observation::observed_port(wrapper = Host, ownership = "borrowed")]
trait RawHost {
    #[operation(
        name = "test.acquire",
        input = review_kit_observation::Fields::new(),
        output = |_| review_kit_observation::Completion::success(review_kit_observation::Fields::new()),
        returns = Session<'_>,
        map = Session::new
    )]
    fn acquire(&mut self) -> Box<dyn RawSession + '_>;
}

struct Provider(usize);
struct Borrowed<'a>(&'a usize);
impl RawSession for Borrowed<'_> {
    fn value(&self) -> usize { *self.0 }
}
impl RawHost for Provider {
    fn acquire(&mut self) -> Box<dyn RawSession + '_> { Box::new(Borrowed(&self.0)) }
}

fn main() {
    let mut provider = Provider(1);
    let mut host = Host::new(&mut provider);
    let session = host.acquire();
    drop(host);
    println!("{}", session.value());
}
