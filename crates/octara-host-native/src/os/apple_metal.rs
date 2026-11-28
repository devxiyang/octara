//! Core Animation transaction state shared by the Apple hosts.

use objc2_quartz_core::CAMetalLayer;

pub(super) fn set_transactional(layer: &CAMetalLayer, transactional: bool) {
    if layer.presentsWithTransaction() != transactional {
        layer.setPresentsWithTransaction(transactional);
    }
}
