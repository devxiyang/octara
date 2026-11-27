//! Validate schema 1, expand app variables, and merge the platform metadata.

use octara_model::diagnostics::codes::{MODULE_MANIFEST_CONFLICT, MODULE_MANIFEST_INVALID, MODULE_VARIABLE_MISSING};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, btree_map::Entry};
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use toml::{Table, Value};

use super::{BasePaths, diagnostic};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Header {
    id: String,
    schema: i64,
}

#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct Contributions {
    ios: Apple,
    macos: Apple,
    apple: AppleFrameworks,
    web: Web,
}

#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct Apple {
    info_plist: Table,
    entitlements: Table,
}

#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct AppleFrameworks {
    frameworks: Frameworks,
}

#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct Frameworks {
    ios: Option<Value>,
    macos: Option<Value>,
}

#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct Web {
    headers: Table,
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct AppConfig {
    modules: BTreeMap<String, Table>,
}

struct Source {
    path: PathBuf,
    id: String,
    data: Contributions,
}

pub(super) struct Merged {
    pub ios: AppleFiles,
    pub macos: AppleFiles,
    pub frameworks: FrameworkLists,
    pub headers: BTreeMap<String, HeaderValue>,
}

pub(super) struct AppleFiles {
    pub info_plist: plist::Dictionary,
    pub entitlements: plist::Dictionary,
}

#[derive(Default, Serialize)]
pub(super) struct FrameworkLists {
    ios: BTreeSet<String>,
    macos: BTreeSet<String>,
}

#[derive(PartialEq, Serialize)]
#[serde(untagged)]
pub(super) enum HeaderValue {
    Single(String),
    List(Vec<String>),
}

pub(super) fn merge(app_root: &Path, paths: Vec<PathBuf>, base: &BasePaths) -> Result<Merged, String> {
    let config_path = app_root.join("octara.toml");
    let config: AppConfig = read_toml(&config_path)?.unwrap_or_default();
    let mut sources = BTreeMap::<String, Source>::new();
    for path in paths {
        let Some(source) = Source::read(path)? else { continue };
        match sources.entry(source.id.clone()) {
            Entry::Vacant(entry) => {
                entry.insert(source);
            }
            Entry::Occupied(entry) => {
                return Err(diagnostic(
                    &MODULE_MANIFEST_CONFLICT,
                    &source.path,
                    format_args!("module `{}` is also declared in {}", source.id, entry.get().path.display()),
                ));
            }
        }
    }

    let mut ios = AppleMerge::from_base(base.ios_info_plist.as_deref(), base.ios_entitlements.as_deref())?;
    let mut macos = AppleMerge::from_base(base.macos_info_plist.as_deref(), base.macos_entitlements.as_deref())?;
    let mut frameworks = FrameworkLists::default();
    let mut headers = Fields::new();
    for source in sources.values() {
        let cx = Context { source, config_path: &config_path, variables: config.modules.get(&source.id) };
        ios.add(&cx, "ios", &source.data.ios)?;
        macos.add(&cx, "macos", &source.data.macos)?;
        frameworks.ios.extend(cx.frameworks("ios", source.data.apple.frameworks.ios.as_ref())?);
        frameworks.macos.extend(cx.frameworks("macos", source.data.apple.frameworks.macos.as_ref())?);
        for (name, value) in &source.data.web.headers {
            let field = format!("web.headers.{name}");
            let name = header_name(&cx, &field, name)?;
            let header = cx.header(&field, value)?;
            let header = match (headers.get_mut(&name), header) {
                (Some((HeaderValue::List(merged), _)), HeaderValue::List(next)) => {
                    for item in next {
                        push_unique(merged, item);
                    }
                    continue;
                }
                (_, header) => header,
            };
            merge_field(&mut headers, name, header, source, &field)?;
        }
    }
    Ok(Merged { ios: ios.finish(), macos: macos.finish(), frameworks, headers: take_values(headers).collect() })
}

fn read_toml<T: serde::de::DeserializeOwned>(path: &Path) -> Result<Option<T>, String> {
    let contents = match fs::read_to_string(path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(diagnostic(&MODULE_MANIFEST_INVALID, path, error)),
    };
    toml::from_str(&contents).map(Some).map_err(|error| diagnostic(&MODULE_MANIFEST_INVALID, path, error))
}

impl Source {
    fn read(path: PathBuf) -> Result<Option<Self>, String> {
        let Some(mut document) = read_toml::<Table>(&path)? else { return Ok(None) };
        let invalid = |message: &str| diagnostic(&MODULE_MANIFEST_INVALID, &path, message);
        let header: Header = document
            .remove("module")
            .ok_or_else(|| invalid("missing [module] with id and schema"))?
            .try_into()
            .map_err(|error: toml::de::Error| invalid(&format!("[module] {error}")))?;
        if header.schema != 1 {
            return Err(invalid(&format!("unsupported module schema {} (expected 1)", header.schema)));
        }
        if header.id.is_empty() || !header.id.bytes().all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte)) {
            return Err(invalid("module.id must be nonempty and use only letters, digits, periods, underscores or hyphens"));
        }
        let data = Value::Table(document).try_into().map_err(|error: toml::de::Error| invalid(&error.to_string()))?;
        Ok(Some(Self { path, id: header.id, data }))
    }
}

enum Origin<'a> {
    Base(&'a Path),
    Module(&'a Source),
}

type Fields<'a, T> = BTreeMap<String, (T, Origin<'a>)>;

fn merge_field<'a, T: PartialEq>(fields: &mut Fields<'a, T>, key: String, value: T, source: &'a Source, field: &str) -> Result<(), String> {
    match fields.entry(key) {
        Entry::Vacant(entry) => {
            entry.insert((value, Origin::Module(source)));
            Ok(())
        }
        Entry::Occupied(entry) if entry.get().0 == value => Ok(()),
        Entry::Occupied(entry) => {
            let previous = match &entry.get().1 {
                Origin::Base(path) => format!("base file {}", path.display()),
                Origin::Module(module) => format!("module `{}` in {}", module.id, module.path.display()),
            };
            Err(diagnostic(
                &MODULE_MANIFEST_CONFLICT,
                &source.path,
                format_args!("`{field}` from module `{}` conflicts with {previous}", source.id),
            ))
        }
    }
}

fn take_values<T>(fields: Fields<'_, T>) -> impl Iterator<Item = (String, T)> {
    fields.into_iter().map(|(key, (value, _))| (key, value))
}

fn read_base(path: Option<&Path>) -> Result<Fields<'_, plist::Value>, String> {
    let Some(path) = path else { return Ok(Fields::new()) };
    let value = plist::Value::from_file(path)
        .map_err(|error| diagnostic(&MODULE_MANIFEST_INVALID, path, format_args!("could not read base plist: {error}")))?;
    let dictionary =
        value.into_dictionary().ok_or_else(|| diagnostic(&MODULE_MANIFEST_INVALID, path, "base plist must contain a dictionary"))?;
    for (key, value) in &dictionary {
        if !xml_text(key) || !plist_value_fits_xml(value) {
            return Err(diagnostic(&MODULE_MANIFEST_INVALID, path, format_args!("base plist entry {key:?} cannot be represented in XML")));
        }
    }
    Ok(dictionary.into_iter().map(|(key, value)| (key, (value, Origin::Base(path)))).collect())
}

fn plist_value_fits_xml(value: &plist::Value) -> bool {
    match value {
        plist::Value::Array(values) => values.iter().all(plist_value_fits_xml),
        plist::Value::Dictionary(entries) => entries.iter().all(|(key, value)| xml_text(key) && plist_value_fits_xml(value)),
        plist::Value::String(text) => xml_text(text),
        plist::Value::Uid(_) => false,
        _ => true,
    }
}

struct AppleMerge<'a> {
    info_plist: Fields<'a, plist::Value>,
    entitlements: Fields<'a, plist::Value>,
}

impl<'a> AppleMerge<'a> {
    fn from_base(info_plist: Option<&'a Path>, entitlements: Option<&'a Path>) -> Result<Self, String> {
        Ok(Self { info_plist: read_base(info_plist)?, entitlements: read_base(entitlements)? })
    }

    fn add(&mut self, cx: &Context<'a>, target: &str, platform: &Apple) -> Result<(), String> {
        for (section, table, merged) in
            [("info_plist", &platform.info_plist, &mut self.info_plist), ("entitlements", &platform.entitlements, &mut self.entitlements)]
        {
            for (key, raw) in table {
                let field = format!("{target}.{section}.{key}");
                cx.plist_key(&field, key)?;
                let value = cx.expand(&field, raw)?;
                merge_field(merged, key.clone(), cx.plist(&field, value)?, cx.source, &field)?;
            }
        }
        Ok(())
    }

    fn finish(self) -> AppleFiles {
        AppleFiles { info_plist: take_values(self.info_plist).collect(), entitlements: take_values(self.entitlements).collect() }
    }
}

struct Context<'a> {
    source: &'a Source,
    config_path: &'a Path,
    variables: Option<&'a Table>,
}

impl Context<'_> {
    fn invalid(&self, field: &str, reason: &str) -> String {
        diagnostic(&MODULE_MANIFEST_INVALID, &self.source.path, format_args!("`{field}` in module `{}` {reason}", self.source.id))
    }

    fn expand(&self, field: &str, value: &Value) -> Result<Value, String> {
        match value {
            Value::String(text) => self.expand_string(field, text),
            Value::Array(values) => values
                .iter()
                .enumerate()
                .map(|(index, value)| self.expand(&format!("{field}[{index}]"), value))
                .collect::<Result<_, _>>()
                .map(Value::Array),
            Value::Table(table) => table
                .iter()
                .map(|(key, value)| self.expand(&format!("{field}.{key}"), value).map(|value| (key.clone(), value)))
                .collect::<Result<_, _>>()
                .map(Value::Table),
            value => Ok(value.clone()),
        }
    }

    fn expand_string(&self, field: &str, text: &str) -> Result<Value, String> {
        let mut result = String::new();
        let mut rest = text;
        while let Some((literal, after)) = rest.split_once("${") {
            let Some((name, suffix)) = after.split_once('}') else {
                return Err(self.invalid(field, "has an unclosed `${...}` placeholder"));
            };
            if name.is_empty() || !name.bytes().all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte)) {
                return Err(self.invalid(field, "has an invalid `${...}` placeholder name"));
            }
            let raw = self.variables.and_then(|variables| variables.get(name)).ok_or_else(|| {
                diagnostic(
                    &MODULE_VARIABLE_MISSING,
                    &self.source.path,
                    format_args!(
                        "`{field}` needs `${{{name}}}`. Set `{name}` under [modules.{:?}] in {}",
                        self.source.id,
                        self.config_path.display()
                    ),
                )
            })?;
            if rest.len() == text.len() && literal.is_empty() && suffix.is_empty() {
                return Ok(raw.clone());
            }
            let value = raw.as_str().ok_or_else(|| self.invalid(field, &format!("needs a string for embedded variable `${{{name}}}`")))?;
            result.push_str(literal);
            result.push_str(value);
            rest = suffix;
        }
        result.push_str(rest);
        Ok(Value::String(result))
    }

    fn plist(&self, field: &str, value: Value) -> Result<plist::Value, String> {
        Ok(match value {
            Value::String(value) if xml_text(&value) => plist::Value::String(value),
            Value::String(_) => return Err(self.invalid(field, "contains a character that is invalid in XML")),
            Value::Integer(value) => plist::Value::Integer(value.into()),
            Value::Float(value) if value.is_finite() => plist::Value::Real(value),
            Value::Float(_) => return Err(self.invalid(field, "must use a finite number")),
            Value::Boolean(value) => plist::Value::Boolean(value),
            Value::Datetime(value) => plist::Value::Date(
                plist::Date::from_xml_format(&value.to_string())
                    .map_err(|_| self.invalid(field, "must use a date/time with a UTC offset"))?,
            ),
            Value::Array(items) => plist::Value::Array(
                items
                    .into_iter()
                    .enumerate()
                    .map(|(index, value)| self.plist(&format!("{field}[{index}]"), value))
                    .collect::<Result<_, _>>()?,
            ),
            Value::Table(table) => plist::Value::Dictionary(
                table
                    .into_iter()
                    .map(|(key, value)| {
                        let field = format!("{field}.{key}");
                        self.plist_key(&field, &key)?;
                        Ok((key, self.plist(&field, value)?))
                    })
                    .collect::<Result<_, String>>()?,
            ),
        })
    }

    fn plist_key(&self, field: &str, key: &str) -> Result<(), String> {
        if key.is_empty() || key.contains("${") || !xml_text(key) {
            return Err(self.invalid(field, "must use a nonempty literal plist key valid in XML"));
        }
        Ok(())
    }

    fn frameworks(&self, target: &str, value: Option<&Value>) -> Result<Vec<String>, String> {
        let Some(value) = value else { return Ok(Vec::new()) };
        let field = format!("apple.frameworks.{target}");
        let Value::Array(items) = self.expand(&field, value)? else {
            return Err(self.invalid(&field, "must be an array of framework names"));
        };
        items
            .into_iter()
            .map(|item| match item {
                Value::String(name) if valid_framework(&name) => Ok(name),
                _ => Err(self.invalid(&field, "contains an invalid framework name")),
            })
            .collect()
    }

    fn header(&self, field: &str, value: &Value) -> Result<HeaderValue, String> {
        let invalid = || self.invalid(field, "must contain header strings without control characters or line breaks");
        match self.expand(field, value)? {
            Value::String(value) if header_text(&value) => Ok(HeaderValue::Single(value)),
            Value::Array(values) => {
                let mut list = Vec::new();
                for value in values {
                    let Value::String(value) = value else { return Err(invalid()) };
                    if !header_text(&value) {
                        return Err(invalid());
                    }
                    push_unique(&mut list, value);
                }
                Ok(HeaderValue::List(list))
            }
            _ => Err(invalid()),
        }
    }
}

fn xml_text(text: &str) -> bool {
    text.chars().all(|c| matches!(c, '\t' | '\n' | '\r' | '\u{20}'..='\u{d7ff}' | '\u{e000}'..='\u{fffd}' | '\u{10000}'..='\u{10ffff}'))
}

fn valid_framework(name: &str) -> bool {
    name.as_bytes().first().is_some_and(u8::is_ascii_alphanumeric)
        && name.bytes().all(|byte| byte.is_ascii_alphanumeric() || b"_-".contains(&byte))
}

fn header_name(cx: &Context<'_>, field: &str, name: &str) -> Result<String, String> {
    if name.is_empty()
        || name.contains("${")
        || !name.bytes().all(|byte| byte.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&byte))
    {
        return Err(cx.invalid(field, "must use a valid literal HTTP header name"));
    }
    Ok(name.to_ascii_lowercase().replace('_', "-"))
}

fn header_text(value: &str) -> bool {
    value.bytes().all(|byte| (byte == b'\t' || byte >= b' ') && byte != 0x7f)
}

fn push_unique(list: &mut Vec<String>, value: String) {
    if !list.contains(&value) {
        list.push(value);
    }
}
