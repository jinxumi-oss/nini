//! Placeholder startup UI stub.

use std::path::Path;

pub fn needs_first_time_setup(_home: &Path) -> bool { false }
pub fn init_agent_dir(_home: &Path) -> Result<(), Box<dyn std::error::Error>> { Ok(()) }
