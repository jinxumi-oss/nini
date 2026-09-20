//! Placeholder project trust stub.

use std::collections::HashMap;
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrustLevel {
    Ask,
    Trusted,
    Distrusted,
}

impl Default for TrustLevel {
    fn default() -> Self { Self::Ask }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TrustDecision {
    pub level: TrustLevel,
}

impl TrustDecision {
    pub const Trusted: Self = Self { level: TrustLevel::Trusted };
    pub const Distrusted: Self = Self { level: TrustLevel::Distrusted };
    pub const Ask: Self = Self { level: TrustLevel::Ask };
}

#[derive(Default)]
pub struct ProjectTrustStore {
    pub decision: TrustDecision,
}

impl ProjectTrustStore {
    pub fn load(_path: &Path) -> Result<Self, std::io::Error> {
        Ok(Self::default())
    }
    pub fn get(&self, _key: &str) -> Option<TrustDecision> {
        Some(self.decision.clone())
    }
    pub fn default_path() -> Option<std::path::PathBuf> {
        Some(std::path::PathBuf::from(".pi/agent/trust.json"))
    }
}

impl ProjectTrustStore {
    pub fn save(&self, _path: &Path) -> Result<(), std::io::Error> { Ok(()) }
    pub fn clear(&mut self, _cwd: &str) {}
}

impl ProjectTrustStore {
    pub fn set(&mut self, _cwd: &str, _decision: TrustDecision) {}
}
