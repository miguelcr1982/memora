use super::resource::MemoryResource;

#[derive(Debug, Default)]
pub struct MemoryManager {
    resources: Vec<MemoryResource>,
}

impl MemoryManager {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn store(&mut self, resource: MemoryResource) {
        self.resources.push(resource);
    }

    pub fn resources(&self) -> &[MemoryResource] {
        &self.resources
    }

    pub fn len(&self) -> usize {
        self.resources.len()
    }

    pub fn is_empty(&self) -> bool {
        self.resources.is_empty()
    }
}