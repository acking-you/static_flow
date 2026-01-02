use serde::Serialize;

#[derive(Debug, Serialize, Clone)]
pub struct TagInfo {
    pub name: String,
    pub count: usize,
}

#[derive(Debug, Serialize, Clone)]
pub struct CategoryInfo {
    pub name: String,
    pub count: usize,
    pub description: String,
}
