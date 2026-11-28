# 20 — Zero-Bloat Core Dependencies & Native OS Primitives (`0.0.10`)

> **Design premise**: Zero historical baggage, zero redundant wrapper crates in core dependencies.  
> **North star**: Every native Octara application and every Wasm bundle should compile fast, ship lean, and never link an entire third-party async runtime, TLS stack, heavy image crate, map SDK, or CLDR binary blob just to open a window.

---

## 1. Audited Core Dependency Bloat (Root Causes)

Workspace dependency inspection after removing `winit` and `TaffyTree` revealed six high-ROI dependency debts concentrated in `octara-host-native`, `octara-tree`, and `octara-render`:

1. **`tokio` + `reqwest` (+ `rustls` + `hyper` + `h2`, ~100+ transitive crates) inside `octara-host-native`**:
   - Used **exclusively** by `crates/octara-host-native/src/media.rs` to fetch remote `http(s)://` image URLs and run `spawn_blocking` for decoding/disk cache.
   - Forces every native Octara GUI app to compile and link a second multi-threaded async runtime and TLS/HTTP stack even on Apple platforms where `NSURLSession` is built into the OS (HTTP/3, system proxy/VPN, Keychain TLS, zero binary size).
2. **`objc2-map-kit` + `objc2-core-location` hardcoded inside `octara-host-native/src/os/macos/native.rs`**:
   - While `WebView` (`objc2-web-kit`) and `Video` (`AVKit`) were extracted into optional `modules/*` via `NativeViewFactory` (SPI-7) in `0.0.8`, `MapKit` remained hardcoded in core `octara-host-native`, forcing every macOS app to link `MapKit.framework` and `CoreLocation.framework`.
3. **Linux Clipboard & Xlib Pile-Up (`arboard` + `clipboard-rs` + `wl-clipboard-rs` + `x11-dl`)**:
   - After Wave D2's direct Wayland (`wl_data_device`) and X11 (`x11rb`) host, three separate clipboard wrappers plus C `x11-dl` are 100% redundant.
4. **`image` Crate Mandatory on Apple Platforms**:
   - `octara-host-native` unconditionally links `image` (PNG/JPEG/GIF/WebP) even on macOS `15.4+` / iOS `18.4+`, where system `ImageIO` (`CGImageSource` / `CGImageDestination`) already provides hardware-accelerated decode (PNG/JPEG/GIF/WebP/HEIC/AVIF) and PNG encode with zero Rust decoder footprint.
5. **`icu_locale` (`compiled_data`) inside Layer-1 Wasm Core `octara-tree`**:
   - `crates/octara-tree/src/style/tree.rs` imports `icu_locale::{Locale, LocaleDirectionality}` **solely** to compute one boolean (`is_rtl_locale(tag: &str) -> bool`), pulling compiled CLDR data into every `.wasm` build and every crate above `octara-tree`.
6. **Trivial Micro-Wrappers (`open`, `pollster`)**:
   - `open` is used in one place (Linux `xdg-open`), and `pollster` is a ~25-line `Condvar` future blocker. Both are trivial to self-own with zero external dependencies.

---

## 2. Clean-Sheet Replacement Design (Wave D5 — `0.0.10`)

### 2.1 Pluggable `MediaTransport` + Native QoS Workers (0 `tokio`, 0 `reqwest` in `octara-host-native`)

1. **Delete `tokio` and `reqwest` from `crates/octara-host-native/Cargo.toml`**:
   - Replace `tokio::runtime::Runtime` + `spawn_blocking` in `media.rs` with Octara's own lightweight bounded worker pool (`std::thread` configured with `OsHost::utility_thread_setup()` E-Core QoS + `Condvar`/channel cancellation).
2. **Introduce `MediaTransport` SPI (`crates/octara-host-native/src/media.rs` & `NativeAppOptions`)**:
   ```rust
   pub trait MediaTransport: Send + Sync + 'static {
       fn fetch(&self, req: MediaFetchRequest, cancel: &AtomicBool) -> Result<MediaFetchResponse, Arc<str>>;
   }
   ```
   - **Apple (`macOS 15.4+` / `iOS 18.4+`) default transport**: Direct **`NSURLSession`** (`NSURLSessionDataTask` with `ETag`/`If-None-Match`, redirect validation, max-bytes cap, cancellation via `cancel()`, system proxy/TLS/HTTP3 — **0 Rust HTTP/TLS crates**).
   - **Linux / Non-Apple default transport**: Clean `http-media` opt-in feature or lightweight system-curl/socket transport (or user-injected `MediaTransport` via `NativeAppOptions::with_media_transport(...)`), ensuring **zero default `tokio` / `reqwest` dependency** in `octara-host-native`.
   - Preserve 100% of existing disk cache (`media/disk.rs`), ETag revalidation, byte limits, and cancellation semantics.

### 2.2 Remove `objc2-map-kit` & `objc2-core-location` from Core `octara-host-native`

- Remove `objc2-map-kit` and `objc2-core-location` from root `Cargo.toml` and `crates/octara-host-native/Cargo.toml`.
- Delete hardcoded `kind == "map"` MapKit logic from `crates/octara-host-native/src/os/macos/native.rs`; native view mounting remains 100% generic via `octara::module::NativeViewRegistry` / `NativeViewFactory` (SPI-7).
- Any macOS underlay integration test previously exercising `kind == "map"` uses a self-contained test `NativeViewFactory` (`NSView` underlay fixture) or `modules/octara-webview` underlay, keeping 100% test coverage with **0 MapKit / 0 CoreLocation** in core.

### 2.3 Native Apple `ImageIO` Codec (0 `image` Crate on macOS & iOS)

- Move `image` in `crates/octara-host-native/Cargo.toml` to `[target.'cfg(not(any(target_os = "macos", target_os = "ios")))'.dependencies]`.
- On macOS `15.4+` and iOS `18.4+`, expand `os/apple_media.rs` (`ImageIO` `CGImageSource` + `CGImageDestination` + `CoreGraphics` `CGBitmapContext`) to handle:
  - **Decode**: PNG, JPEG, GIF (including animated frame delays), WebP, HEIC, AVIF, with strict pixel/byte limits and EXIF orientation normalization;
  - **Resize**: Hardware-accelerated `CGContext` / `CGImageSourceCreateThumbnailAtIndex` downsampling;
  - **Encode PNG**: `CGImageDestinationCreateWithData(kUTTypePNG)` for `NSPasteboard` / `UIPasteboard` clipboard images (`os/macos/services.rs`).
- Result: **0 `image` crate** compiled or linked on macOS & iOS.

### 2.4 Purge Linux Redundant Wrappers (`clipboard-rs`, `wl-clipboard-rs`, `arboard`, `x11-dl`, `open`)

- Delete `arboard`, `clipboard-rs`, `wl-clipboard-rs`, `x11-dl`, and `open` from root `Cargo.toml`, `crates/octara-host-native/Cargo.toml`, and `Cargo.lock`.
- Linux clipboard runs 100% through our direct Wayland `wl_data_device` and X11 `x11rb` ICCCM selection engines.
- Replace `open::that_detached(url)` on Linux with a 15-line `std::process::Command::new("xdg-open").arg(url)` (with `Stdio::null()` and safe URI scheme check: `http`, `https`, `mailto`, `file`).

### 2.5 Replace `icu_locale` in Layer-1 `octara-tree` with Pure-Rust BCP-47 RTL Table

- Remove `icu_locale` from root `Cargo.toml` and `crates/octara-tree/Cargo.toml`.
- Implement a self-contained ~35-line `const` BCP-47 parser in `crates/octara-tree/src/style/tree.rs`:
  1. Split locale tag on `-` / `_`;
  2. If an explicit 4-letter ISO 15924 script subtag is present (e.g., `ku-Arab`, `az-Arab`, `pa-Arab`, `sd-Arab`, `sr-Latn`), decide RTL strictly from the CLDR RTL script set (`Arab`, `Hebr`, `Syrc`, `Thaa`, `Nkoo`, `Adlm`, `Mand`, `Samr`, `Mend`, `Rohg`, `Yezi`, `Tfng`→LTR, etc.);
  3. Otherwise match the primary language subtag against CLDR's default-RTL language set (`ar`, `arc`, `ckb`, `dv`, `fa`, `he`, `iw`, `nqo`, `ps`, `sd`, `syr`, `ug`, `ur`, `yi`, etc.).
- Verify with exhaustive unit tests covering `ar`, `ar-EG`, `he-IL`, `fa-IR`, `ur-PK`, `ku-Arab`, `ku-Latn`, `zh-Hans`, `en-US`, `pa-Arab`, `pa-Guru`, `az-Arab`, `az-Latn`, `sr-Cyrl`.
- Result: **0 `icu_*` crates** in `octara-tree` and `octara-host-web` `.wasm`.

### 2.6 Self-Own `block_on` (Delete `pollster`)

- Remove `pollster` from root `Cargo.toml`, `crates/octara-render/Cargo.toml`, and `crates/octara-host-native/Cargo.toml`.
- Provide a ~25-line `pub fn block_on<F: Future>(f: F) -> F::Output` (using `std::task::Wake` + `Mutex<bool>` + `Condvar`) inside `octara-render` (non-wasm) / `octara-host-native`.

---

## 3. Hard Exit Gates (`0.0.10` Wave D5)

1. **Dependency Tree Audit (0 hits)**:
   - `cargo tree -p octara-host-native` on macOS & `aarch64-apple-ios-sim` contains **0** `tokio`, **0** `reqwest`, **0** `rustls`, **0** `hyper`, **0** `image`, **0** `objc2-map-kit`, **0** `objc2-core-location`, **0** `icu_locale`, **0** `pollster`, **0** `arboard`, **0** `open`, **0** `winit`, **0** `taffy_tree`.
   - `cargo tree -p octara-host-web --target wasm32-unknown-unknown` contains **0** `icu_locale` / `icu_*`.
   - Linux `octara-host-native/Cargo.toml` contains **0** `arboard`, **0** `clipboard-rs`, **0** `wl-clipboard-rs`, **0** `x11-dl`, **0** `open`, **0** `tokio`, **0** `reqwest`.
2. **Functional Regressions**:
   - Apple `NSURLSession` image fetch (local loopback HTTP server test with ETag 304, redirect, byte-limit rejection, cancellation) + `ImageIO` PNG/JPEG/GIF/WebP/AVIF decode & resize & clipboard PNG encode pass on macOS.
   - Pure-Rust BCP-47 RTL test suite passes 100% of CLDR script + language cases.
   - Full workspace gate green: `check_layers.py`, strict Clippy `-D warnings`, `cargo test --workspace`, `cargo test -p octara-cli --doc`, DST performance ceilings, dual Wasm + 4 modules Wasm, `aarch64-apple-ios-sim`, and Linux cross-check.
