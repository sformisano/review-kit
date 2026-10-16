use crate::Cents;

/// Returns the fee for `amount` at `percent` percent, rounded to the nearest cent.
pub fn percentage_fee(amount: Cents, percent: f64) -> Cents {
    Cents((amount.0 as f64 * percent / 100.0).round() as i64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fees_round_to_the_nearest_cent() {
        assert_eq!(percentage_fee(Cents(1_000), 2.5), Cents(25));
        assert_eq!(percentage_fee(Cents(199), 50.0), Cents(100));
        assert_eq!(percentage_fee(Cents(0), 3.0), Cents(0));
    }
}
