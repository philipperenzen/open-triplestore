use super::*;
use std::collections::HashMap;

fn load(vars: &[(&str, &str)]) -> Result<Config, Vec<ConfigError>> {
    let map: HashMap<String, String> = vars
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    Config::load(&move |n| map.get(n).cloned(), &HashMap::new())
}

fn errors_of(vars: &[(&str, &str)]) -> Vec<ConfigError> {
    load(vars).expect_err("the configuration should be refused")
}

#[test]
fn one_boolean_parser_for_every_spelling() {
    for t in ["1", "true", "TRUE", "yes", "Yes", "on", "ON", " true "] {
        assert_eq!(parse::bool(t), Ok(true), "{t}");
    }
    for f in ["0", "false", "False", "no", "NO", "off", "Off"] {
        assert_eq!(parse::bool(f), Ok(false), "{f}");
    }
    for bad in ["", "y", "n", "enabled", "2", "truee", "-1"] {
        assert!(parse::bool(bad).is_err(), "{bad:?} must not parse");
    }
}

#[test]
fn every_key_type_reads_its_kind() {
    for (name, ok) in registry::type_checks() {
        assert!(
            ok,
            "{name}: the key's Rust type cannot read the setting's kind"
        );
    }
}

#[test]
fn names_and_flags_are_unique() {
    let mut names = std::collections::HashSet::new();
    let mut flags = std::collections::HashSet::new();
    for s in SPECS {
        assert!(names.insert(s.name), "{} declared twice", s.name);
        if let Some(f) = s.cli {
            assert!(flags.insert(f), "flag --{f} declared twice");
        }
        assert!(!s.doc.is_empty(), "{} has no documentation", s.name);
    }
    for (old, _) in RETIRED {
        assert!(spec(old).is_none(), "{old} is both retired and declared");
    }
}

#[test]
fn every_default_is_valid() {
    for s in SPECS {
        let defaults: Vec<&str> = match s.default {
            Def::Value(v) => vec![v],
            Def::Profile {
                development,
                production,
            } => development.into_iter().chain(production).collect(),
            Def::Unset | Def::Derived { .. } => vec![],
        };
        for d in defaults {
            if d.is_empty() && s.empty_is_value {
                continue;
            }
            assert!(
                parse::validate(s, d).is_ok(),
                "{}: default {d:?} does not validate",
                s.name
            );
        }
        if let Kind::Bool = s.kind {
            assert!(
                !matches!(s.default, Def::Unset),
                "{}: a boolean needs a default",
                s.name
            );
        }
    }
}

#[test]
fn units_follow_the_naming_rule() {
    for s in SPECS {
        let n = s.name;
        assert!(
            !n.ends_with("_SECONDS")
                && !n.ends_with("_MINUTES")
                && !n.ends_with("_HOURS")
                && !n.ends_with("_MB"),
            "{n}: durations are _MS, _SECS or _DAYS and sizes _BYTES"
        );
        if let Kind::Bytes { .. } = s.kind {
            assert!(n.ends_with("_BYTES"), "{n}: a size's name ends in _BYTES");
        }
    }
    for (name, ok) in registry::type_checks() {
        let _ = ok;
        let s = spec(name).unwrap();
        // A Duration key needs a unit in its name; `from_setting` refuses
        // one without, so reading every duration key proves the rule.
        if s.duration_unit().is_none() {
            continue;
        }
        assert!(matches!(s.kind, Kind::Int { .. }), "{name}");
    }
}

#[test]
fn every_setting_is_documented() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut text = std::fs::read_to_string(root.join(".env.example")).unwrap_or_default();
    for entry in std::fs::read_dir(root.join("docs")).expect("docs/") {
        let path = entry.unwrap().path();
        if path.extension().is_some_and(|e| e == "md") {
            text.push_str(&std::fs::read_to_string(&path).unwrap_or_default());
        }
    }
    let missing: Vec<&str> = SPECS
        .iter()
        .map(|s| s.name)
        .filter(|n| !text.contains(n))
        .collect();
    assert!(
        missing.is_empty(),
        "settings named nowhere in docs/ or .env.example: {missing:?}"
    );
}

#[test]
fn the_defaults_load() {
    let c = load(&[]).expect("an empty environment is a valid configuration");
    assert_eq!(c.get(&SPARQL_QUERY_TIMEOUT), Duration::from_secs(30));
    assert!(!c.get(&SECURE_COOKIES));
    assert!(c.get(&SERVE_FRONTEND));
    assert_eq!(c.get(&REMOTE_MAX_BYTES), 64 * 1024 * 1024);
    assert_eq!(c.get(&SERVICE_ALLOWLIST), Vec::<String>::new());
    assert_eq!(c.entry(BASE_URL.name()).source, Source::Default);
    assert!(!c.production());
}

#[test]
fn every_problem_is_reported_at_once() {
    let errors = errors_of(&[
        ("SPARQL_QUERY_TIMEOUT_SECS", "thirty"),
        ("SECURE_COOKIES", "maybe"),
        ("DCAT_PROFILE", "dcat-ap-be"),
        ("OTS_SERVICE_ALLOWLIST", "https://ok.example/, not a url"),
    ]);
    let names: Vec<&str> = errors.iter().map(|e| e.name.as_str()).collect();
    for n in [
        "SPARQL_QUERY_TIMEOUT_SECS",
        "SECURE_COOKIES",
        "DCAT_PROFILE",
        "OTS_SERVICE_ALLOWLIST",
    ] {
        assert!(names.contains(&n), "{n} missing from {names:?}");
    }
}

#[test]
fn numbers_never_fall_back() {
    for bad in ["", "abc", "-5", "1.5", "10s"] {
        if bad.is_empty() {
            continue; // empty is unset
        }
        let e = errors_of(&[("OTS_SERVICE_MAX_ROWS", bad)]);
        assert_eq!(e[0].name, "OTS_SERVICE_MAX_ROWS", "{bad}");
    }
    // Below a minimum is an error, not a clamp.
    assert!(load(&[("OTS_SERVICE_MAX_ROWS", "0")]).is_err());
    assert!(load(&[("LLM_CHAT_MAX_ROUNDS", "9")]).is_err());
    assert!(load(&[("OTS_PARALLEL_QUERY_SHARDS", "64")]).is_err());
}

#[test]
fn enums_refuse_unknown_values_and_aliases() {
    assert!(load(&[("OTS_OIDC_SESSION_POLICY", "legacy")]).is_err());
    assert!(load(&[("SMTP_TLS", "smtps")]).is_err());
    assert!(load(&[("OTS_REMOTE_AUTH", "forward")]).is_err());
    assert!(load(&[("OIDC_DEFAULT_ROLE", "admin")]).is_err());
    let c = load(&[("OTS_OIDC_SESSION_POLICY", "Scoped")]).unwrap();
    assert_eq!(c.get(&OIDC_SESSION_POLICY), "scoped", "canonical spelling");
}

#[test]
fn byte_counts_take_binary_suffixes() {
    assert_eq!(parse::bytes("1048576"), Ok(1 << 20));
    assert_eq!(parse::bytes("64MiB"), Ok(64 << 20));
    assert_eq!(parse::bytes("2 GiB"), Ok(2 << 30));
    assert_eq!(parse::bytes("8kib"), Ok(8 << 10));
    assert!(parse::bytes("64MB").is_err(), "decimal units are ambiguous");
    assert!(parse::bytes("MiB").is_err());
    let c = load(&[("OTS_MAX_UPLOAD_BYTES", "512MiB")]).unwrap();
    assert_eq!(c.get(&MAX_UPLOAD_BYTES), Some(512 << 20));
}

#[test]
fn empty_means_unset() {
    let c = load(&[("BASE_URL", "  "), ("OTS_SERVICE_MAX_ROWS", "")]).unwrap();
    assert_eq!(c.get(&BASE_URL), "http://localhost:7878");
    assert_eq!(c.entry(BASE_URL.name()).source, Source::Default);
    assert_eq!(c.get(&SERVICE_MAX_ROWS), 10_000);
}

#[test]
fn retired_names_stop_the_start_with_their_replacement() {
    let e = errors_of(&[("LLM_TIMEOUT_SECONDS", "60")]);
    assert_eq!(e[0].name, "LLM_TIMEOUT_SECONDS");
    assert!(
        e[0].message.contains("LLM_TIMEOUT_SECS"),
        "{}",
        e[0].message
    );
    assert!(load(&[("BACKUP_SCHEDULE_HOURS", "24")]).is_err());
    assert!(load(&[("ACCESS_TOKEN_EXPIRY_MINUTES", "30")]).is_err());
}

#[test]
fn a_flag_wins_over_the_environment() {
    let env: HashMap<String, String> =
        [("SPARQL_QUERY_TIMEOUT_SECS".to_string(), "60".to_string())].into();
    let cli: HashMap<String, String> =
        [("SPARQL_QUERY_TIMEOUT_SECS".to_string(), "90".to_string())].into();
    let c = Config::load(&move |n| env.get(n).cloned(), &cli).unwrap();
    assert_eq!(c.get(&SPARQL_QUERY_TIMEOUT), Duration::from_secs(90));
    assert_eq!(c.entry("SPARQL_QUERY_TIMEOUT_SECS").source, Source::Cli);
}

#[test]
fn sources_are_reported() {
    let c = load(&[
        ("BASE_URL", "https://data.example.org"),
        ("LLM_API_KEY", "file:/run/secrets/llm"),
    ])
    .unwrap();
    assert_eq!(c.entry("BASE_URL").source, Source::Env);
    assert_eq!(c.entry("LLM_API_KEY").source, Source::File);
    assert_eq!(c.entry("LLM_MODEL").source, Source::Default);
}

#[test]
fn derived_defaults_follow_their_inputs() {
    let c = load(&[("SPARQL_QUERY_TIMEOUT_SECS", "120")]).unwrap();
    assert_eq!(c.get(&SERVICE_DEADLINE), Duration::from_secs(120));
    let c = load(&[
        ("SPARQL_QUERY_TIMEOUT_SECS", "120"),
        ("OTS_SERVICE_DEADLINE_SECS", "15"),
    ])
    .unwrap();
    assert_eq!(c.get(&SERVICE_DEADLINE), Duration::from_secs(15));
}

#[test]
fn secrets_are_never_shown_but_references_are() {
    let c = load(&[
        ("LLM_API_KEY", "sk-very-secret-value"),
        ("SMTP_PASSWORD", "vault:secret/data/smtp#password"),
    ])
    .unwrap();
    let described = c.describe();
    let get = |n: &str| described.iter().find(|d| d.name == n).unwrap();
    assert_eq!(get("LLM_API_KEY").value.as_deref(), Some("[redacted]"));
    assert!(get("LLM_API_KEY").secret);
    assert_eq!(
        get("SMTP_PASSWORD").value.as_deref(),
        Some("vault:secret/data/smtp#password")
    );
    let json = serde_json::to_string(&described).unwrap();
    assert!(!json.contains("sk-very-secret-value"));
}

#[test]
fn a_malformed_secret_reference_is_an_error() {
    assert!(load(&[("LLM_API_KEY", "aws-sm:whatever")]).is_err());
}

#[test]
fn settings_that_need_each_other_are_checked_together() {
    let e = errors_of(&[("OTS_REPLICATION_ROLE", "follower")]);
    assert_eq!(e[0].name, "OTS_REPLICATION_LEADER_URL");
    let e = errors_of(&[("OTS_DL_BACKEND", "sidecar")]);
    assert_eq!(e[0].name, "OTS_REASONER_URL");
    let e = errors_of(&[("OIDC_ISSUER", "https://idp.example.org")]);
    assert_eq!(e[0].name, "OIDC_AUDIENCE");
    let e = errors_of(&[
        ("OTS_REPLICATION_ROLE", "cluster"),
        ("OTS_REPLICATION_CLUSTER", "1=http://a:7878,2=http://b:7878"),
        ("OTS_REPLICATION_CLUSTER_ID", "3"),
        ("OTS_REPLICATION_CLUSTER_SECRET", "env:CLUSTER_SECRET"),
    ]);
    assert_eq!(e[0].name, "OTS_REPLICATION_CLUSTER_ID");
}

#[test]
fn a_cleartext_idp_is_refused() {
    assert!(load(&[
        ("OIDC_ISSUER", "http://idp.example.org"),
        ("OIDC_AUDIENCE", "ots")
    ])
    .is_err());
    assert!(load(&[
        ("OIDC_ISSUER", "http://localhost:8080/realms/dev"),
        ("OIDC_AUDIENCE", "ots")
    ])
    .is_ok());
}

#[test]
fn the_live_view_reads_the_lookup_on_every_access() {
    use std::sync::{Arc, Mutex};
    let value = Arc::new(Mutex::new(Some("45".to_string())));
    let v = value.clone();
    let c = Config::live_from(move |n| {
        (n == "SPARQL_QUERY_TIMEOUT_SECS")
            .then(|| v.lock().unwrap().clone())
            .flatten()
    });
    assert_eq!(c.get(&SPARQL_QUERY_TIMEOUT), Duration::from_secs(45));
    *value.lock().unwrap() = None;
    assert_eq!(c.get(&SPARQL_QUERY_TIMEOUT), Duration::from_secs(30));
}

#[test]
#[should_panic(expected = "OTS_SERVICE_MAX_ROWS")]
fn the_live_view_panics_on_a_malformed_value_instead_of_defaulting() {
    let c = Config::live_from(|n| (n == "OTS_SERVICE_MAX_ROWS").then(|| "lots".to_string()));
    let _ = c.get(&SERVICE_MAX_ROWS);
}

const PROD: (&str, &str) = ("OTS_ENV", "production");

#[test]
fn the_production_profile_hardens_the_defaults() {
    let c = load(&[PROD]).unwrap();
    assert!(c.production());
    assert!(c.get(&SECURE_COOKIES), "secure cookies");
    assert!(c.get(&DISABLE_REGISTRATION), "registration closed");
    assert_eq!(c.get(&LDP_ROOT_ACL), "owners");
    assert_eq!(c.get(&EMBED_FRAME_ANCESTORS), "'none'");
    assert!(!c.get(&SEED_STANDARDS_DEMO), "no demo seeding");
    assert_eq!(c.get(&OIDC_SESSION_POLICY), "scoped");
    assert!(!c.get(&STORE_AUTO_RECOVER), "fail fast on a corrupt store");
    assert!(!c.get(&RATE_LIMIT_DISABLED));
    assert_eq!(c.entry(LDP_ROOT_ACL.name()).source, Source::Profile);
    // Development keeps the open defaults.
    let d = load(&[]).unwrap();
    assert!(!d.get(&DISABLE_REGISTRATION));
    assert_eq!(d.get(&LDP_ROOT_ACL), "open");
    assert_eq!(d.get(&EMBED_FRAME_ANCESTORS), "*");
    assert!(d.get(&STORE_AUTO_RECOVER));
    assert_eq!(d.get(&OIDC_SESSION_POLICY), "session");
}

#[test]
fn production_defaults_can_be_overridden_explicitly() {
    let c = load(&[
        PROD,
        ("OTS_DISABLE_REGISTRATION", "false"),
        ("LDP_ROOT_ACL", "open"),
        ("EMBED_FRAME_ANCESTORS", "https://portal.example.org"),
        ("SEED_STANDARDS_DEMO", "true"),
        ("OTS_OIDC_SESSION_POLICY", "session"),
        ("STORE_AUTO_RECOVER", "true"),
    ])
    .unwrap();
    assert!(!c.get(&DISABLE_REGISTRATION));
    assert_eq!(c.get(&LDP_ROOT_ACL), "open");
    assert_eq!(c.get(&EMBED_FRAME_ANCESTORS), "https://portal.example.org");
    assert!(c.get(&SEED_STANDARDS_DEMO));
    assert_eq!(c.get(&OIDC_SESSION_POLICY), "session");
    assert!(c.get(&STORE_AUTO_RECOVER));
    assert_eq!(c.entry(LDP_ROOT_ACL.name()).source, Source::Env);
}

#[test]
fn production_refuses_what_it_must_not_run_with() {
    let names = |vars: &[(&str, &str)]| -> Vec<String> {
        errors_of(vars).into_iter().map(|e| e.name).collect()
    };
    assert_eq!(
        names(&[PROD, ("SECURE_COOKIES", "false")]),
        ["SECURE_COOKIES"]
    );
    assert_eq!(
        names(&[PROD, ("RATE_LIMIT_DISABLED", "true")]),
        ["RATE_LIMIT_DISABLED"]
    );
    assert_eq!(
        names(&[PROD, ("CORS_ORIGINS", "https://app.example.org, *")]),
        ["CORS_ORIGINS"]
    );
    assert_eq!(
        names(&[PROD, ("LLM_API_KEY", "sk-raw-value")]),
        ["LLM_API_KEY"]
    );
    // All at once.
    assert_eq!(
        errors_of(&[
            PROD,
            ("SECURE_COOKIES", "off"),
            ("RATE_LIMIT_DISABLED", "1"),
            ("CORS_ORIGINS", "*"),
        ])
        .len(),
        3
    );
    // The same values are fine in development.
    assert!(load(&[
        ("SECURE_COOKIES", "false"),
        ("RATE_LIMIT_DISABLED", "true"),
        ("CORS_ORIGINS", "*"),
        ("LLM_API_KEY", "sk-raw-value"),
    ])
    .is_ok());
    // A reference is fine in production.
    assert!(load(&[PROD, ("LLM_API_KEY", "file:/run/secrets/llm")]).is_ok());
}

#[test]
fn secure_cookies_follow_an_https_base_url() {
    let c = load(&[("BASE_URL", "https://data.example.org")]).unwrap();
    assert!(c.get(&SECURE_COOKIES));
    let c = load(&[("BASE_URL", "http://localhost:7878")]).unwrap();
    assert!(!c.get(&SECURE_COOKIES));
    // Development may still turn them off explicitly behind https.
    let c = load(&[
        ("BASE_URL", "https://data.example.org"),
        ("SECURE_COOKIES", "false"),
    ])
    .unwrap();
    assert!(!c.get(&SECURE_COOKIES));
}

#[test]
fn an_unknown_profile_stops_the_start() {
    let e = errors_of(&[("OTS_ENV", "prod")]);
    assert_eq!(e[0].name, "OTS_ENV");
}
