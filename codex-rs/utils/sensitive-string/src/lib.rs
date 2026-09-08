//! CATALYST: sensitive text shared by host UI, credentials and Provider requests.
//!
//! This foundation owns redacted diagnostics and zeroization, not credential validation or UI.
//! It deliberately provides no implicit string access, cloning or serialization.

use std::fmt;
use zeroize::Zeroize;

/// Owned sensitive text with explicit access and zeroization on drop.
pub struct SensitiveString(String);

impl SensitiveString {
    pub fn new(value: String) -> Self {
        Self(value)
    }

    pub fn expose_secret(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for SensitiveString {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SensitiveInput([REDACTED])")
    }
}

impl Drop for SensitiveString {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

#[cfg(test)]
#[path = "sensitive_string_tests.rs"]
mod tests;
