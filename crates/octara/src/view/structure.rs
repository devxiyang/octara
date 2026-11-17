//! Structural writes shared by ordinary mounting, conditional branches and lists.
//! Callers own scope disposal and slot counts. These operations run no user callbacks;
//! release the kernel borrow before disposing scopes or retiring interactions.

use crate::kernel::{Kernel, NodeId};

/// Places consecutive application nodes without replacing their allocations.
pub(super) fn place(kernel: &mut Kernel, parent: NodeId, start: usize, nodes: impl IntoIterator<Item = NodeId>) {
    for (offset, node) in nodes.into_iter().enumerate() {
        let index = start + offset;
        match kernel.parent(node) {
            None => kernel.insert_child(parent, node, index),
            Some(owner) => {
                assert_eq!(owner, parent, "view structure cannot transfer a node between owners");
                if kernel.children(parent).get(index) != Some(&node) {
                    kernel.move_child(node, index);
                }
            }
        }
    }
}

/// Detaches animated roots before their caller disposes the owning scopes.
pub(super) fn begin_exit(kernel: &mut Kernel, nodes: impl IntoIterator<Item = NodeId>) {
    for node in nodes {
        kernel.begin_exit(node);
    }
}

/// Cleanup may encounter descendants already destroyed by their parent. Exiting nodes
/// remain owned by the kernel until their animation finishes (`Kernel::destroy`).
pub(super) fn destroy(kernel: &mut Kernel, nodes: impl IntoIterator<Item = NodeId>) {
    for node in nodes {
        if kernel.is_alive(node) {
            kernel.destroy(node);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn placement_preserves_identity_and_rejects_foreign_ownership() {
        let mut k = Kernel::default();
        let parent = k.create_box();
        let other = k.create_box();
        let a = k.create_input();
        let b = k.create_box();
        k.set_value(a, "draft");
        place(&mut k, parent, 0, [a, b]);
        place(&mut k, parent, 0, [b, a]);
        assert_eq!(k.children(parent), &[b, a]);
        assert_eq!(k.value(a), "draft");
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| place(&mut k, other, 0, [a])));
        assert!(result.is_err());
        assert_eq!(k.children(parent), &[b, a]);
        assert!(k.children(other).is_empty());
        destroy(&mut k, [parent, a, b]);
        assert!(!k.is_alive(a));
        assert!(!k.is_alive(b));
    }
}
