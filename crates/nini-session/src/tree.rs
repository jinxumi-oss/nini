//! Placeholder SessionTree stub.

pub struct SessionTree;

impl SessionTree {
    pub fn from_entries(_e: &[nini_core::SessionEntry]) -> Self { Self }
    pub fn main_path(&self) -> Vec<String> { vec![] }
    pub fn len(&self) -> usize { 0 }
    pub fn is_empty(&self) -> bool { true }
    pub fn get_path_to(&self, _id: &dyn std::fmt::Display) -> Vec<String> { vec![] }
    pub fn descendants<D: std::fmt::Display>(&self, _id: D) -> Vec<String> { vec![] }
}
