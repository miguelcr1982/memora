#[derive(Debug, Clone)]
pub struct MemoryResource {
    pub url: String,
    pub mime_type: String,
    pub status: i32,
    pub body: Vec<u8>,
}