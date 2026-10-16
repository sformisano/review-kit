use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Backoff {
    initial_ms: u32,
    limit_ms: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum BackoffError {
    ZeroInitial,
    LimitTooSmall,
}

impl fmt::Display for BackoffError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ZeroInitial => f.write_str("the initial delay must be greater than zero"),
            Self::LimitTooSmall => f.write_str("the limit must be greater than the initial delay"),
        }
    }
}

impl std::error::Error for BackoffError {}

impl Backoff {
    pub fn new(initial_ms: u32, limit_ms: u32) -> Result<Self, BackoffError> {
        if initial_ms == 0 {
            return Err(BackoffError::ZeroInitial);
        }
        if limit_ms <= initial_ms {
            return Err(BackoffError::LimitTooSmall);
        }
        Ok(Self {
            initial_ms,
            limit_ms,
        })
    }

    pub fn delay_ms(&self, attempt: u32) -> u32 {
        let factor = 1u32.checked_shl(attempt).unwrap_or(u32::MAX);
        self.initial_ms.saturating_mul(factor).min(self.limit_ms)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delays_double_up_to_the_limit() {
        let backoff = Backoff::new(100, 1_000).unwrap();
        assert_eq!(backoff.delay_ms(0), 100);
        assert_eq!(backoff.delay_ms(1), 200);
        assert_eq!(backoff.delay_ms(3), 800);
        assert_eq!(backoff.delay_ms(4), 1_000);
        assert_eq!(backoff.delay_ms(40), 1_000);
        assert_eq!(backoff.delay_ms(u32::MAX), 1_000);
    }

    #[test]
    fn constructor_rejects_invalid_schedules() {
        assert_eq!(Backoff::new(0, 1_000), Err(BackoffError::ZeroInitial));
        assert_eq!(Backoff::new(500, 500), Err(BackoffError::LimitTooSmall));
        assert_eq!(Backoff::new(500, 100), Err(BackoffError::LimitTooSmall));
    }
}
