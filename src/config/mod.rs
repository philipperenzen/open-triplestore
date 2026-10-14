//! Every setting the server reads, declared once and validated at startup.
//!
//! The server is configured through environment variables (and, for a few,
//! a command-line flag). Each one is declared in [`registry`] with its kind,
//! its default, the flag that sets it if any, and a line of documentation.
//! [`Config::from_env`] reads them all at startup and returns *every*
//! problem at once — an unknown enum value, a number that does not parse, a
//! boolean spelled some other way, a retired name — and `main` refuses to
//! start on any of them. Nothing falls back silently.
//!
//! Reading a setting goes through its typed [`Key`]: `SPARQL_QUERY_TIMEOUT.get()`
//! is a `Duration`, `RATE_LIMIT_DISABLED.get()` a `bool`. In the server the
//! value comes from the snapshot `main` validated and [`install`]ed (the
//! same `Arc` sits in `AppState::config`); a process that installed none
//! (the library's tests, an embedding) reads the environment afresh on each
//! access, through the same parser, so a test that sets a variable sees it
//! and a malformed value is still an error, never a default.
//!
//! The unit rule: a duration's name ends in `_MS`, `_SECS` or `_DAYS`, and a
//! size's in `_BYTES` (its value may carry a `KiB`/`MiB`/`GiB` suffix).
//!
//! The secrets module ([`crate::secrets`]) still resolves the references a
//! secret setting holds (`env:`, `file:`, `vault:`); the registry only knows
//! the setting exists, checks that a reference is well formed, and never
//! shows the value.

pub mod admin;
pub mod parse;
mod registry;

pub use registry::*;

use std::collections::{BTreeMap, HashMap};
use std::marker::PhantomData;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

/// Where a setting's effective value came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    /// A command-line flag.
    Cli,
    /// The environment.
    Env,
    /// A file: a secret given as a `file:` reference.
    File,
    /// The production profile's default (`OTS_ENV=production`).
    Profile,
    /// The built-in default.
    Default,
}

impl Source {
    pub fn as_str(self) -> &'static str {
        match self {
            Source::Cli => "cli",
            Source::Env => "env",
            Source::File => "file",
            Source::Profile => "profile",
            Source::Default => "default",
        }
    }
}

/// What values a setting takes.
#[derive(Clone, Copy)]
pub enum Kind {
    /// `1/0/true/false/yes/no/on/off`, any case.
    Bool,
    /// A non-negative integer within `min..=max`.
    Int { min: u64, max: u64 },
    /// A byte count, optionally with a `KiB`, `MiB` or `GiB` suffix.
    Bytes { min: u64 },
    /// A decimal number in `(min, max]` (`min` excluded).
    Ratio { min: f64, max: f64 },
    /// Free text.
    Text,
    /// One of a fixed set of words, any case.
    Enum(&'static [&'static str]),
    /// A comma-separated list of words.
    List,
    /// An absolute `http(s)` URL.
    Url,
    /// A filesystem path.
    Path,
    /// A secret: a reference (`env:`, `file:`, `vault:`) or, outside the
    /// production posture, a raw value. Never shown.
    Secret,
    /// Text with a validator of its own; `what` names the form in messages.
    Custom {
        what: &'static str,
        check: fn(&str) -> Result<(), String>,
    },
}

impl Kind {
    /// A short name of the kind, for the admin view.
    pub fn label(&self) -> &'static str {
        match self {
            Kind::Bool => "bool",
            Kind::Int { .. } => "int",
            Kind::Bytes { .. } => "bytes",
            Kind::Ratio { .. } => "ratio",
            Kind::Text => "text",
            Kind::Enum(_) => "enum",
            Kind::List => "list",
            Kind::Url => "url",
            Kind::Path => "path",
            Kind::Secret => "secret",
            Kind::Custom { what, .. } => what,
        }
    }
}

/// How a setting's default is worked out.
#[derive(Clone, Copy)]
pub enum Def {
    /// No default: unset unless configured.
    Unset,
    /// A fixed default.
    Value(&'static str),
    /// One default in development, another under `OTS_ENV=production`
    /// (`None`: unset in that profile).
    Profile {
        development: Option<&'static str>,
        production: Option<&'static str>,
    },
    /// Worked out from other settings or the host (memory, cores); `label`
    /// says how, for the admin view.
    Derived {
        label: &'static str,
        f: fn(&Ctx) -> Option<String>,
    },
}

/// One setting's declaration.
#[derive(Clone, Copy)]
pub struct Spec {
    pub name: &'static str,
    pub kind: Kind,
    pub default: Def,
    /// The long command-line flag that also sets it, if any.
    pub cli: Option<&'static str>,
    pub short: Option<char>,
    /// The admin view's section.
    pub group: &'static str,
    pub doc: &'static str,
    /// An explicitly empty value is a value (most settings treat empty as
    /// unset, so that compose's `NAME=${NAME:-}` means "not configured").
    pub empty_is_value: bool,
}

impl Spec {
    pub const fn new(name: &'static str, kind: Kind, group: &'static str) -> Self {
        Spec {
            name,
            kind,
            default: Def::Unset,
            cli: None,
            short: None,
            group,
            doc: "",
            empty_is_value: false,
        }
    }
    pub const fn doc(mut self, doc: &'static str) -> Self {
        self.doc = doc;
        self
    }
    pub const fn default(mut self, value: &'static str) -> Self {
        self.default = Def::Value(value);
        self
    }
    pub const fn profile(
        mut self,
        development: Option<&'static str>,
        production: Option<&'static str>,
    ) -> Self {
        self.default = Def::Profile {
            development,
            production,
        };
        self
    }
    pub const fn derived(mut self, label: &'static str, f: fn(&Ctx) -> Option<String>) -> Self {
        self.default = Def::Derived { label, f };
        self
    }
    pub const fn cli(mut self, long: &'static str) -> Self {
        self.cli = Some(long);
        self
    }
    pub const fn short(mut self, c: char) -> Self {
        self.short = Some(c);
        self
    }
    pub const fn empty_is_value(mut self) -> Self {
        self.empty_is_value = true;
        self
    }

    /// The unit a duration setting's name declares, if it is one.
    pub fn duration_unit(&self) -> Option<Duration> {
        let n = self.name;
        if n.ends_with("_MS") {
            Some(Duration::from_millis(1))
        } else if n.ends_with("_SECS") {
            Some(Duration::from_secs(1))
        } else if n.ends_with("_DAYS") {
            Some(Duration::from_secs(86_400))
        } else {
            None
        }
    }

    /// The default as the admin view describes it.
    pub fn default_label(&self, production: bool) -> Option<String> {
        match self.default {
            Def::Unset => None,
            Def::Value(v) => Some(v.to_string()),
            Def::Profile {
                development,
                production: prod,
            } => (if production { prod } else { development }).map(str::to_string),
            Def::Derived { label, .. } => Some(format!("({label})")),
        }
    }
}

/// A typed handle on one setting. `KEY.get()` reads the effective value.
pub struct Key<T> {
    pub spec: Spec,
    _t: PhantomData<fn() -> T>,
}

impl<T> Key<T> {
    pub const fn new(spec: Spec) -> Self {
        Key {
            spec,
            _t: PhantomData,
        }
    }
    pub const fn name(&self) -> &'static str {
        self.spec.name
    }
}

impl<T: FromSetting> Key<T> {
    /// The effective value, from the installed configuration (or, with none
    /// installed, the environment as it is now).
    pub fn get(&self) -> T {
        current().get(self)
    }
}

/// A Rust type a setting's value converts to.
pub trait FromSetting: Sized {
    /// Convert the validated, normalised value (`None`: unset).
    fn from_setting(spec: &Spec, raw: Option<&str>) -> Result<Self, String>;
    /// Whether a setting of this kind can be read as `Self` (checked by the
    /// registry's tests, so a mismatch never reaches a running server).
    fn accepts(kind: &Kind) -> bool;
}

fn required<'a>(spec: &Spec, raw: Option<&'a str>) -> Result<&'a str, String> {
    raw.ok_or_else(|| format!("{} has no value and no default", spec.name))
}

impl FromSetting for bool {
    fn from_setting(spec: &Spec, raw: Option<&str>) -> Result<Self, String> {
        parse::bool(required(spec, raw)?)
    }
    fn accepts(kind: &Kind) -> bool {
        matches!(kind, Kind::Bool)
    }
}

macro_rules! int_setting {
    ($($t:ty),*) => {$(
        impl FromSetting for $t {
            fn from_setting(spec: &Spec, raw: Option<&str>) -> Result<Self, String> {
                let n = parse::number(spec, required(spec, raw)?)?;
                <$t>::try_from(n).map_err(|_| format!("{n} is too large"))
            }
            fn accepts(kind: &Kind) -> bool {
                matches!(kind, Kind::Int { .. } | Kind::Bytes { .. })
            }
        }
        impl FromSetting for Option<$t> {
            fn from_setting(spec: &Spec, raw: Option<&str>) -> Result<Self, String> {
                raw.map(|r| <$t>::from_setting(spec, Some(r))).transpose()
            }
            fn accepts(kind: &Kind) -> bool {
                <$t>::accepts(kind)
            }
        }
    )*};
}
int_setting!(u64, u32, u16, usize);

impl FromSetting for Duration {
    fn from_setting(spec: &Spec, raw: Option<&str>) -> Result<Self, String> {
        let unit = spec
            .duration_unit()
            .ok_or_else(|| format!("{} does not name a unit", spec.name))?;
        let n = parse::number(spec, required(spec, raw)?)?;
        let n = u32::try_from(n).map_err(|_| format!("{n} is too large"))?;
        Ok(unit * n)
    }
    fn accepts(kind: &Kind) -> bool {
        matches!(kind, Kind::Int { .. })
    }
}

impl FromSetting for Option<Duration> {
    fn from_setting(spec: &Spec, raw: Option<&str>) -> Result<Self, String> {
        raw.map(|r| Duration::from_setting(spec, Some(r)))
            .transpose()
    }
    fn accepts(kind: &Kind) -> bool {
        matches!(kind, Kind::Int { .. })
    }
}

impl FromSetting for f64 {
    fn from_setting(spec: &Spec, raw: Option<&str>) -> Result<Self, String> {
        parse::ratio(spec, required(spec, raw)?)
    }
    fn accepts(kind: &Kind) -> bool {
        matches!(kind, Kind::Ratio { .. })
    }
}

impl FromSetting for String {
    fn from_setting(spec: &Spec, raw: Option<&str>) -> Result<Self, String> {
        required(spec, raw).map(str::to_string)
    }
    fn accepts(kind: &Kind) -> bool {
        !matches!(
            kind,
            Kind::Bool | Kind::List | Kind::Int { .. } | Kind::Bytes { .. } | Kind::Ratio { .. }
        )
    }
}

impl FromSetting for Option<String> {
    fn from_setting(_spec: &Spec, raw: Option<&str>) -> Result<Self, String> {
        Ok(raw.map(str::to_string))
    }
    fn accepts(kind: &Kind) -> bool {
        String::accepts(kind)
    }
}

impl FromSetting for PathBuf {
    fn from_setting(spec: &Spec, raw: Option<&str>) -> Result<Self, String> {
        required(spec, raw).map(PathBuf::from)
    }
    fn accepts(kind: &Kind) -> bool {
        matches!(kind, Kind::Path)
    }
}

impl FromSetting for Option<PathBuf> {
    fn from_setting(_spec: &Spec, raw: Option<&str>) -> Result<Self, String> {
        Ok(raw.map(PathBuf::from))
    }
    fn accepts(kind: &Kind) -> bool {
        matches!(kind, Kind::Path)
    }
}

impl FromSetting for Vec<String> {
    fn from_setting(_spec: &Spec, raw: Option<&str>) -> Result<Self, String> {
        Ok(raw.map(parse::list).unwrap_or_default())
    }
    fn accepts(kind: &Kind) -> bool {
        matches!(kind, Kind::List | Kind::Custom { .. })
    }
}

/// What a derived default may look at: the other settings' effective values
/// and the profile.
pub struct Ctx<'a> {
    lookup: &'a (dyn Fn(&str) -> Option<String> + 'a),
    cli: &'a HashMap<String, String>,
    production: bool,
}

impl Ctx<'_> {
    /// Whether `OTS_ENV=production`.
    pub fn production(&self) -> bool {
        self.production
    }
    /// The effective, normalised value of another setting. A value that does
    /// not validate counts as unset here; it is reported on its own.
    pub fn value(&self, name: &str) -> Option<String> {
        let spec = spec(name)?;
        resolve(spec, self).ok().and_then(|(v, _)| v)
    }
    /// [`Self::value`] as a boolean (`false` when unset).
    pub fn flag(&self, name: &str) -> bool {
        self.value(name)
            .is_some_and(|v| parse::bool(&v).unwrap_or(false))
    }
}

/// The declaration of the setting called `name`.
pub fn spec(name: &str) -> Option<&'static Spec> {
    SPECS.iter().find(|s| s.name == name)
}

/// The configured value of `spec` as given, before validation (`None`:
/// not configured), and where it came from.
fn configured(spec: &Spec, ctx: &Ctx) -> Option<(String, Source)> {
    if let Some(v) = ctx.cli.get(spec.name) {
        return Some((v.trim().to_string(), Source::Cli));
    }
    let v = (ctx.lookup)(spec.name)?;
    let v = v.trim();
    if v.is_empty() && !spec.empty_is_value {
        return None;
    }
    let source = if matches!(spec.kind, Kind::Secret) && v.starts_with("file:") {
        Source::File
    } else {
        Source::Env
    };
    Some((v.to_string(), source))
}

/// The effective, normalised value of `spec` and its source.
fn resolve(spec: &Spec, ctx: &Ctx) -> Result<(Option<String>, Source), String> {
    if let Some((raw, source)) = configured(spec, ctx) {
        let v = parse::validate(spec, &raw)?;
        return Ok((Some(v), source));
    }
    let (v, source) = match spec.default {
        Def::Unset => (None, Source::Default),
        Def::Value(v) => (Some(v.to_string()), Source::Default),
        Def::Profile {
            development,
            production,
        } => {
            if ctx.production && production != development {
                (production.map(str::to_string), Source::Profile)
            } else if ctx.production {
                (production.map(str::to_string), Source::Default)
            } else {
                (development.map(str::to_string), Source::Default)
            }
        }
        Def::Derived { f, .. } => (f(ctx), Source::Default),
    };
    // Defaults are written in the registry and validated by its tests;
    // normalise them all the same so a derived one reads like the rest.
    let v = v.map(|v| parse::validate(spec, &v)).transpose()?;
    Ok((v, source))
}

/// One problem with the configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigError {
    pub name: String,
    pub message: String,
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.name, self.message)
    }
}

/// One setting's effective value.
#[derive(Debug, Clone)]
pub struct Entry {
    pub value: Option<String>,
    pub source: Source,
}

type LiveLookup = dyn Fn(&str) -> Option<String> + Send + Sync;

/// The effective configuration: a validated snapshot, or (with nothing
/// installed) a live view of the environment.
pub struct Config {
    entries: BTreeMap<&'static str, Entry>,
    live: Option<Box<LiveLookup>>,
    production: bool,
    /// Things worth a line in the log that do not stop the start.
    pub warnings: Vec<String>,
}

impl std::fmt::Debug for Config {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Config")
            .field("live", &self.live.is_some())
            .field("production", &self.production)
            .finish_non_exhaustive()
    }
}

impl Config {
    /// Read and validate every setting from the process environment.
    pub fn from_env() -> Result<Self, Vec<ConfigError>> {
        Self::load(&|name| std::env::var(name).ok(), &HashMap::new())
    }

    /// Read and validate every setting from `lookup`, with `cli` (setting
    /// name → value) taking precedence over it. Every error is returned.
    pub fn load(
        lookup: &dyn Fn(&str) -> Option<String>,
        cli: &HashMap<String, String>,
    ) -> Result<Self, Vec<ConfigError>> {
        let mut errors = Vec::new();
        let production = match profile_of(lookup, cli) {
            Ok(p) => p,
            Err(e) => {
                errors.push(e);
                false
            }
        };
        let ctx = Ctx {
            lookup,
            cli,
            production,
        };
        let mut entries = BTreeMap::new();
        for spec in SPECS {
            match resolve(spec, &ctx) {
                Ok((value, source)) => {
                    entries.insert(spec.name, Entry { value, source });
                }
                Err(message) => errors.push(ConfigError {
                    name: spec.name.to_string(),
                    message,
                }),
            }
        }
        for (old, hint) in RETIRED {
            if lookup(old).is_some_and(|v| !v.trim().is_empty()) {
                errors.push(ConfigError {
                    name: old.to_string(),
                    message: hint.to_string(),
                });
            }
        }
        let config = Config {
            entries,
            live: None,
            production,
            warnings: Vec::new(),
        };
        if errors.is_empty() {
            errors.extend(registry::checks(&config));
        }
        if errors.is_empty() {
            Ok(config)
        } else {
            Err(errors)
        }
    }

    /// A view that reads the environment on every access.
    pub fn live() -> Self {
        Self::live_from(|name| std::env::var(name).ok())
    }

    /// A view that reads `lookup` on every access (tests).
    pub fn live_from(lookup: impl Fn(&str) -> Option<String> + Send + Sync + 'static) -> Self {
        Config {
            entries: BTreeMap::new(),
            live: Some(Box::new(lookup)),
            production: false,
            warnings: Vec::new(),
        }
    }

    /// The effective value of `key`. A value that does not validate is a
    /// startup error in the server; reading one through a live view panics
    /// with the same message rather than substituting a default.
    pub fn get<T: FromSetting>(&self, key: &Key<T>) -> T {
        let spec = &key.spec;
        let raw = self.value(spec.name);
        T::from_setting(spec, raw.as_deref())
            .unwrap_or_else(|e| panic!("setting {}: {e}", spec.name))
    }

    /// The effective, normalised value of the setting called `name`.
    pub fn value(&self, name: &str) -> Option<String> {
        self.entry(name).value
    }

    /// The effective value and its source.
    pub fn entry(&self, name: &str) -> Entry {
        let spec = spec(name).unwrap_or_else(|| panic!("{name} is not a registered setting"));
        match &self.live {
            Some(lookup) => {
                let lookup = |n: &str| lookup(n);
                let empty = HashMap::new();
                let production = profile_of(&lookup, &empty).unwrap_or(false);
                let ctx = Ctx {
                    lookup: &lookup,
                    cli: &empty,
                    production,
                };
                match resolve(spec, &ctx) {
                    Ok((value, source)) => Entry { value, source },
                    Err(e) => panic!("setting {name}: {e}"),
                }
            }
            None => self
                .entries
                .get(spec.name)
                .cloned()
                .unwrap_or_else(|| panic!("{name} missing from the configuration")),
        }
    }

    /// Whether the production profile applies (`OTS_ENV=production`).
    pub fn production(&self) -> bool {
        match &self.live {
            Some(lookup) => profile_of(&|n: &str| lookup(n), &HashMap::new()).unwrap_or(false),
            None => self.production,
        }
    }

    /// Every setting, in registry order, as the admin view and the startup
    /// log show it: secrets redacted (a reference is shown, it is no secret).
    pub fn describe(&self) -> Vec<Described> {
        let production = self.production();
        SPECS
            .iter()
            .map(|spec| {
                let entry = self.entry(spec.name);
                Described {
                    name: spec.name,
                    group: spec.group,
                    kind: spec.kind.label(),
                    value: entry.value.as_deref().map(|v| display_value(spec, v)),
                    default: spec.default_label(production),
                    source: entry.source,
                    secret: matches!(spec.kind, Kind::Secret),
                    doc: spec.doc,
                    choices: match spec.kind {
                        Kind::Enum(c) => Some(c),
                        _ => None,
                    },
                }
            })
            .collect()
    }
}

/// What a secret setting shows: a reference as written, a raw value never.
pub fn display_value(spec: &Spec, value: &str) -> String {
    if matches!(spec.kind, Kind::Secret) && !crate::secrets::looks_like_ref(value) {
        "[redacted]".to_string()
    } else {
        value.to_string()
    }
}

/// One setting as the admin view shows it.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Described {
    pub name: &'static str,
    pub group: &'static str,
    pub kind: &'static str,
    pub value: Option<String>,
    pub default: Option<String>,
    pub source: Source,
    pub secret: bool,
    pub doc: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub choices: Option<&'static [&'static str]>,
}

/// Whether `OTS_ENV` selects the production profile.
fn profile_of(
    lookup: &dyn Fn(&str) -> Option<String>,
    cli: &HashMap<String, String>,
) -> Result<bool, ConfigError> {
    let spec = &ENV.spec;
    let ctx = Ctx {
        lookup,
        cli,
        production: false,
    };
    match configured(spec, &ctx) {
        None => Ok(false),
        Some((raw, _)) => parse::validate(spec, &raw)
            .map(|v| v == "production")
            .map_err(|message| ConfigError {
                name: spec.name.to_string(),
                message,
            }),
    }
}

static INSTALLED: OnceLock<Arc<Config>> = OnceLock::new();
static LIVE: OnceLock<Arc<Config>> = OnceLock::new();

/// Make `config` the one every [`Key::get`] reads. Once per process; a
/// second call is ignored and reported.
pub fn install(config: Arc<Config>) -> bool {
    INSTALLED.set(config).is_ok()
}

/// The configuration [`Key::get`] reads: the installed snapshot, else a
/// live view of the environment.
pub fn current() -> &'static Arc<Config> {
    INSTALLED
        .get()
        .unwrap_or_else(|| LIVE.get_or_init(|| Arc::new(Config::live())))
}

/// The `OTS_*` names among `names` that no setting declares (a typo, or a
/// name from another version), for a startup warning. A secret reference
/// may point at any variable (`env:OTS_SOMETHING`), so these only warn.
pub fn unknown_names(names: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut out: Vec<String> = names
        .into_iter()
        .filter(|n| n.starts_with("OTS_"))
        .filter(|n| spec(n).is_none() && !RETIRED.iter().any(|(old, _)| old == n))
        .collect();
    out.sort();
    out
}

/// Read a boolean whose name is not known until runtime (a seed bundle's
/// opt-out variable) with the same parser as every registered one. `Ok(None)`
/// when unset or empty.
pub fn dynamic_bool(name: &str) -> Result<Option<bool>, String> {
    match std::env::var(name) {
        Ok(v) if !v.trim().is_empty() => parse::bool(&v)
            .map(Some)
            .map_err(|e| format!("{name}: {e}")),
        _ => Ok(None),
    }
}

#[cfg(test)]
mod tests;
