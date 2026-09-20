//! Clipboard stub.
pub fn copy(_s: &str) -> Result<(), String> { Ok(()) }
pub fn read_image() -> Option<(Vec<u8>, u32, u32)> { None }
