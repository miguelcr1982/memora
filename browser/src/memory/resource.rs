#[derive(Debug, Clone)]
pub struct MemoryResource {
    pub workspace: String,
    pub url: String,
    pub mime_type: String,
    pub status: i32,
    pub body: Vec<u8>,
}

impl MemoryResource {
    pub fn new(
        workspace: impl Into<String>,
        url: impl Into<String>,
        mime_type: impl Into<String>,
        status: i32,
        body: Vec<u8>,
    ) -> Self {
        Self {
            workspace: workspace.into(),
            url: url.into(),
            mime_type: mime_type.into(),
            status,
            body,
        }
    }
}