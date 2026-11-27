# 15 — High-Frequency System Modules & WebView Depth (`0.0.9`)

> **Design premise**: Deliver the six highest-frequency application capabilities across **Tier-1 platforms (macOS, iOS, Web, Linux + Headless DST)** purely as optional first-party `modules/*` crates on top of `0.0.8`'s `octara::module` SPI — with **zero new core crates and zero core bloat**.  
> **Prerequisite**: `0.0.8` (`docs/design/v2/14-octara-module-architecture.md` Wave 1 & Wave 2) complete and published.

---

## 1. Scope Overview

| Crate | Module ID | Tier-1 Backends (macOS · iOS · Web · Linux) | `SENSITIVE` Policy |
| --- | --- | --- | --- |
| **`modules/octara-notifications`** | `octara.notifications` | Apple `UNUserNotificationCenter` · Web Notification API · Linux `org.freedesktop.Notifications` (D-Bus) · Headless `mock()` | Title/body redacted if `sensitive: true` |
| **`modules/octara-secure-store`** | `octara.secure_store` | Apple Keychain (`Security.framework`) · Linux Secret Service (D-Bus) · Web Crypto AES-GCM + IndexedDB · Headless `mock()` | **Mandatory `SENSITIVE = true`** on all secret reads/writes |
| **`modules/octara-desktop-shell`** | `octara.desktop_shell` | macOS `NSStatusItem` + Carbon/CGEvent hotkey + `SMAppService` · Linux StatusNotifierItem (D-Bus) + XDG GlobalShortcuts + autostart `.desktop` · iOS/Web graceful `Unavailable` · Headless `mock()` | Non-sensitive |
| **`modules/octara-storage`** | `octara.storage` | Apple `Application Support`/`Caches` + atomic KV · Linux XDG Base Dirs + atomic KV · Web IndexedDB + OPFS · Headless in-memory `mock()` | KV values `SENSITIVE = true` |
| **`modules/octara-share`** | `octara.share` | iOS `UIActivityViewController` · macOS `NSSharingServicePicker` · Linux XDG portal / clipboard fallback · Web `navigator.share` · Headless `mock()` | Share payloads `SENSITIVE = true` |
| **`modules/octara-network`** | `octara.network` | Apple `NWPathMonitor` · Linux NetworkManager D-Bus / netlink · Web `navigator.onLine` · Headless `mock()` | Non-sensitive |
| **`modules/octara-webview`** *(upgrade)* | `octara.webview` | `WKWebsiteDataStore` profiles, cookie/cache clearing, `WKUserScript` injection, CSP policy helpers, `WKDownloadDelegate` | Cookies/headers `SENSITIVE = true` |

**Hard Architectural Rules**:
1. Every crate lives under `modules/*` and passes `tools/check_layers.py` (internal deps limited to `octara` / `octara-icons` / `octara-kit`; OS crates strictly target-gated).
2. Every crate provides both `module()` (platform-selected runtime backend) and `mock()` (pure-Rust deterministic backend with zero OS dependencies).
3. Every crate requiring OS entitlements, Info.plist keys, frameworks, or Web permissions includes a schema-v1 `octara-module.toml`.
4. All async OS callbacks hop back to the UI thread exclusively via `ModuleDispatcher`, honoring `ScopeId` auto-cancellation.

---

## 2. Detailed Module Specifications

### 2.1 `modules/octara-notifications`

#### Public Types & Calls
```rust
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocalNotification {
    pub id: String,
    pub title: String,
    pub body: Option<String>,
    pub subtitle: Option<String>,
    pub badge: Option<u32>,
    pub sound: NotificationSound,
    pub thread_id: Option<String>,
    /// Opaque app-defined routing payload echoed back on `NotificationTapped`.
    pub data: BTreeMap<String, String>,
    /// Optional delay before delivery (relative to host clock / virtual clock in DST).
    pub deliver_after: Option<Duration>,
}

pub enum NotificationSound { Default, Silent }

// ModuleCalls (`MODULE = ModuleId("octara.notifications")`)
pub struct RequestNotificationPermission;               // Output = PermissionStatus
pub struct GetNotificationPermission;                   // Output = PermissionStatus
pub struct ScheduleNotification(pub LocalNotification); // Output = ()
pub struct CancelNotification { pub id: String }        // Output = ()
pub struct CancelAllNotifications;                      // Output = ()
pub struct RegisterRemotePush;                          // Output = RemotePushToken (SENSITIVE = true)

// ModuleEvents
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NotificationTapped {
    pub id: String,
    pub action_id: Option<String>,
    pub data: BTreeMap<String, String>,
}
```

#### Platform Mapping
- **macOS & iOS**: `UNUserNotificationCenter` delegate installed without clobbering existing host delegates; `UNNotificationRequest` with optional `UNTimeIntervalNotificationTrigger`; remote push token forwarded from `AppDelegate` via `HostLifecycleEvent` / module registration.
- **Linux**: `org.freedesktop.Notifications` D-Bus (`Notify`, `CloseNotification`, `ActionInvoked` signal → `NotificationTapped`); graceful `PermissionStatus::Granted` when D-Bus notification daemon is reachable, `Unavailable` otherwise.
- **Web**: `Notification.requestPermission()` + `new Notification(...)` + `onclick` → `NotificationTapped`; `RegisterRemotePush` returns `ModuleError::Unavailable` unless push worker is configured.
- **Headless `mock()`**: Virtual-clock scheduled delivery + deterministic `MockNotifications::tap(id)` test helper.

#### `octara-module.toml`
- `apple.frameworks`: `macos = ["UserNotifications"]`, `ios = ["UserNotifications"]`.
- `web.headers`: optional notifications permission metadata.

---

### 2.2 `modules/octara-secure-store`

#### Public Types & Calls
All calls set **`const SENSITIVE: bool = true`** so secret bytes/strings never enter `octara-inspect` traces or logs:

```rust
pub enum AccessPolicy {
    /// Accessible whenever the device is unlocked (default).
    WhenUnlocked,
    /// Accessible after first unlock (suitable for background sync).
    AfterFirstUnlock,
    /// Accessible only on this device (excluded from cloud keychain sync).
    WhenUnlockedThisDeviceOnly,
}

// ModuleCalls (`MODULE = ModuleId("octara.secure_store")`, all `SENSITIVE = true`)
pub struct SecureSet {
    pub service: String,
    pub account: String,
    pub secret: Vec<u8>,
    pub access: AccessPolicy,
} // Output = ()

pub struct SecureGet {
    pub service: String,
    pub account: String,
} // Output = Option<Vec<u8>>

pub struct SecureDelete {
    pub service: String,
    pub account: String,
} // Output = bool
```

#### Platform Mapping
- **macOS & iOS**: `Security.framework` Keychain Services (`SecItemAdd` / `SecItemCopyMatching` / `SecItemUpdate` / `SecItemDelete`) with `kSecClassGenericPassword`, mapping `AccessPolicy` to `kSecAttrAccessible*`.
- **Linux**: Freedesktop Secret Service D-Bus (`org.freedesktop.secrets`, default collection lookup + locked-collection prompt handling); returns `ModuleError::Unavailable` cleanly on headless systems without a keyring daemon.
- **Web**: Origin-scoped AES-GCM key stored as a non-extractable `CryptoKey` in IndexedDB encrypting secret payloads at rest (documented security boundary: browser origin isolation, not hardware enclave).
- **Headless `mock()`**: In-memory map enforcing identical `SENSITIVE = true` redaction guarantees.

#### `octara-module.toml`
- `apple.frameworks`: `macos = ["Security"]`, `ios = ["Security"]`.
- Optional macOS/iOS keychain-access-groups entitlement template when configured in `octara.toml`.

---

### 2.3 `modules/octara-desktop-shell`

Desktop-only ergonomic shell module (returns `ModuleError::Unavailable` cleanly on iOS and Web):

#### Public Types & Calls
```rust
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TrayIcon {
    /// Monochrome template icon (adapts to light/dark menu bar on macOS).
    TemplatePng(Arc<[u8]>),
    RgbaPng(Arc<[u8]>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrayConfig {
    pub id: String,
    pub title: Option<String>,
    pub tooltip: Option<String>,
    pub icon: Option<TrayIcon>,
    /// Reuses core `octara::Menu`!
    pub menu: Option<Menu>,
}

// ModuleCalls (`MODULE = ModuleId("octara.desktop_shell")`)
pub struct UpsertTray(pub TrayConfig);                        // Output = ()
pub struct RemoveTray { pub id: String }                      // Output = ()
pub struct RegisterGlobalHotkey { pub id: String, pub chord: Chord } // Output = ()
pub struct UnregisterGlobalHotkey { pub id: String }          // Output = ()
pub struct SetLaunchAtLogin { pub enabled: bool }             // Output = ()
pub struct GetLaunchAtLogin;                                  // Output = bool

// ModuleEvents
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrayClicked { pub id: String }
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrayMenuSelected { pub tray_id: String, pub item_id: String }
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GlobalHotkeyPressed { pub id: String }
```

#### Platform Mapping
- **macOS**:
  - Tray: `NSStatusBar::systemStatusBar().statusItemWithLength(...)` + `NSMenu` built from `octara::Menu`.
  - Global Hotkey: Carbon `RegisterEventHotKey` (requires zero Accessibility permissions for standard shortcuts, cleanly unregistered on `shutdown()`).
  - Launch-at-Login: `SMAppService::mainAppService()` (macOS 13+).
- **Linux**:
  - Tray: D-Bus `org.kde.StatusNotifierItem` + `com.canonical.dbusmenu`.
  - Global Hotkey: `org.freedesktop.portal.GlobalShortcuts` (Wayland-safe; graceful `Unavailable` when portal is absent).
  - Launch-at-Login: `$XDG_CONFIG_HOME/autostart/<app_id>.desktop`.
- **iOS / Web**: Compile cleanly with zero desktop dependencies; calls return `ModuleError::Unavailable`.
- **Headless `mock()`**: Records active trays, hotkeys, and launch-at-login flag; exposes deterministic `trigger_tray_click`, `trigger_tray_menu`, and `trigger_hotkey` helpers.

---

### 2.4 `modules/octara-storage`

Standard cross-platform storage paths + atomic KV store + direct `RestartState` integration:

#### Public Types & Calls
```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StorageDir {
    AppData,
    Cache,
    Temp,
    Documents,
}

// ModuleCalls (`MODULE = ModuleId("octara.storage")`)
pub struct ResolveDir(pub StorageDir);                                   // Output = Option<PathBuf> (None on Web)
pub struct KvGet { pub namespace: String, pub key: String }              // Output = Option<Vec<u8>> (SENSITIVE = true)
pub struct KvSet { pub namespace: String, pub key: String, pub value: Vec<u8> } // Output = () (SENSITIVE = true)
pub struct KvDelete { pub namespace: String, pub key: String }           // Output = bool
pub struct KvKeys { pub namespace: String, pub prefix: Option<String> }  // Output = Vec<String>
pub struct KvClear { pub namespace: String }                             // Output = ()
```

#### `RestartState` Integration Helper
- Provide `octara_storage::save_restart_state(w, namespace, state)` and `load_restart_state(task, namespace)` so desktop, iOS, and Web apps share a single persistence path without bespoke filesystem/IndexedDB glue.

#### Platform Mapping
- **macOS & iOS**: `NSSearchPathForDirectoriesInDomains` (`Application Support`, `Caches`, `Documents`, `NSTemporaryDirectory`) + crash-safe atomic temp-file-and-rename KV store under `AppData/kv/<namespace>/`.
- **Linux**: XDG Base Directory Specification (`$XDG_DATA_HOME`, `$XDG_CACHE_HOME`, `/tmp`, `xdg-user-dir DOCUMENTS`) + identical crash-safe atomic KV store.
- **Web**: `ResolveDir` returns `Ok(None)`; KV calls store binary values in IndexedDB (object store per namespace) with async completion over `ModuleDispatcher`.
- **Headless `mock()`**: Pure in-memory KV + isolated temp directory paths for deterministic restart tests.

---

### 2.5 `modules/octara-share`

```rust
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SharePayload {
    pub title: Option<String>,
    pub text: Option<String>,
    pub url: Option<String>,
    pub files: Vec<ShareFile>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShareFile {
    pub name: String,
    pub media_type: String,
    pub bytes: Vec<u8>,
}

// ModuleCall (`MODULE = ModuleId("octara.share")`, `SENSITIVE = true`)
pub struct PresentShare(pub SharePayload); // Output = ShareOutcome::{Completed, Dismissed}
```

- **iOS**: `UIActivityViewController` presented on the active `UIViewController` from `HostSurface` (with popover source anchor on iPad).
- **macOS**: `NSSharingServicePicker` anchored to the active `NSView` from `HostSurface`.
- **Web**: `navigator.share(...)` (returns `ModuleError::ActivationRequired` or `Unavailable` cleanly if unsupported or called outside user activation).
- **Linux**: Open/share via XDG desktop portal or return `ModuleError::Unavailable` gracefully when unsupported.
- **Headless `mock()`**: Records shared payloads and returns scripted `ShareOutcome`.

---

### 2.6 `modules/octara-network`

```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NetworkKind {
    Wifi,
    Cellular,
    Ethernet,
    Other,
    None,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NetworkStatus {
    pub online: bool,
    pub kind: NetworkKind,
    pub expensive: bool,
    pub constrained: bool,
}

// Installed on root scope during `OctaraModule::install`:
// `cx.context::<NetworkHandle>().status()` -> `ReadSignal<NetworkStatus>`
pub struct GetNetworkStatus; // ModuleCall, Output = NetworkStatus
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NetworkStatusChanged(pub NetworkStatus); // ModuleEvent
```

- **macOS & iOS**: `Network.framework` `NWPathMonitor` running on a dedicated dispatch queue, forwarding transitions through `ModuleDispatcher` into both the root `Signal<NetworkStatus>` and `NetworkStatusChanged`.
- **Linux**: NetworkManager D-Bus (`org.freedesktop.NetworkManager` `StateChanged` / `PrimaryConnectionType`) with graceful fallback to interface link check.
- **Web**: `navigator.onLine` + `window` `online`/`offline` listeners (+ optional `navigator.connection` type/saveData when present).
- **Headless `mock()`**: Deterministic `set_status(NetworkStatus)` mutator updating the signal and emitting `NetworkStatusChanged` inside the next `Cycle`.

---

### 2.7 `modules/octara-webview` Depth Upgrades

Extend `modules/octara-webview` (from `0.0.8`) without breaking its core `NativeViewFactory` contract:
1. **Data Store & Profile Isolation**:
   - Add `WebViewProfile::{Default, Persistent(String), Ephemeral}` on `WebViewProps`.
   - On Apple `WKWebView`: map `Ephemeral` to `WKWebsiteDataStore::nonPersistentDataStore()`, `Persistent(id)` to deterministic UUID `WKWebsiteDataStore::dataStoreForIdentifier:` (macOS 14+/iOS 17+), with typed command `WebViewCommand::ClearData { kinds, since }` (`Cookies`, `Cache`, `LocalStorage`, `IndexedDb`).
2. **User Script Injection (`WKUserScript`)**:
   - Add `WebViewScript { source: Arc<str>, injection_time: ScriptInjectionTime::{DocumentStart, DocumentEnd}, main_frame_only: bool }` to `WebViewProps` (installed on `WKUserContentController` before initial navigation).
3. **Download Interception (`WKDownloadDelegate`)**:
   - Intercept download responses/actions and emit typed `WebViewEvent::DownloadRequested { url, suggested_filename, mime_type }` with typed response policy (`Allow(PathBuf)` / `Cancel`).
4. **CSP Sandbox Helpers**:
   - Support declarative `WebViewSandboxPolicy` (restrict allowed origins, disable inline eval/scripts when requested) enforced both via navigation policy and injected CSP meta/headers for custom scheme responses.

---

## 3. Execution Waves & Verification Gates (`0.0.9`)

### Wave A — Core Utility & Security Modules (`storage`, `secure-store`, `network`)
- Implement `modules/octara-storage`, `modules/octara-secure-store`, and `modules/octara-network` with macOS/iOS/Linux/Web backends, `octara-module.toml`, Headless `mock()`, and `RestartState` integration tests.

### Wave B — Desktop & User-Facing OS Modules (`notifications`, `desktop-shell`, `share`)
- Implement `modules/octara-notifications`, `modules/octara-desktop-shell`, and `modules/octara-share` with full Tier-1 backends, `octara-module.toml`, and DST suites.

### Wave C — `modules/octara-webview` Persistence, Scripts & Downloads + Release Gate
- Upgrade `modules/octara-webview` (`WebViewProfile`, `WKUserScript`, `WKDownloadDelegate`, `ClearData`, `WebViewSandboxPolicy`).
- Run full `0.0.9` release gate:
  1. `python3 tools/check_layers.py` (confirm zero core crate depends on any `modules/*` crate).
  2. `cargo clippy --workspace --all-targets -- -D warnings`.
  3. Cross-target checks: `wasm32-unknown-unknown` (default + `webgpu` + all `modules/*`), `aarch64-apple-ios-sim` (`octara-host-native` + all `modules/*`).
  4. `cargo test --workspace` + `cargo test -p octara-cli --doc` + `octara-cli` manifest merge test covering all 10 official `modules/*` crates together.
  5. Verify `SENSITIVE = true` redaction tests across `octara-secure-store`, `octara-storage`, `octara-share`, and `octara-notifications`.
