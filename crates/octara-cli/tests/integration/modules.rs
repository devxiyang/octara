//! Resolve and merge module manifests through the installed command's Cargo entry point.

use serde_json::{Value, json};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const FILES: &[&str] = &[
    "ios/Info.plist",
    "ios/Module.entitlements",
    "macos/Info.plist",
    "macos/Module.entitlements",
    "apple-frameworks.json",
    "web-headers.json",
];

struct Workspace(tempfile::TempDir);

impl Workspace {
    fn new() -> Self {
        let workspace = Self(tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).unwrap());
        fs::write(workspace.path().join("Cargo.toml"), "[workspace]\nresolver = \"3\"\nmembers = [\"packages/*\"]\n").unwrap();
        workspace
    }

    fn path(&self) -> &Path {
        self.0.path()
    }

    fn package(&self, name: &str, cargo: &str) -> PathBuf {
        let path = self.path().join("packages").join(name);
        fs::create_dir_all(path.join("src")).unwrap();
        fs::write(path.join("Cargo.toml"), format!("[package]\nname = {name:?}\nversion = \"0.1.0\"\nedition = \"2024\"\n{cargo}"))
            .unwrap();
        fs::write(path.join(if name == "app" { "src/main.rs" } else { "src/lib.rs" }), if name == "app" { "fn main() {}" } else { "" })
            .unwrap();
        path
    }

    fn module(&self, name: &str, cargo: &str, fields: &str) -> PathBuf {
        let path = self.package(name, cargo).join("octara-module.toml");
        fs::write(&path, format!("[module]\nid = \"fixture.{name}\"\nschema = 1\n{fields}")).unwrap();
        path
    }

    fn run(&self, cwd: &Path, out: &str, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_octara"))
            .current_dir(cwd)
            .args(["modules", "merge", "--out-dir"])
            .arg(self.path().join(out))
            .args(args)
            .output()
            .unwrap()
    }

    fn merge(&self, cwd: &Path, out: &str, args: &[&str]) -> PathBuf {
        let result = self.run(cwd, out, args);
        assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
        self.path().join(out)
    }

    fn error(&self, cwd: &Path, args: &[&str]) -> String {
        let output = self.run(cwd, "unpublished", args);
        assert_eq!(output.status.code(), Some(2));
        assert!(output.stdout.is_empty());
        assert!(!self.path().join("unpublished").exists());
        String::from_utf8(output.stderr).unwrap()
    }
}

fn plist(out: &Path, file: &str) -> plist::Dictionary {
    plist::Value::from_file(out.join(file)).unwrap().into_dictionary().unwrap()
}

fn json(out: &Path, file: &str) -> Value {
    serde_json::from_slice(&fs::read(out.join(file)).unwrap()).unwrap()
}

#[test]
fn package_closure_merges_transitive_and_target_dependencies_once() {
    let w = Workspace::new();
    let app = w.package(
        "app",
        r#"
[dependencies]
alias = { package = "left", path = "../left" }
right = { path = "../right" }
optional = { path = "../optional", optional = true }

[target.'cfg(target_os = "macos")'.dependencies]
mac = { path = "../mac" }

[dev-dependencies]
dev-only = { path = "../dev-only" }

[build-dependencies]
build-only = { path = "../build-only" }
"#,
    );
    let deep = "[dependencies]\ndeep = { path = \"../deep\" }";
    w.module(
        "left",
        deep,
        r#"
[ios.info_plist]
Common = 42
[apple.frameworks]
ios = ["PhotosUI", "Metal"]
[web.headers]
permissions_policy = ["camera=()", "geolocation=()"]
"#,
    );
    w.module(
        "right",
        deep,
        r#"
[ios.info_plist]
Common = 42
[apple.frameworks]
ios = ["Metal", "Metal"]
[web.headers]
"Permissions-Policy" = ["camera=()", "microphone=()"]
"#,
    );
    w.module(
        "deep",
        "",
        r#"
[ios.info_plist]
NSPhotoLibraryUsageDescription = "${description} for ${name}/${name}"
Nested = { use = "${enabled}", numbers = "${numbers}" }
[ios.entitlements]
"com.example.entitlement" = "${enabled}"
[apple.frameworks]
ios = ["${framework}"]
[web.headers]
"x-fixture" = "${name}"
"#,
    );
    w.module(
        "mac",
        "",
        "[macos.entitlements]\n\"com.apple.security.files.user-selected.read-write\" = true\n[apple.frameworks]\nmacos = [\"AppKit\"]",
    );
    w.module("optional", "", "[ios.info_plist]\nOptional = true");
    for name in ["unrelated", "dev-only", "build-only"] {
        fs::write(w.package(name, "").join("octara-module.toml"), "deliberately invalid manifest").unwrap();
    }
    fs::write(
        app.join("octara.toml"),
        r#"
[application]
label = "Ignored by the module merger"
[modules."fixture.deep"]
description = "A < B & ${literal}"
name = "Example"
enabled = true
numbers = [1, 2]
framework = "UniformTypeIdentifiers"
"#,
    )
    .unwrap();

    let manifest = w.path().join("Cargo.toml");
    let out = w.merge(w.path(), "artifacts/first", &["--manifest-path", manifest.to_str().unwrap(), "-p", "app"]);
    let info = plist(&out, "ios/Info.plist");
    assert_eq!(info["Common"].as_signed_integer(), Some(42));
    assert_eq!(info["NSPhotoLibraryUsageDescription"].as_string(), Some("A < B & ${literal} for Example/Example"));
    assert!(info.get("Optional").is_none());
    let nested = info["Nested"].as_dictionary().unwrap();
    assert_eq!(nested["use"].as_boolean(), Some(true));
    let numbers = nested["numbers"].as_array().unwrap();
    assert_eq!(numbers[1].as_signed_integer(), Some(2));
    assert_eq!(plist(&out, "ios/Module.entitlements")["com.example.entitlement"].as_boolean(), Some(true));
    assert!(plist(&out, "macos/Info.plist").is_empty());
    assert_eq!(plist(&out, "macos/Module.entitlements")["com.apple.security.files.user-selected.read-write"].as_boolean(), Some(true));
    assert_eq!(json(&out, "apple-frameworks.json"), json!({"ios": ["Metal", "PhotosUI", "UniformTypeIdentifiers"], "macos": ["AppKit"]}));
    assert_eq!(
        json(&out, "web-headers.json"),
        json!({"permissions-policy": ["camera=()", "geolocation=()", "microphone=()"], "x-fixture": "Example"})
    );

    let same = w.merge(&app.join("src"), "second", &["--locked"]);
    for file in FILES {
        assert_eq!(fs::read(out.join(file)).unwrap(), fs::read(same.join(file)).unwrap(), "{file}");
    }
    let enabled = w.merge(&app, "enabled", &["--features", "optional", "--locked"]);
    assert_eq!(plist(&enabled, "ios/Info.plist")["Optional"].as_boolean(), Some(true));
    let existing = w.run(&app, "second", &[]);
    assert_eq!(existing.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&existing.stderr).contains("choose a new output directory"));
    assert_eq!(fs::read(out.join("ios/Info.plist")).unwrap(), fs::read(same.join("ios/Info.plist")).unwrap());
}

#[test]
fn base_files_preserve_bundle_metadata_and_add_module_values() {
    use plist::Value as Plist;

    let w = Workspace::new();
    let app = w.package("app", "[dependencies]\none = { path = \"../one\" }");
    w.module(
        "one",
        "",
        r#"
[ios.info_plist]
NSCameraUsageDescription = "${camera}"
NSPhotoLibraryUsageDescription = "Choose a photo"
[ios.entitlements]
"com.apple.developer.applesignin" = ["Default"]
[macos.info_plist]
NSMicrophoneUsageDescription = "Record a note"
[macos.entitlements]
"com.apple.security.files.user-selected.read-write" = true
"#,
    );
    fs::write(app.join("octara.toml"), "[modules.\"fixture.one\"]\ncamera = \"Scan a code\"\n").unwrap();

    let mut ios_info: plist::Dictionary = [
        ("CFBundleIdentifier", "com.example.ios"),
        ("CFBundleExecutable", "$(EXECUTABLE_NAME)"),
        ("CFBundleShortVersionString", "1.2.3"),
        ("CFBundleVersion", "42"),
        ("NSCameraUsageDescription", "Scan a code"),
        ("AppSetting", "${literal}"),
    ]
    .into_iter()
    .collect();
    let url_type = [("CFBundleURLSchemes", Plist::Array(vec!["example".into()]))].into_iter().collect();
    ios_info.insert("CFBundleURLTypes".into(), Plist::Array(vec![Plist::Dictionary(url_type)]));
    let ios_entitlements: plist::Dictionary =
        [("keychain-access-groups", Plist::Array(vec!["$(AppIdentifierPrefix)com.example.ios".into()]))].into_iter().collect();
    let macos_info: plist::Dictionary =
        [("CFBundleIdentifier", "com.example.mac".into()), ("AppData", Plist::Data(vec![0, 255]))].into_iter().collect();
    let macos_entitlements: plist::Dictionary = [("com.apple.security.app-sandbox", Plist::Boolean(true))].into_iter().collect();
    let bases = [
        ("--ios-info-plist", "iOS Info.plist", "ios/Info.plist", &ios_info),
        ("--ios-entitlements", "iOS.entitlements", "ios/Module.entitlements", &ios_entitlements),
        ("--macos-info-plist", "macOS Info.plist", "macos/Info.plist", &macos_info),
        ("--macos-entitlements", "macOS.entitlements", "macos/Module.entitlements", &macos_entitlements),
    ];
    let mut args = vec!["--package", "app"];
    for (flag, file, _, base) in bases {
        args.extend([flag, file]);
        let value = Plist::Dictionary(base.clone());
        let path = w.path().join(file);
        if file == "macOS Info.plist" {
            value.to_file_binary(path).unwrap();
        } else {
            value.to_file_xml(path).unwrap();
        }
    }

    let out = w.merge(w.path(), "merged", &args);
    for (_, input, output, base) in bases {
        let merged = plist(&out, output);
        for (key, value) in base {
            assert_eq!(merged.get(key), Some(value), "{output}: {key}");
        }
        assert_eq!(merged.len(), base.len() + 1, "{output}");
        assert_eq!(&plist(w.path(), input), base);
    }
    assert_eq!(plist(&out, "ios/Info.plist")["NSPhotoLibraryUsageDescription"].as_string(), Some("Choose a photo"));
    assert_eq!(plist(&out, "ios/Module.entitlements")["com.apple.developer.applesignin"], Plist::Array(vec!["Default".into()]));
    assert_eq!(plist(&out, "macos/Info.plist")["NSMicrophoneUsageDescription"].as_string(), Some("Record a note"));
    assert_eq!(plist(&out, "macos/Module.entitlements")["com.apple.security.files.user-selected.read-write"].as_boolean(), Some(true));
}

#[test]
fn base_conflicts_report_the_base_path_and_module() {
    let w = Workspace::new();
    let app = w.package("app", "[dependencies]\ntwo = { path = \"../two\" }\none = { path = \"../one\" }");
    w.module("one", "", "[macos.entitlements]\n\"com.apple.security.app-sandbox\" = true");
    w.module("two", "", "[macos.entitlements]\n\"com.apple.security.app-sandbox\" = false");
    let base = [("com.apple.security.app-sandbox", true)].into_iter().collect();
    plist::Value::Dictionary(base).to_file_xml(app.join("App.entitlements")).unwrap();

    let stderr = w.error(&app, &["--macos-entitlements", "App.entitlements"]);
    for text in [
        "[MODULE_MANIFEST_CONFLICT]",
        "macos.entitlements.com.apple.security.app-sandbox",
        "fixture.two",
        "octara-module.toml",
        "base file App.entitlements",
    ] {
        assert!(stderr.contains(text), "{stderr}");
    }
}

#[test]
fn invalid_base_files_leave_no_artifacts() {
    let w = Workspace::new();
    let app = w.package("app", "");
    let args = ["--ios-info-plist", "Base.plist"];
    let base = app.join("Base.plist");
    for invalid in [None, Some(b"<?xml version=\"1.0\"?><plist><dict>".as_slice())] {
        if let Some(bytes) = invalid {
            fs::write(&base, bytes).unwrap();
        }
        let stderr = w.error(&app, &args);
        assert!(stderr.contains("[MODULE_MANIFEST_INVALID] Base.plist: could not read base plist"), "{stderr}");
    }
    plist::Value::Boolean(true).to_file_xml(&base).unwrap();
    let stderr = w.error(&app, &args);
    assert!(stderr.contains("[MODULE_MANIFEST_INVALID] Base.plist: base plist must contain a dictionary"), "{stderr}");

    let binary = [("AppSetting", "with a NUL\0")].into_iter().collect();
    plist::Value::Dictionary(binary).to_file_binary(&base).unwrap();
    let stderr = w.error(&app, &args);
    assert!(stderr.contains("base plist entry \"AppSetting\" cannot be represented in XML"), "{stderr}");
}

#[test]
fn disagreements_report_both_modules_and_leave_no_artifacts() {
    let w = Workspace::new();
    let app = w.package("app", "[dependencies]\none = { path = \"../one\" }\ntwo = { path = \"../two\" }");
    w.module("one", "", "[macos.entitlements]\n\"example.entitlement\" = true");
    let two = w.module("two", "", "[macos.entitlements]\n\"example.entitlement\" = false");
    let stderr = w.error(&app, &[]);
    for text in ["[MODULE_MANIFEST_CONFLICT]", "macos.entitlements.example.entitlement", "fixture.one", "fixture.two", "octara-module.toml"]
    {
        assert!(stderr.contains(text), "{stderr}");
    }

    fs::write(&two, "[module]\nid = \"fixture.one\"\nschema = 1").unwrap();
    let stderr = w.error(&app, &[]);
    assert!(
        stderr.contains("[MODULE_MANIFEST_CONFLICT]") && stderr.contains("fixture.one") && stderr.contains("also declared"),
        "{stderr}"
    );

    w.module("one", "", "[web.headers]\nX_Mode = \"first\"");
    w.module("two", "", "[web.headers]\n\"x-mode\" = \"second\"");
    let stderr = w.error(&app, &[]);
    assert!(stderr.contains("[MODULE_MANIFEST_CONFLICT]") && stderr.contains("web.headers.x-mode"), "{stderr}");
}

#[test]
fn missing_variables_report_the_expansion_site_and_app_config() {
    let w = Workspace::new();
    let app = w.package("app", "[dependencies]\none = { path = \"../one\" }");
    w.module("one", "", "[ios.info_plist]\nNested = [{ usage = \"${description}\" }]");
    fs::write(app.join("octara.toml"), "[modules.\"fixture.other\"]\ndescription = \"another module's setting\"\n").unwrap();
    let stderr = w.error(&app, &[]);
    for text in
        ["[MODULE_VARIABLE_MISSING]", "ios.info_plist.Nested[0].usage", "${description}", "[modules.\"fixture.one\"]", "octara.toml"]
    {
        assert!(stderr.contains(text), "{stderr}");
    }
}

#[test]
fn schema_and_platform_field_validation_run_before_output() {
    let w = Workspace::new();
    let app = w.package("app", "[dependencies]\none = { path = \"../one\" }");
    let module = w.module("one", "", "");
    fs::write(&module, "[module]\nid = \"fixture.one\"\nschema = 2\n[future]\nvalue = true").unwrap();
    assert!(w.error(&app, &[]).contains("unsupported module schema 2"));

    for (fields, expected) in [
        ("[ios.info_plsit]\nName = \"App\"", "info_plsit"),
        ("[apple.frameworks]\nios = [false]", "invalid framework name"),
        ("[ios.info_plist]\nName = \"${unfinished\"", "unclosed"),
        ("[ios.info_plist]\nName = \"App ${flag}\"", "needs a string"),
        (
            r#"[web.headers]
"X-Name" = "abc\r\ndef""#,
            "without control characters",
        ),
    ] {
        fs::write(&module, format!("[module]\nid = \"fixture.one\"\nschema = 1\n{fields}")).unwrap();
        fs::write(app.join("octara.toml"), "[modules.\"fixture.one\"]\nflag = true\n").unwrap();
        let stderr = w.error(&app, &[]);
        assert!(stderr.contains("[MODULE_MANIFEST_INVALID]") && stderr.contains(expected), "{stderr}");
    }

    fs::write(app.join("octara.toml"), "modules = false").unwrap();
    let stderr = w.error(&app, &[]);
    assert!(stderr.contains("[MODULE_MANIFEST_INVALID]") && stderr.contains("octara.toml"), "{stderr}");
}
