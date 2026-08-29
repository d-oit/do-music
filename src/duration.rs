use anyhow::{Context, Result, anyhow};

/// A validated requested length between 3 and 60 minutes, stored as seconds.
///
/// The provider never accepts a duration parameter; this only describes what
/// the final result should be, so that `plan_tracks` can split it up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Duration(u64);

impl Duration {
    /// Parse `4m`, `60m`, `180s` ... `3600s` (case-insensitive, trimmed).
    ///
    /// Valid range: 3m–60m, i.e. 180s–3600s.
    pub fn parse(value: &str) -> Result<Self> {
        let value = value.trim().to_ascii_lowercase();
        let (n, unit) = value.split_at(value.len().saturating_sub(1));
        let amount: u64 = n.parse().context("duration must look like 4m or 60m")?;
        match unit {
            "m" if (3..=60).contains(&amount) => Ok(Self(amount * 60)),
            "s" if (180..=3600).contains(&amount) => Ok(Self(amount)),
            _ => Err(anyhow!("duration must be 3m–60m")),
        }
    }

    /// Total length in seconds.
    pub fn seconds(self) -> u64 {
        self.0
    }

    /// Total length in whole minutes (floored).
    pub fn minutes(self) -> u64 {
        self.0 / 60
    }
}
