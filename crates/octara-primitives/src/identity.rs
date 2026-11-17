//! Opaque identity shared by documents, renderers and tools.

use slotmap::new_key_type;

new_key_type! {
    /// A node handle; a destroyed node's handle never aliases a new node.
    pub struct NodeId;
}
