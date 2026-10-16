use crate::Cents;

/// Returns the fee for `amount` at `basis_points` hundredths of a percent,
/// rounded half away from zero to the nearest cent, or `None` on overflow.
pub fn basis_point_fee(amount: Cents, basis_points: u32) -> Option<Cents> {
    let scaled = i128::from(amount.0).checked_mul(i128::from(basis_points))?;
    let half = if scaled < 0 { -5_000 } else { 5_000 };
    let fee = (scaled + half) / 10_000;
    i64::try_from(fee).ok().map(Cents)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fees_round_half_away_from_zero() {
        assert_eq!(basis_point_fee(Cents(1_000), 250), Some(Cents(25)));
        assert_eq!(basis_point_fee(Cents(199), 5_000), Some(Cents(100)));
        assert_eq!(basis_point_fee(Cents(-199), 5_000), Some(Cents(-100)));
        assert_eq!(basis_point_fee(Cents(0), 300), Some(Cents(0)));
    }

    #[test]
    fn large_amounts_stay_exact() {
        assert_eq!(
            basis_point_fee(Cents(i64::MAX), 10_000),
            Some(Cents(i64::MAX))
        );
        assert_eq!(
            basis_point_fee(Cents(9_007_199_254_740_993), 10_000),
            Some(Cents(9_007_199_254_740_993))
        );
        assert_eq!(basis_point_fee(Cents(i64::MAX), 20_000), None);
    }
}
