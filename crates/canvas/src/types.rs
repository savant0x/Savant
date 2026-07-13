use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Represents a visual element on the A2UI Canvas.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CanvasElement {
    pub id: String,
    pub element_type: String, // e.g., "rect", "text", "circle"
    pub properties: HashMap<String, String>,
}

/// The global state of the A2UI Canvas.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CanvasState {
    pub elements: HashMap<String, CanvasElement>,
    pub version: u64,
}

impl CanvasState {
    /// Applies a list of elements to the state, incrementing version.
    pub fn update(&mut self, new_elements: Vec<CanvasElement>) {
        for el in new_elements {
            self.elements.insert(el.id.clone(), el);
        }
        self.version += 1;
    }
}
