//! Shared diagnostic codes for failures and warnings detected at runtime.

use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Info,
    Warning,
    Error,
}

/// A registered diagnostic and the rule behind it.
#[derive(Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Code {
    pub id: &'static str,
    pub severity: Severity,
    pub summary: &'static str,
    pub cause: &'static str,
    pub doc: &'static str,
}

impl fmt::Display for Code {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.id)
    }
}

impl fmt::Debug for Code {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Diagnostic {
    #[serde(with = "code_id")]
    pub code: &'static Code,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub location: Option<String>,
}

impl Diagnostic {
    pub fn new(code: &'static Code, message: impl Into<String>) -> Self {
        Self { code, message: message.into(), location: None }
    }

    pub fn severity(&self) -> Severity {
        self.code.severity
    }
}

mod code_id {
    use super::{Code, codes};
    use serde::{Deserialize, Deserializer, Serializer, de::Error as _};

    pub(super) fn serialize<S: Serializer>(code: &&Code, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(code.id)
    }

    pub(super) fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<&'static Code, D::Error> {
        let id = String::deserialize(deserializer)?;
        codes::lookup(&id).ok_or_else(|| D::Error::custom(format!("unknown diagnostic code `{id}`")))
    }
}

/// The registry used by the runtime and its inspection tools.
pub mod codes {
    use super::{Code, Severity};

    macro_rules! register {
        ($($name:ident: $severity:ident, $summary:literal, $cause:literal, $doc:literal;)*) => {
            $(
                #[doc = $summary]
                pub static $name: Code = Code {
                    id: stringify!($name),
                    severity: Severity::$severity,
                    summary: $summary,
                    cause: $cause,
                    doc: $doc,
                };
            )*
            pub static ALL: &[&Code] = &[$(&$name),*];
        };
    }

    pub fn lookup(id: &str) -> Option<&'static Code> {
        ALL.iter().copied().find(|code| code.id == id)
    }

    register! {
        CYCLE: Error,
            "A reactive computation depends on its own output.",
            "A dynamic read reaches a computation already on the evaluation stack.",
            "docs/design/v2/02-reactive-runtime.md";
        FLUSH_RUNAWAY: Error,
            "Reactive updates exhausted the cycle budget.",
            "Post-commit effects keep writing values that cause more effects to run.",
            "docs/design/v2/02-reactive-runtime.md#5-five-phase-cycle-engine-cyclers";
        LAYOUT_LOOP: Warning,
            "A window did not settle within the projection round budget.",
            "Geometry feedback changed state that kept invalidating the inputs to layout.",
            "docs/design/v2/02-reactive-runtime.md#5-five-phase-cycle-engine-cyclers";
        READ_DISPOSED: Error,
            "A reactive handle was read after its owning scope was disposed.",
            "A copy of the handle survived the runtime allocation it names.",
            "docs/design/v2/02-reactive-runtime.md";
        WRITE_DISPOSED: Error,
            "A reactive handle was written after its owning scope was disposed.",
            "A queued task or handler retained a handle to state whose scope retired.",
            "docs/design/v2/02-reactive-runtime.md";
        WRONG_RUNTIME: Error,
            "A reactive handle was used with a different application.",
            "A handle names a slot and generation in one runtime namespace.",
            "docs/design/v2/02-reactive-runtime.md";
        DUPLICATE_PERSISTED_KEY: Error,
            "A scope declared the same persisted key more than once.",
            "A persisted key must uniquely identify one value within its structural owner.",
            "docs/design/v2/02-reactive-runtime.md";
        PERSISTED_TYPE_CHANGED: Warning,
            "A persisted key was requested with a different type.",
            "The stored value's type differs from the type declared for this key.",
            "docs/design/v2/02-reactive-runtime.md";
        DUPLICATE_ROW_KEY: Error,
            "Two rows in one structural region have the same key.",
            "A keyed region uses each key to identify exactly one row.",
            "docs/design/v2/03-tree-microkernel.md#32-preflight-validation-algorithm-otx--tree_depth";
        LIST_ITEM_UNAVAILABLE: Error,
            "A retained row read a key absent from its current source.",
            "Row inputs read from the current collection, so a removed key cannot supply its previous value.",
            "docs/design/v2/02-reactive-runtime.md";
        JOIN_FANOUT: Warning,
            "A join change invalidated many rows on the opposite side.",
            "Many rows share the same join key and must each update when its match changes.",
            "docs/design/v2/02-reactive-runtime.md";
        STALE_TARGET: Error,
            "An input target no longer names its original receiver.",
            "The window, tree incarnation, node, or editor session changed since the target was chosen.",
            "docs/design/v2/06-inspect-dst-release-migration.md#11-octarainspect2-protocol-contract";
        INSPECT_PROTOCOL: Error,
            "An inspection request uses an unsupported protocol version.",
            "The request must name the protocol version accepted by the inspection host.",
            "docs/design/v2/06-inspect-dst-release-migration.md";
        INSPECT_REQUEST: Error,
            "An inspection request could not be decoded.",
            "The request is malformed or it exceeds the transport's input limit.",
            "docs/design/v2/06-inspect-dst-release-migration.md";
        INSPECT_RESPONSE: Error,
            "An inspection response could not be encoded.",
            "The receipt payload is invalid or it exceeds the transport's output limit.",
            "docs/design/v2/06-inspect-dst-release-migration.md";
        INSPECT_RESTART_UNAVAILABLE: Error,
            "The inspection host refused a development restart command.",
            "Only a dev host provides restart state export and shutdown commands.",
            "docs/design/v2/07-app-primitives-and-headless-kit.md#27-dev-hot-restart-runner-octara-cli";
        CLIPPED_TEXT: Warning,
            "A text element is clipped at its settled layout size.",
            "The host truncated the text with an ellipsis or a line limit.",
            "docs/design/v2/11-extreme-engine-cpu-gpu-and-ai-dx.md#5-octara-inspect-audit-spi--deterministic-performance-gates";
        MISSING_SEMANTICS: Error,
            "An interactive element has no accessible name.",
            "The element has no nonblank label, text alternative, or usable labeling relationship.",
            "docs/design/v2/11-extreme-engine-cpu-gpu-and-ai-dx.md#5-octara-inspect-audit-spi--deterministic-performance-gates";
        DEAD_FOCUS_TARGET: Warning,
            "A focus target cannot be reached in the settled window.",
            "The target is hidden, inert, has empty bounds, or is outside the active modal.",
            "docs/design/v2/11-extreme-engine-cpu-gpu-and-ai-dx.md#5-octara-inspect-audit-spi--deterministic-performance-gates";
        BUILD_MISMATCH: Error,
            "The inspected process differs from the launch receipt.",
            "The process identifier or executable digest in the Inspector response differs from the expected build.",
            "docs/design/v2/06-inspect-dst-release-migration.md";
        INPUT_REJECTED: Warning,
            "The action cannot be delivered to the requested target.",
            "The current hit, focus, modal scope, or input payload does not accept the action.",
            "docs/design/v2/06-inspect-dst-release-migration.md";
        CAPTURE_UNAVAILABLE: Warning,
            "The host cannot capture the requested frame.",
            "The requested capture has no available presentation or painting source.",
            "docs/design/v2/06-inspect-dst-release-migration.md";
        NOT_SETTLED: Warning,
            "The requested window has an incomplete update.",
            "Pending work or a failed projection prevented publication of a new settled frame.",
            "docs/design/v2/02-reactive-runtime.md#5-five-phase-cycle-engine-cyclers";
        APP_FAILED: Error,
            "An application operation panicked and the runtime stopped publishing updates.",
            "A panic can leave application values partially changed. Shut down this application and create a new one.",
            "docs/design/architecture-v2.md#5-five-phase-cycle-engine--atomic-command-stream-mounting";
        DUPLICATE_MODULE: Error,
            "The application registered more than one module with the same ID.",
            "Each registered module must declare a unique ID within the application.",
            "docs/design/v2/14-octara-module-architecture.md";
        MODULE_INSTALL_FAILED: Error,
            "The application could not install a registered module.",
            "The module returned an error during installation.",
            "docs/design/v2/14-octara-module-architecture.md";
        MODULE_MANIFEST_INVALID: Error,
            "A module manifest could not be validated.",
            "Check the manifest schema and the type of every declared value.",
            "docs/design/v2/14-octara-module-architecture.md#25-declarative-manifest-octara-moduletoml--octara-cli-integration";
        MODULE_VARIABLE_MISSING: Error,
            "A module manifest requires an application configuration value.",
            "Set the named variable in the module's table in octara.toml.",
            "docs/design/v2/14-octara-module-architecture.md#25-declarative-manifest-octara-moduletoml--octara-cli-integration";
        MODULE_MANIFEST_CONFLICT: Error,
            "Two modules require different values for the same application setting.",
            "Resolve the conflicting module declarations before merging the manifests.",
            "docs/design/v2/14-octara-module-architecture.md#25-declarative-manifest-octara-moduletoml--octara-cli-integration";
        NATIVE_VIEW_MOUNT_FAILED: Warning,
            "A registered native view could not mount.",
            "Its factory is unavailable on this host or could not create the view.",
            "docs/design/v2/14-octara-module-architecture.md";
        MISSING_THEME_TOKEN: Warning,
            "An element references a token missing from its theme.",
            "The active theme does not define a value for that token.",
            "docs/design/v2/03-tree-microkernel.md";
        ELEMENT_CONTENT: Error,
            "An operation is incompatible with the target's content or ownership.",
            "Atomic content has a specific node kind, and authored children may only be placed in containers.",
            "docs/design/v2/03-tree-microkernel.md#3-atomic-transaction-protocol-transactionrs";
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn registry_codes_are_unique_and_diagnostics_round_trip_by_id() {
        let mut ids = HashSet::new();
        for code in codes::ALL {
            assert!(ids.insert(code.id), "duplicate diagnostic code {code}");
            assert_eq!(codes::lookup(code.id), Some(*code));
        }
        let diagnostic = Diagnostic::new(&codes::STALE_TARGET, "target's tree was replaced");
        let value = serde_json::to_value(&diagnostic).unwrap();
        assert_eq!(value["code"], "STALE_TARGET");
        assert_eq!(serde_json::from_value::<Diagnostic>(value).unwrap(), diagnostic);
    }
}
