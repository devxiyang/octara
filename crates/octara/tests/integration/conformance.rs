//! The headless platform passes the platform contract's conformance checks.

use octara::platform::HeadlessPlatform;
use octara::prelude::*;

#[test]
fn the_headless_platform_conforms() {
    let app = App::new(HeadlessPlatform::default());
    let checks = octara::conformance::run(&app);
    let failed: Vec<_> = checks.iter().filter(|c| c.failure.is_some()).collect();
    assert!(failed.is_empty(), "{failed:#?}");
    assert_eq!(checks.len(), 6);
}
