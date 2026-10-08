//! Human-message presentation, separate from codec errors and wire protocols.
//!
//! Keep a [`Message`] (stable ID and typed, named arguments) until the human
//! presentation boundary. A host supplies an immutable [`Catalog`] per request
//! or context; this module never reads the environment or changes process locale.
//! Only the English source catalog ships today. Missing or invalid translations
//! fall back to the complete English message, with English plural/number rules.

use std::collections::{BTreeMap, BTreeSet};

mod generated {
    include!("i18n_catalog.rs");
}

/// Exact numeric values. Codec/protocol numbers must not use this formatter.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Number {
    Signed(i64),
    Unsigned(u64),
}

impl std::fmt::Display for Number {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Signed(value) => std::fmt::Display::fmt(value, f),
            Self::Unsigned(value) => std::fmt::Display::fmt(value, f),
        }
    }
}

/// Values are inserted once, never interpreted as message syntax or markup.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Value {
    Text(String),
    /// Command names, options, model IDs and other invariant identifiers.
    Code(String),
    /// A display path, never a normalized or translated filesystem path.
    Path(String),
    Number(Number),
    Message(Box<Message>),
}

impl Value {
    fn kind(&self) -> ParameterKind {
        match self {
            Self::Text(_) => ParameterKind::Text,
            Self::Code(_) => ParameterKind::Code,
            Self::Path(_) => ParameterKind::Path,
            Self::Number(_) => ParameterKind::Number,
            Self::Message(_) => ParameterKind::Message,
        }
    }
}

/// This data, not a translated string, is suitable for structured diagnostics.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Message {
    pub id: String,
    pub args: BTreeMap<String, Value>,
}

impl Message {
    pub fn new(id: impl Into<String>) -> Self {
        Self { id: id.into(), args: BTreeMap::new() }
    }

    pub fn arg(mut self, name: impl Into<String>, value: Value) -> Self {
        self.args.insert(name.into(), value);
        self
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParameterKind { Text, Code, Path, Number, Message }

#[derive(Clone, Copy, Debug)]
pub struct Parameter {
    pub name: &'static str,
    pub kind: ParameterKind,
}

/// Translation context and argument contract are shipped with source templates.
#[derive(Clone, Copy, Debug)]
pub struct Definition {
    pub id: &'static str,
    pub context: &'static str,
    pub parameters: &'static [Parameter],
    pub other: &'static str,
    pub one: Option<&'static str>,
    pub plural: Option<&'static str>,
}

pub fn definitions() -> &'static [Definition] { generated::MESSAGES }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PluralCategory { Zero, One, Two, Few, Many, Other }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction { Ltr, Rtl }

/// A host can adapt an established localization library to this interface.
/// Catalog implementations supply their locale's plural and number rules. No
/// English two-form assumption is imposed on translations. Return `None` for a
/// missing plural variant so the entire English message is used instead.
pub trait Catalog: Send + Sync {
    fn locale(&self) -> &str;
    fn direction(&self) -> Direction;
    fn pattern(&self, id: &str, category: PluralCategory) -> Option<&str>;
    fn plural_category(&self, count: &Number) -> PluralCategory;
    fn format_number(&self, number: &Number) -> String;
}

struct English;
impl Catalog for English {
    fn locale(&self) -> &str { "en" }
    fn direction(&self) -> Direction { Direction::Ltr }
    fn pattern(&self, id: &str, category: PluralCategory) -> Option<&str> {
        let definition = definitions().iter().find(|entry| entry.id == id)?;
        Some(if category == PluralCategory::One {
            definition.one.unwrap_or(definition.other)
        } else { definition.other })
    }
    fn plural_category(&self, number: &Number) -> PluralCategory {
        if matches!(number, Number::Signed(1 | -1) | Number::Unsigned(1)) {
            PluralCategory::One
        } else { PluralCategory::Other }
    }
    fn format_number(&self, number: &Number) -> String { number.to_string() }
}
static ENGLISH: English = English;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RenderError {
    UnknownMessage,
    InvalidArguments,
    InvalidPattern,
    NestingLimit,
}

/// An immutable presentation context. It has no ambient/global SDK state.
pub struct Localizer<'a> {
    catalog: &'a dyn Catalog,
}

impl Localizer<'static> {
    pub fn english() -> Self { Self { catalog: &ENGLISH } }
}

impl<'a> Localizer<'a> {
    pub fn new(catalog: &'a dyn Catalog) -> Self { Self { catalog } }
    pub fn locale(&self) -> &str { self.catalog.locale() }
    pub fn direction(&self) -> Direction { self.catalog.direction() }

    pub fn render(&self, message: &Message) -> Result<String, RenderError> {
        self.render_at(message, 0)
    }

    fn render_at(&self, message: &Message, depth: usize) -> Result<String, RenderError> {
        if depth >= 16 { return Err(RenderError::NestingLimit); }
        let definition = definitions().iter().find(|entry| entry.id == message.id)
            .ok_or(RenderError::UnknownMessage)?;
        if message.args.len() != definition.parameters.len()
            || definition.parameters.iter().any(|parameter|
                message.args.get(parameter.name).is_none_or(|value| value.kind() != parameter.kind))
        {
            return Err(RenderError::InvalidArguments);
        }
        let category = |catalog: &dyn Catalog| match definition.plural {
            Some(name) => match &message.args[name] {
                Value::Number(number) => catalog.plural_category(number),
                _ => PluralCategory::Other,
            },
            None => PluralCategory::Other,
        };
        // Validate the pattern before formatting numbers/arguments. Fallback
        // cannot leave half a translated sentence or locale-specific number.
        let expected = definition.parameters.iter().map(|p| p.name).collect::<BTreeSet<_>>();
        let translated = self.catalog.pattern(&message.id, category(self.catalog));
        let (catalog, pattern): (&dyn Catalog, &str) = match translated {
            Some(pattern) if placeholders(pattern).is_ok_and(|names| names == expected) =>
                (self.catalog, pattern),
            _ => (&ENGLISH, ENGLISH.pattern(&message.id, category(&ENGLISH))
                .ok_or(RenderError::UnknownMessage)?),
        };
        if placeholders(pattern)? != expected { return Err(RenderError::InvalidPattern); }
        let mut arguments = BTreeMap::new();
        for (name, value) in &message.args {
            let (text, isolate) = match value {
                Value::Text(value) => (value.clone(), true),
                Value::Code(value) | Value::Path(value) => (value.clone(), true),
                Value::Number(value) => (catalog.format_number(value), false),
                Value::Message(value) =>
                    (Localizer::new(catalog).render_at(value, depth + 1)?, true),
            };
            let text = if isolate && catalog.direction() == Direction::Rtl {
                // FSI/PDI contain the direction of an inserted filename/code;
                // these marks never touch the actual argument or filesystem.
                format!("\u{2068}{text}\u{2069}")
            } else { text };
            arguments.insert(name.as_str(), text);
        }
        interpolate(pattern, &arguments)
    }
}

fn placeholders(pattern: &str) -> Result<BTreeSet<&str>, RenderError> {
    let mut names = BTreeSet::new();
    walk_pattern(pattern, |name| { names.insert(name); Ok(String::new()) })?;
    Ok(names)
}

fn interpolate(pattern: &str, arguments: &BTreeMap<&str, String>) -> Result<String, RenderError> {
    walk_pattern(pattern, |name| arguments.get(name).cloned().ok_or(RenderError::InvalidArguments))
}

fn walk_pattern<'a>(pattern: &'a str, mut argument: impl FnMut(&'a str) -> Result<String, RenderError>)
    -> Result<String, RenderError>
{
    let mut out = String::new();
    let mut rest = pattern;
    while !rest.is_empty() {
        if rest.starts_with("{{") { out.push('{'); rest = &rest[2..]; }
        else if rest.starts_with("}}") { out.push('}'); rest = &rest[2..]; }
        else if let Some(tail) = rest.strip_prefix('{') {
            let end = tail.find('}').ok_or(RenderError::InvalidPattern)?;
            let name = &tail[..end];
            if name.is_empty() || !name.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_') {
                return Err(RenderError::InvalidPattern);
            }
            out.push_str(&argument(name)?);
            rest = &tail[end + 1..];
        } else if rest.starts_with('}') { return Err(RenderError::InvalidPattern); }
        else {
            let end = rest.find(['{', '}']).unwrap_or(rest.len());
            out.push_str(&rest[..end]);
            rest = &rest[end..];
        }
    }
    Ok(out)
}

/// Pure locale preference resolution. The executable/host chooses which input
/// values to pass; library rendering never reads or modifies the environment.
/// POSIX C/POSIX explicitly requests English. Otherwise priority is explicit,
/// CIX_LOCALE, LANGUAGE, LC_ALL, LC_MESSAGES, LANG, then English fallback.
pub fn locale_preferences(
    explicit: Option<&str>, lookup: impl Fn(&str) -> Option<String>,
) -> Vec<String> {
    let selected = explicit.filter(|value| !value.trim().is_empty()).map(str::to_owned)
        .or_else(|| lookup("CIX_LOCALE").filter(|value| !value.trim().is_empty()));
    let system = ["LC_ALL", "LC_MESSAGES", "LANG"].into_iter()
        .find_map(|key| lookup(key).filter(|value| !value.trim().is_empty()));
    let raw = selected.unwrap_or_else(|| {
        if system.as_deref().is_some_and(is_c_locale) { return "en".into(); }
        lookup("LANGUAGE").filter(|value| !value.trim().is_empty())
            .or(system).unwrap_or_else(|| "en".into())
    });
    let mut result = Vec::new();
    for value in raw.split(':') {
        if let Some(locale) = normalize_locale(value) {
            // The full locale wins, then its progressively less specific tags.
            // Unicode/private extensions are removed as a unit before parents.
            if !result.contains(&locale) { result.push(locale.clone()); }
            let parts = locale.split('-').collect::<Vec<_>>();
            let base_len = parts.iter().position(|part| part.len() == 1).unwrap_or(parts.len());
            for length in (1..=base_len).rev() {
                let parent = parts[..length].join("-");
                if !result.contains(&parent) { result.push(parent); }
            }
        }
    }
    if !result.iter().any(|value| value == "en") { result.push("en".into()); }
    result
}

fn is_c_locale(value: &str) -> bool {
    matches!(value.trim().split('.').next(), Some("C" | "POSIX"))
}

/// Syntax normalization, not an IANA language-registry validation. Locale names
/// are never treated as filesystem paths. POSIX encoding suffixes are accepted;
/// unsupported POSIX modifiers fall back instead of silently selecting a script.
pub fn normalize_locale(value: &str) -> Option<String> {
    let value = value.trim();
    if is_c_locale(value) { return Some("en".into()); }
    if value.contains('@') || value.len() > 128 { return None; }
    let value = value.split('.').next()?.replace('_', "-");
    let parts = value.split('-').collect::<Vec<_>>();
    if parts.is_empty() || !(2..=8).contains(&parts[0].len())
        || !parts[0].bytes().all(|byte| byte.is_ascii_alphabetic())
        || parts.iter().skip(1).any(|part| part.is_empty() || part.len() > 8
            || !part.bytes().all(|byte| byte.is_ascii_alphanumeric()))
    { return None; }
    Some(value.to_ascii_lowercase())
}
