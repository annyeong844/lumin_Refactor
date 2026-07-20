use std::fmt;
use std::num::NonZeroU64;

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct StoreGeneration(NonZeroU64);

impl StoreGeneration {
    pub const INITIAL: Self = Self(NonZeroU64::MIN);

    pub fn get(self) -> u64 {
        self.0.get()
    }

    #[cfg(test)]
    pub(crate) fn checked_next(self) -> Option<Self> {
        self.0
            .get()
            .checked_add(1)
            .and_then(NonZeroU64::new)
            .map(Self)
    }
}

impl fmt::Display for StoreGeneration {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.get().fmt(formatter)
    }
}
