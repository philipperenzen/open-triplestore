//! Web Access Control (WAC) for LDP resources.
//!
//! Every LDP resource `R` has an ACL resource at `R.acl` (`{c}/.acl` for a
//! container `{c}/`). Its `acl:Authorization` nodes live in one system graph,
//! [`ACL_GRAPH`], keyed by hash IRIs under the ACL resource (`R.acl#owner`,
//! `R.acl#public`, …). LDP bodies cannot name a graph, so a resource body can
//! never plant or overwrite an authorization; the graph is a `urn:system:*`
//! graph, so it is invisible over `/sparql` and the Graph Store Protocol.
//!
//! # Evaluation
//!
//! 1. Admins pass, as everywhere else in the store.
//! 2. The server-managed owner grant of the resource (`R.acl#owner`, written
//!    when the resource is created) always applies to it.
//! 3. If the resource has client-written authorizations with `acl:accessTo R`,
//!    those are its policy. Otherwise the container chain is walked up to the
//!    root `{base}/ldp/`; the first container whose ACL has client-written
//!    authorizations with `acl:default <container>` supplies the policy. The
//!    owner grants of the containers passed on the way apply too: a container's
//!    owner reaches its members through `acl:default`, until a member gets an
//!    ACL of its own.
//! 4. Access is allowed when any applicable authorization matches the agent
//!    and lists the mode. `acl:Write` satisfies `acl:Append`; nothing else is
//!    implied.
//! 5. A lookup error refuses the request.
//!
//! The owner grant is additive on purpose. Under strict WAC an authorization
//! with `acl:accessTo R` shadows every inherited one, so writing an owner ACL on
//! every create would make each new resource private to its creator and would
//! make tightening the root ACL ineffective for anything created since. Here
//! creating a resource never changes who else may reach it; only writing its
//! `.acl` does.
//!
//! # Agents
//!
//! The store's principals are named by stable URNs, so an install that moves
//! base URL keeps its ACLs: `urn:ots:user:{id}` (`acl:agent`),
//! `urn:ots:org:{id}` and `urn:ots:group:{id}` (`acl:agentGroup`),
//! `urn:ots:role:{admin|super_admin|user|guest}` (`acl:agentClass`), plus
//! `foaf:Agent` (everyone, anonymous included) and `acl:AuthenticatedAgent`
//! (any signed-in user).

use std::collections::{BTreeMap, BTreeSet};

use oxigraph::io::{RdfFormat, RdfParser};
use oxigraph::model::{GraphName, NamedNode, NamedOrBlankNode, Term, Triple};
use oxigraph::sparql::QueryResults;

use crate::auth::db::AuthDb;
use crate::auth::middleware::AuthenticatedUser;
use crate::store::TripleStore;

// ─── Vocabulary ────────────────────────────────────────────────────────────────

/// The system graph every LDP authorization lives in.
pub const ACL_GRAPH: &str = "urn:system:ldp-acl";
pub const ACL_NS: &str = "http://www.w3.org/ns/auth/acl#";
pub const FOAF_AGENT: &str = "http://xmlns.com/foaf/0.1/Agent";
pub const ACL_AUTHENTICATED_AGENT: &str = "http://www.w3.org/ns/auth/acl#AuthenticatedAgent";
const ACL_AUTHORIZATION: &str = "http://www.w3.org/ns/auth/acl#Authorization";
const ACL_ACCESS_TO: &str = "http://www.w3.org/ns/auth/acl#accessTo";
const ACL_DEFAULT: &str = "http://www.w3.org/ns/auth/acl#default";
const ACL_AGENT: &str = "http://www.w3.org/ns/auth/acl#agent";
const ACL_AGENT_GROUP: &str = "http://www.w3.org/ns/auth/acl#agentGroup";
const ACL_AGENT_CLASS: &str = "http://www.w3.org/ns/auth/acl#agentClass";
const ACL_MODE: &str = "http://www.w3.org/ns/auth/acl#mode";
const RDF_TYPE: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";
const DCTERMS_CREATED: &str = "http://purl.org/dc/terms/created";
const XSD_DATE_TIME: &str = "http://www.w3.org/2001/XMLSchema#dateTime";

const AGENT_USER_NS: &str = "urn:ots:user:";
const AGENT_ORG_NS: &str = "urn:ots:org:";
const AGENT_GROUP_NS: &str = "urn:ots:group:";
const AGENT_ROLE_NS: &str = "urn:ots:role:";

/// Fragment of the server-managed owner authorization: `R.acl#owner`.
pub const OWNER_FRAGMENT: &str = "owner";

/// An access mode of the `acl:` vocabulary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Mode {
    Read,
    Write,
    Append,
    Control,
}

impl Mode {
    pub const ALL: [Mode; 4] = [Mode::Read, Mode::Write, Mode::Append, Mode::Control];

    pub fn iri(self) -> &'static str {
        match self {
            Mode::Read => "http://www.w3.org/ns/auth/acl#Read",
            Mode::Write => "http://www.w3.org/ns/auth/acl#Write",
            Mode::Append => "http://www.w3.org/ns/auth/acl#Append",
            Mode::Control => "http://www.w3.org/ns/auth/acl#Control",
        }
    }

    pub fn from_iri(iri: &str) -> Option<Mode> {
        Mode::ALL.into_iter().find(|m| m.iri() == iri)
    }

    /// The lower-case token `WAC-Allow` uses.
    pub fn label(self) -> &'static str {
        match self {
            Mode::Read => "read",
            Mode::Write => "write",
            Mode::Append => "append",
            Mode::Control => "control",
        }
    }
}

// ─── Agents ────────────────────────────────────────────────────────────────────

pub fn user_agent_iri(user_id: &str) -> String {
    format!("{AGENT_USER_NS}{user_id}")
}

pub fn org_agent_iri(org_id: &str) -> String {
    format!("{AGENT_ORG_NS}{org_id}")
}

pub fn group_agent_iri(group_id: &str) -> String {
    format!("{AGENT_GROUP_NS}{group_id}")
}

pub fn role_agent_iri(role: &str) -> String {
    format!("{AGENT_ROLE_NS}{role}")
}

/// The caller as WAC sees them: the principal plus the organisations and
/// groups they belong to. `user: None` is an anonymous caller.
#[derive(Debug, Clone, Default)]
pub struct Agent {
    pub user: Option<AuthenticatedUser>,
    pub orgs: Vec<String>,
    pub groups: Vec<String>,
}

impl Agent {
    pub fn anonymous() -> Self {
        Self::default()
    }

    /// Resolve a principal's memberships. Admins skip the lookups: they pass
    /// every check anyway. A failed lookup is an error the caller must treat as
    /// a refusal.
    pub fn resolve(auth_db: &AuthDb, user: Option<&AuthenticatedUser>) -> anyhow::Result<Self> {
        let Some(user) = user else {
            return Ok(Self::anonymous());
        };
        if user.is_admin() {
            return Ok(Self {
                user: Some(user.clone()),
                orgs: Vec::new(),
                groups: Vec::new(),
            });
        }
        let orgs = auth_db.get_user_org_ids(&user.user_id)?;
        let groups = auth_db
            .list_user_groups(&user.user_id)?
            .into_iter()
            .map(|g| g.id)
            .collect();
        Ok(Self {
            user: Some(user.clone()),
            orgs,
            groups,
        })
    }

    pub fn is_admin(&self) -> bool {
        self.user.as_ref().is_some_and(|u| u.is_admin())
    }

    fn agent_iri(&self) -> Option<String> {
        self.user.as_ref().map(|u| user_agent_iri(&u.user_id))
    }

    fn group_iris(&self) -> Vec<String> {
        self.orgs
            .iter()
            .map(|o| org_agent_iri(o))
            .chain(self.groups.iter().map(|g| group_agent_iri(g)))
            .collect()
    }

    fn class_iris(&self) -> Vec<String> {
        let mut classes = vec![FOAF_AGENT.to_string()];
        if let Some(u) = &self.user {
            classes.push(ACL_AUTHENTICATED_AGENT.to_string());
            classes.push(role_agent_iri(u.role.as_str()));
        }
        classes
    }
}

// ─── IRIs ──────────────────────────────────────────────────────────────────────

/// The root container `{base}/ldp/`.
pub fn root_iri(base_url: &str) -> String {
    format!("{base_url}/ldp/")
}

/// The ACL resource governing `resource_iri`: `R.acl`, which for a container
/// `{c}/` is `{c}/.acl`.
pub fn acl_iri(resource_iri: &str) -> String {
    format!("{resource_iri}.acl")
}

/// The resource an ACL IRI or request path governs, if it is one (`…/x.acl`
/// governs `…/x`, `…/c/.acl` governs `…/c/`).
pub fn governed_resource(acl_iri_or_path: &str) -> Option<&str> {
    acl_iri_or_path.strip_suffix(".acl")
}

pub fn is_container_iri(iri: &str) -> bool {
    iri.ends_with('/')
}

/// The IRI a request path under `/ldp/` names, or `None` outside it.
pub fn iri_for_request_path(base_url: &str, request_path: &str) -> Option<String> {
    let rest = request_path.strip_prefix("/ldp/")?;
    Some(format!("{base_url}/ldp/{rest}"))
}

/// The containers above `resource_iri`, nearest first, ending with `root`.
/// Empty for the root itself or for an IRI outside the root.
pub fn ancestors(resource_iri: &str, root: &str) -> Vec<String> {
    let mut out = Vec::new();
    if resource_iri == root || !resource_iri.starts_with(root) {
        return out;
    }
    let mut cur = resource_iri.trim_end_matches('/');
    while cur.len() >= root.len() {
        let Some(slash) = cur.rfind('/') else { break };
        let parent = &resource_iri[..slash + 1];
        if parent.len() < root.len() {
            break;
        }
        out.push(parent.to_string());
        if parent == root {
            break;
        }
        cur = &resource_iri[..slash];
    }
    out
}

// ─── Authorizations ────────────────────────────────────────────────────────────

/// One `acl:Authorization` node as stored.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Authorization {
    pub subject: String,
    pub access_to: BTreeSet<String>,
    pub default: BTreeSet<String>,
    pub agents: BTreeSet<String>,
    pub agent_groups: BTreeSet<String>,
    pub agent_classes: BTreeSet<String>,
    pub modes: BTreeSet<Mode>,
}

impl Authorization {
    /// Whether this node is the server-managed owner grant of the resource
    /// whose ACL it belongs to.
    pub fn is_owner_grant(&self) -> bool {
        self.subject
            .rsplit_once('#')
            .is_some_and(|(acl, frag)| frag == OWNER_FRAGMENT && acl.ends_with(".acl"))
    }

    fn matches(&self, agent: &Agent) -> bool {
        if let Some(a) = agent.agent_iri() {
            if self.agents.contains(&a) {
                return true;
            }
        }
        if agent
            .group_iris()
            .iter()
            .any(|g| self.agent_groups.contains(g))
        {
            return true;
        }
        agent
            .class_iris()
            .iter()
            .any(|c| self.agent_classes.contains(c))
    }
}

fn sparql_str(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

/// Every authorization in the ACL graph with `?a <predicate> <target>`.
fn load_authorizations(
    store: &TripleStore,
    predicate: &str,
    target: &str,
) -> Result<Vec<Authorization>, String> {
    let q = format!(
        "SELECT ?a ?p ?o WHERE {{ GRAPH <{ACL_GRAPH}> {{ ?a <{predicate}> <{target}> . ?a ?p ?o }} }}"
    );
    let mut by_subject: BTreeMap<String, Authorization> = BTreeMap::new();
    let QueryResults::Solutions(sols) = store.query(&q).map_err(|e| e.to_string())? else {
        return Ok(Vec::new());
    };
    for sol in sols {
        let sol = sol.map_err(|e| e.to_string())?;
        let Some(Term::NamedNode(a)) = sol.get("a") else {
            continue;
        };
        let Some(Term::NamedNode(p)) = sol.get("p") else {
            continue;
        };
        let Some(Term::NamedNode(o)) = sol.get("o") else {
            continue;
        };
        let auth = by_subject
            .entry(a.as_str().to_string())
            .or_insert_with(|| Authorization {
                subject: a.as_str().to_string(),
                ..Default::default()
            });
        let o = o.as_str().to_string();
        match p.as_str() {
            ACL_ACCESS_TO => {
                auth.access_to.insert(o);
            }
            ACL_DEFAULT => {
                auth.default.insert(o);
            }
            ACL_AGENT => {
                auth.agents.insert(o);
            }
            ACL_AGENT_GROUP => {
                auth.agent_groups.insert(o);
            }
            ACL_AGENT_CLASS => {
                auth.agent_classes.insert(o);
            }
            ACL_MODE => {
                if let Some(m) = Mode::from_iri(&o) {
                    auth.modes.insert(m);
                }
            }
            _ => {}
        }
    }
    Ok(by_subject.into_values().collect())
}

fn split_owner(auths: Vec<Authorization>) -> (Vec<Authorization>, Vec<Authorization>) {
    auths.into_iter().partition(|a| a.is_owner_grant())
}

/// The authorizations that govern `resource_iri` (see the module docs).
pub fn effective_authorizations(
    store: &TripleStore,
    resource_iri: &str,
    root: &str,
) -> Result<Vec<Authorization>, String> {
    let (owner, client) = split_owner(load_authorizations(store, ACL_ACCESS_TO, resource_iri)?);
    let mut effective = owner;
    if !client.is_empty() {
        effective.extend(client);
        return Ok(effective);
    }
    for container in ancestors(resource_iri, root) {
        let (owner, client) = split_owner(load_authorizations(store, ACL_DEFAULT, &container)?);
        effective.extend(owner);
        if !client.is_empty() {
            effective.extend(client);
            return Ok(effective);
        }
    }
    Ok(effective)
}

/// The modes `agent` holds under `authorizations`. `acl:Write` implies
/// `acl:Append`; nothing else is implied.
pub fn modes_for(authorizations: &[Authorization], agent: &Agent) -> BTreeSet<Mode> {
    let mut modes = BTreeSet::new();
    for auth in authorizations.iter().filter(|a| a.matches(agent)) {
        modes.extend(auth.modes.iter().copied());
    }
    if modes.contains(&Mode::Write) {
        modes.insert(Mode::Append);
    }
    modes
}

/// The modes `agent` holds on `resource_iri`; every mode for an admin.
pub fn granted_modes(
    store: &TripleStore,
    agent: &Agent,
    resource_iri: &str,
    root: &str,
) -> Result<BTreeSet<Mode>, String> {
    if agent.is_admin() {
        return Ok(Mode::ALL.into_iter().collect());
    }
    let auths = effective_authorizations(store, resource_iri, root)?;
    Ok(modes_for(&auths, agent))
}

/// Whether `agent` may perform `mode` on `resource_iri`. An `Err` is a lookup
/// failure: callers refuse the request.
pub fn allowed(
    store: &TripleStore,
    agent: &Agent,
    resource_iri: &str,
    root: &str,
    mode: Mode,
) -> Result<bool, String> {
    if agent.is_admin() {
        return Ok(true);
    }
    Ok(granted_modes(store, agent, resource_iri, root)?.contains(&mode))
}

/// The `WAC-Allow` header value: `user="read write", public="read"`.
pub fn wac_allow_header(user_modes: &BTreeSet<Mode>, public_modes: &BTreeSet<Mode>) -> String {
    let join = |modes: &BTreeSet<Mode>| {
        modes
            .iter()
            .map(|m| m.label())
            .collect::<Vec<_>>()
            .join(" ")
    };
    format!(
        "user=\"{}\", public=\"{}\"",
        join(user_modes),
        join(public_modes)
    )
}

// ─── ACL resources ─────────────────────────────────────────────────────────────

fn nt_line(t: &Triple) -> String {
    format!("{} {} {} .\n", t.subject, t.predicate, t.object)
}

fn insert_triples(store: &TripleStore, triples: &[Triple]) -> Result<(), String> {
    if triples.is_empty() {
        return Ok(());
    }
    let body: String = triples.iter().map(nt_line).collect();
    store
        .update(&format!(
            "INSERT DATA {{ GRAPH <{ACL_GRAPH}> {{ {body} }} }}"
        ))
        .map_err(|e| e.to_string())
}

fn nn(iri: &str) -> Result<NamedNode, String> {
    NamedNode::new(iri).map_err(|e| e.to_string())
}

/// Write the server-managed owner grant for a freshly created resource: the
/// creator gets Read, Write and Control on it and, for a container, on its
/// members through `acl:default` until they get an ACL of their own.
pub fn write_owner_acl(
    store: &TripleStore,
    resource_iri: &str,
    user_id: &str,
) -> Result<(), String> {
    delete_owner_acl(store, resource_iri)?;
    let subject = nn(&format!("{}#{OWNER_FRAGMENT}", acl_iri(resource_iri)))?;
    let resource = nn(resource_iri)?;
    let mut triples = vec![
        Triple::new(subject.clone(), nn(RDF_TYPE)?, nn(ACL_AUTHORIZATION)?),
        Triple::new(subject.clone(), nn(ACL_ACCESS_TO)?, resource.clone()),
        Triple::new(
            subject.clone(),
            nn(ACL_AGENT)?,
            nn(&user_agent_iri(user_id))?,
        ),
    ];
    if is_container_iri(resource_iri) {
        triples.push(Triple::new(subject.clone(), nn(ACL_DEFAULT)?, resource));
    }
    for mode in [Mode::Read, Mode::Write, Mode::Control] {
        triples.push(Triple::new(subject.clone(), nn(ACL_MODE)?, nn(mode.iri())?));
    }
    insert_triples(store, &triples)
}

fn delete_subject(store: &TripleStore, subject: &str) -> Result<(), String> {
    store
        .update(&format!(
            "DELETE WHERE {{ GRAPH <{ACL_GRAPH}> {{ <{subject}> ?p ?o }} }}"
        ))
        .map_err(|e| e.to_string())
}

fn delete_owner_acl(store: &TripleStore, resource_iri: &str) -> Result<(), String> {
    delete_subject(
        store,
        &format!("{}#{OWNER_FRAGMENT}", acl_iri(resource_iri)),
    )
}

/// Remove the client-written authorizations of `resource_iri` (every node under
/// `R.acl#` except the owner grant). The resource falls back to inheritance.
pub fn delete_client_acl(store: &TripleStore, resource_iri: &str) -> Result<(), String> {
    let prefix = sparql_str(&format!("{}#", acl_iri(resource_iri)));
    let owner = sparql_str(&format!("{}#{OWNER_FRAGMENT}", acl_iri(resource_iri)));
    store
        .update(&format!(
            "DELETE {{ GRAPH <{ACL_GRAPH}> {{ ?s ?p ?o }} }} \
             WHERE {{ GRAPH <{ACL_GRAPH}> {{ ?s ?p ?o }} \
                      FILTER(STRSTARTS(STR(?s), \"{prefix}\") && STR(?s) != \"{owner}\") }}"
        ))
        .map_err(|e| e.to_string())
}

/// Remove the whole ACL of a deleted resource, owner grant included.
pub fn delete_acl(store: &TripleStore, resource_iri: &str) -> Result<(), String> {
    delete_client_acl(store, resource_iri)?;
    delete_owner_acl(store, resource_iri)
}

/// Every triple of the ACL resource of `resource_iri`, as N-Triples.
pub fn acl_ntriples(store: &TripleStore, resource_iri: &str) -> Result<Vec<u8>, String> {
    let prefix = sparql_str(&format!("{}#", acl_iri(resource_iri)));
    let q = format!(
        "SELECT ?s ?p ?o WHERE {{ GRAPH <{ACL_GRAPH}> {{ ?s ?p ?o }} \
         FILTER(STRSTARTS(STR(?s), \"{prefix}\")) }} ORDER BY ?s ?p ?o"
    );
    let mut out = Vec::new();
    if let QueryResults::Solutions(sols) = store.query(&q).map_err(|e| e.to_string())? {
        for sol in sols {
            let sol = sol.map_err(|e| e.to_string())?;
            if let (Some(s), Some(p), Some(o)) = (sol.get("s"), sol.get("p"), sol.get("o")) {
                out.extend_from_slice(format!("{s} {p} {o} .\n").as_bytes());
            }
        }
    }
    Ok(out)
}

/// Whether `resource_iri` has client-written authorizations of its own.
pub fn has_client_acl(store: &TripleStore, resource_iri: &str) -> Result<bool, String> {
    let (_, client) = split_owner(load_authorizations(store, ACL_ACCESS_TO, resource_iri)?);
    if !client.is_empty() {
        return Ok(true);
    }
    let (_, client) = split_owner(load_authorizations(store, ACL_DEFAULT, resource_iri)?);
    Ok(!client.is_empty())
}

/// The IRIs `acl:agentClass` accepts.
fn valid_agent_class(iri: &str) -> bool {
    iri == FOAF_AGENT
        || iri == ACL_AUTHENTICATED_AGENT
        || iri
            .strip_prefix(AGENT_ROLE_NS)
            .is_some_and(|r| crate::auth::models::SystemRole::from_str(r).is_some())
}

/// Parse and validate the body of a `PUT` to `R.acl`.
///
/// Accepted: `acl:Authorization` nodes named under `R.acl#` (not `#owner`,
/// which is server-managed), each with `acl:accessTo R` and/or (for a
/// container) `acl:default R`, at least one agent, at least one mode, and no
/// other predicates. Agents must be this store's principals or the two agent
/// classes. Anything else is refused, and nothing is written.
pub fn validate_acl_body(
    body: &str,
    format: RdfFormat,
    resource_iri: &str,
) -> Result<Vec<Triple>, String> {
    let acl = acl_iri(resource_iri);
    let parser = RdfParser::from_format(format)
        .with_base_iri(acl.as_str())
        .map_err(|e| e.to_string())?;
    let mut triples = Vec::new();
    for quad in parser.for_reader(body.as_bytes()) {
        let quad = quad.map_err(|e| e.to_string())?;
        if !matches!(quad.graph_name, GraphName::DefaultGraph) {
            return Err("an ACL body may not name a graph".to_string());
        }
        triples.push(Triple::new(quad.subject, quad.predicate, quad.object));
    }

    let prefix = format!("{acl}#");
    let owner = format!("{acl}#{OWNER_FRAGMENT}");
    let mut nodes: BTreeMap<String, Authorization> = BTreeMap::new();
    let mut typed: BTreeSet<String> = BTreeSet::new();
    for t in &triples {
        let NamedOrBlankNode::NamedNode(s) = &t.subject else {
            return Err(
                "authorizations must be named under the ACL resource (<#name>), not blank nodes"
                    .to_string(),
            );
        };
        let s = s.as_str();
        if !s.starts_with(&prefix) {
            return Err(format!(
                "authorization <{s}> must be named under the ACL resource: <{prefix}…>"
            ));
        }
        if s == owner {
            return Err(format!(
                "<#{OWNER_FRAGMENT}> is the server-managed creator grant and cannot be written; \
                 use another fragment"
            ));
        }
        let Term::NamedNode(o) = &t.object else {
            return Err(format!(
                "<{}> takes an IRI object in an authorization, got {}",
                t.predicate, t.object
            ));
        };
        let o = o.as_str();
        let node = nodes.entry(s.to_string()).or_insert_with(|| Authorization {
            subject: s.to_string(),
            ..Default::default()
        });
        match t.predicate.as_str() {
            RDF_TYPE if o == ACL_AUTHORIZATION => {
                typed.insert(s.to_string());
            }
            RDF_TYPE => return Err(format!("<{s}> must be an acl:Authorization, not <{o}>")),
            ACL_ACCESS_TO if o == resource_iri => {
                node.access_to.insert(o.to_string());
            }
            ACL_ACCESS_TO => {
                return Err(format!(
                    "acl:accessTo must be <{resource_iri}>, the resource this ACL governs"
                ))
            }
            ACL_DEFAULT if o == resource_iri && is_container_iri(resource_iri) => {
                node.default.insert(o.to_string());
            }
            ACL_DEFAULT => {
                return Err(format!(
                    "acl:default must be <{resource_iri}>, and only a container has defaults"
                ))
            }
            ACL_AGENT if o.starts_with(AGENT_USER_NS) => {
                node.agents.insert(o.to_string());
            }
            ACL_AGENT => {
                return Err(format!(
                    "acl:agent must be a user of this store (<{AGENT_USER_NS}…>), got <{o}>"
                ))
            }
            ACL_AGENT_GROUP if o.starts_with(AGENT_ORG_NS) || o.starts_with(AGENT_GROUP_NS) => {
                node.agent_groups.insert(o.to_string());
            }
            ACL_AGENT_GROUP => {
                return Err(format!(
                    "acl:agentGroup must be an organisation (<{AGENT_ORG_NS}…>) or group \
                     (<{AGENT_GROUP_NS}…>) of this store, got <{o}>"
                ))
            }
            ACL_AGENT_CLASS if valid_agent_class(o) => {
                node.agent_classes.insert(o.to_string());
            }
            ACL_AGENT_CLASS => {
                return Err(format!(
                    "acl:agentClass must be foaf:Agent, acl:AuthenticatedAgent or a role \
                     (<{AGENT_ROLE_NS}…>), got <{o}>"
                ))
            }
            ACL_MODE => match Mode::from_iri(o) {
                Some(m) => {
                    node.modes.insert(m);
                }
                None => return Err(format!("unknown acl:mode <{o}>")),
            },
            other => return Err(format!("<{other}> is not allowed in an authorization")),
        }
    }
    for (s, node) in &nodes {
        if !typed.contains(s) {
            return Err(format!("<{s}> must be typed acl:Authorization"));
        }
        if node.access_to.is_empty() && node.default.is_empty() {
            return Err(format!("<{s}> needs acl:accessTo or acl:default"));
        }
        if node.agents.is_empty() && node.agent_groups.is_empty() && node.agent_classes.is_empty() {
            return Err(format!(
                "<{s}> needs at least one acl:agent, acl:agentGroup or acl:agentClass"
            ));
        }
        if node.modes.is_empty() {
            return Err(format!("<{s}> needs at least one acl:mode"));
        }
    }
    Ok(triples)
}

/// Replace the client-written authorizations of `resource_iri` with validated
/// triples (see [`validate_acl_body`]).
pub fn replace_client_acl(
    store: &TripleStore,
    resource_iri: &str,
    triples: &[Triple],
) -> Result<(), String> {
    delete_client_acl(store, resource_iri)?;
    insert_triples(store, triples)
}

// ─── Root ACL ──────────────────────────────────────────────────────────────────

/// What the root ACL is seeded with on first start (`LDP_ROOT_ACL`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RootAclPolicy {
    /// Today's behaviour: every signed-in user may read, write and append
    /// anywhere; admins get everything.
    Open,
    /// Only admins; users reach only what they create or are granted.
    Owners,
}

impl RootAclPolicy {
    /// `LDP_ROOT_ACL=open|owners`, default `open`.
    pub fn from_env() -> Self {
        match std::env::var("LDP_ROOT_ACL")
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase()
            .as_str()
        {
            "owners" => RootAclPolicy::Owners,
            _ => RootAclPolicy::Open,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            RootAclPolicy::Open => "open",
            RootAclPolicy::Owners => "owners",
        }
    }
}

/// Whether the root ACL has been seeded (a `dcterms:created` on `{root}.acl`).
pub fn root_acl_seeded(store: &TripleStore, base_url: &str) -> Result<bool, String> {
    let acl = acl_iri(&root_iri(base_url));
    let q = format!("ASK {{ GRAPH <{ACL_GRAPH}> {{ <{acl}> <{DCTERMS_CREATED}> ?t }} }}");
    match store.query(&q).map_err(|e| e.to_string())? {
        QueryResults::Boolean(b) => Ok(b),
        _ => Ok(false),
    }
}

/// Seed the root ACL once. Returns whether anything was written.
///
/// The seed is recorded with a `dcterms:created` on `{root}.acl`, so an admin
/// who later empties the root ACL is not overridden at the next start.
pub fn ensure_root_acl(
    store: &TripleStore,
    base_url: &str,
    policy: RootAclPolicy,
) -> Result<bool, String> {
    if root_acl_seeded(store, base_url)? {
        return Ok(false);
    }
    let root = root_iri(base_url);
    let acl = acl_iri(&root);
    let root_nn = nn(&root)?;
    let mut triples = Vec::new();

    let mut grant = |fragment: &str, class: &str, modes: &[Mode]| -> Result<(), String> {
        let s = nn(&format!("{acl}#{fragment}"))?;
        triples.push(Triple::new(
            s.clone(),
            nn(RDF_TYPE)?,
            nn(ACL_AUTHORIZATION)?,
        ));
        triples.push(Triple::new(s.clone(), nn(ACL_ACCESS_TO)?, root_nn.clone()));
        triples.push(Triple::new(s.clone(), nn(ACL_DEFAULT)?, root_nn.clone()));
        triples.push(Triple::new(s.clone(), nn(ACL_AGENT_CLASS)?, nn(class)?));
        for m in modes {
            triples.push(Triple::new(s.clone(), nn(ACL_MODE)?, nn(m.iri())?));
        }
        Ok(())
    };
    grant("admins", &role_agent_iri("admin"), &Mode::ALL)?;
    grant("super-admins", &role_agent_iri("super_admin"), &Mode::ALL)?;
    if policy == RootAclPolicy::Open {
        grant(
            "authenticated",
            ACL_AUTHENTICATED_AGENT,
            &[Mode::Read, Mode::Write, Mode::Append],
        )?;
    }
    let created = oxigraph::model::Literal::new_typed_literal(
        chrono::Utc::now().to_rfc3339(),
        nn(XSD_DATE_TIME)?,
    );
    triples.push(Triple::new(nn(&acl)?, nn(DCTERMS_CREATED)?, created));
    insert_triples(store, &triples)?;
    Ok(true)
}

// ─── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::models::SystemRole;

    const BASE: &str = "http://localhost:7878";

    fn user(id: &str, role: SystemRole) -> Agent {
        Agent {
            user: Some(AuthenticatedUser {
                user_id: id.to_string(),
                role,
                can_publish: false,
                write_access: true,
                can_mint_api_tokens: true,
                scopes: Vec::new(),
            }),
            orgs: Vec::new(),
            groups: Vec::new(),
        }
    }

    fn fresh_open() -> (TripleStore, String) {
        let store = TripleStore::in_memory().unwrap();
        assert!(ensure_root_acl(&store, BASE, RootAclPolicy::Open).unwrap());
        assert!(!ensure_root_acl(&store, BASE, RootAclPolicy::Open).unwrap());
        (store, root_iri(BASE))
    }

    fn put_acl(store: &TripleStore, resource: &str, turtle: &str) {
        let triples = validate_acl_body(turtle, RdfFormat::Turtle, resource).unwrap();
        replace_client_acl(store, resource, &triples).unwrap();
    }

    #[test]
    fn iri_helpers() {
        assert_eq!(acl_iri("http://h/ldp/a"), "http://h/ldp/a.acl");
        assert_eq!(acl_iri("http://h/ldp/c/"), "http://h/ldp/c/.acl");
        assert_eq!(
            governed_resource("http://h/ldp/c/.acl"),
            Some("http://h/ldp/c/")
        );
        assert_eq!(governed_resource("http://h/ldp/a"), None);
        assert_eq!(
            ancestors("http://h/ldp/a/b/c", "http://h/ldp/"),
            vec!["http://h/ldp/a/b/", "http://h/ldp/a/", "http://h/ldp/"]
        );
        assert_eq!(
            ancestors("http://h/ldp/a/b/", "http://h/ldp/"),
            vec!["http://h/ldp/a/", "http://h/ldp/"]
        );
        assert!(ancestors("http://h/ldp/", "http://h/ldp/").is_empty());
        assert_eq!(
            iri_for_request_path(BASE, "/ldp/x/y"),
            Some(format!("{BASE}/ldp/x/y"))
        );
        assert_eq!(iri_for_request_path(BASE, "/sparql"), None);
    }

    #[test]
    fn open_root_grants_authenticated_everywhere_and_anonymous_nothing() {
        let (store, root) = fresh_open();
        let alice = user("alice", SystemRole::User);
        let res = format!("{root}deep/er/x");
        for m in [Mode::Read, Mode::Write, Mode::Append] {
            assert!(allowed(&store, &alice, &res, &root, m).unwrap(), "{m:?}");
        }
        assert!(!allowed(&store, &alice, &res, &root, Mode::Control).unwrap());
        let anon = Agent::anonymous();
        assert!(!allowed(&store, &anon, &res, &root, Mode::Read).unwrap());
        assert!(allowed(&store, &anon, &res, &root, Mode::Control)
            .map(|_| true)
            .unwrap());
    }

    #[test]
    fn owners_root_grants_users_nothing_but_admins_everything() {
        let store = TripleStore::in_memory().unwrap();
        ensure_root_acl(&store, BASE, RootAclPolicy::Owners).unwrap();
        let root = root_iri(BASE);
        let bob = user("bob", SystemRole::User);
        assert!(!allowed(&store, &bob, &root, &root, Mode::Read).unwrap());
        let adm = user("adm", SystemRole::Admin);
        assert!(allowed(&store, &adm, &root, &root, Mode::Control).unwrap());
    }

    #[test]
    fn owner_grant_is_additive_and_survives_a_closed_container() {
        let (store, root) = fresh_open();
        let alice = user("alice", SystemRole::User);
        let bob = user("bob", SystemRole::User);
        let dir = format!("{root}a/");
        let doc = format!("{dir}doc");
        write_owner_acl(&store, &dir, "alice").unwrap();
        write_owner_acl(&store, &doc, "alice").unwrap();

        // Creating did not shadow the root: Bob still writes inside.
        assert!(allowed(&store, &bob, &doc, &root, Mode::Write).unwrap());
        assert!(!allowed(&store, &bob, &doc, &root, Mode::Control).unwrap());
        assert!(allowed(&store, &alice, &doc, &root, Mode::Control).unwrap());

        // Alice closes her container: only she remains, through her own ACL.
        put_acl(
            &store,
            &dir,
            &format!(
                "@prefix acl: <{ACL_NS}> .\n\
                 <#me> a acl:Authorization ; acl:accessTo <{dir}> ; acl:default <{dir}> ;\n\
                   acl:agent <urn:ots:user:alice> ; acl:mode acl:Read, acl:Write, acl:Control ."
            ),
        );
        assert!(!allowed(&store, &bob, &doc, &root, Mode::Read).unwrap());
        assert!(!allowed(&store, &bob, &dir, &root, Mode::Append).unwrap());
        assert!(allowed(&store, &alice, &doc, &root, Mode::Write).unwrap());

        // A grant to Bob on the document: Read only, not its sibling.
        put_acl(
            &store,
            &doc,
            &format!(
                "@prefix acl: <{ACL_NS}> .\n\
                 <#bob> a acl:Authorization ; acl:accessTo <{doc}> ;\n\
                   acl:agent <urn:ots:user:bob> ; acl:mode acl:Read ."
            ),
        );
        assert!(allowed(&store, &bob, &doc, &root, Mode::Read).unwrap());
        assert!(!allowed(&store, &bob, &doc, &root, Mode::Write).unwrap());
        assert!(!allowed(&store, &bob, &format!("{dir}other"), &root, Mode::Read).unwrap());
        // The owner keeps control even though the document's own ACL omits her.
        assert!(allowed(&store, &alice, &doc, &root, Mode::Control).unwrap());

        // Deleting the document's ACL puts it back under the container's policy.
        delete_acl(&store, &doc).unwrap();
        assert!(!allowed(&store, &bob, &doc, &root, Mode::Read).unwrap());
        assert!(acl_ntriples(&store, &doc).unwrap().is_empty());
        assert!(allowed(&store, &alice, &doc, &root, Mode::Write).unwrap());
    }

    #[test]
    fn groups_classes_and_public_match() {
        let (store, root) = fresh_open();
        let doc = format!("{root}g");
        put_acl(
            &store,
            &doc,
            &format!(
                "@prefix acl: <{ACL_NS}> .\n\
                 <#org> a acl:Authorization ; acl:accessTo <{doc}> ;\n\
                   acl:agentGroup <urn:ots:org:acme> ; acl:mode acl:Write .\n\
                 <#guests> a acl:Authorization ; acl:accessTo <{doc}> ;\n\
                   acl:agentClass <urn:ots:role:guest> ; acl:mode acl:Read .\n\
                 <#public> a acl:Authorization ; acl:accessTo <{doc}> ;\n\
                   acl:agentClass <{FOAF_AGENT}> ; acl:mode acl:Read ."
            ),
        );
        let mut member = user("m", SystemRole::User);
        member.orgs.push("acme".to_string());
        let modes = granted_modes(&store, &member, &doc, &root).unwrap();
        assert!(modes.contains(&Mode::Write) && modes.contains(&Mode::Append));
        assert!(
            modes.contains(&Mode::Read),
            "public read reaches members too"
        );
        assert!(!modes.contains(&Mode::Control));
        let guest = user("g", SystemRole::Guest);
        assert!(allowed(&store, &guest, &doc, &root, Mode::Read).unwrap());
        assert!(!allowed(&store, &guest, &doc, &root, Mode::Write).unwrap());
        assert!(allowed(&store, &Agent::anonymous(), &doc, &root, Mode::Read).unwrap());
        assert_eq!(
            wac_allow_header(
                &modes,
                &granted_modes(&store, &Agent::anonymous(), &doc, &root).unwrap()
            ),
            "user=\"read write append\", public=\"read\""
        );
    }

    #[test]
    fn acl_body_validation_refuses_the_wrong_shapes() {
        let doc = format!("{BASE}/ldp/d");
        let ok = |ttl: &str| validate_acl_body(ttl, RdfFormat::Turtle, &doc);
        let acl = format!("@prefix acl: <{ACL_NS}> .\n");
        assert!(ok(&format!(
            "{acl}<#a> a acl:Authorization ; acl:accessTo <{doc}> ; acl:agent <urn:ots:user:x> ; acl:mode acl:Read ."
        ))
        .is_ok());
        for bad in [
            // wrong resource
            format!("{acl}<#a> a acl:Authorization ; acl:accessTo <{BASE}/ldp/other> ; acl:agent <urn:ots:user:x> ; acl:mode acl:Read ."),
            // default on a document
            format!("{acl}<#a> a acl:Authorization ; acl:default <{doc}> ; acl:agent <urn:ots:user:x> ; acl:mode acl:Read ."),
            // reserved owner node
            format!("{acl}<#owner> a acl:Authorization ; acl:accessTo <{doc}> ; acl:agent <urn:ots:user:x> ; acl:mode acl:Read ."),
            // foreign agent
            format!("{acl}<#a> a acl:Authorization ; acl:accessTo <{doc}> ; acl:agent <https://alice.example/#me> ; acl:mode acl:Read ."),
            // unknown mode
            format!("{acl}<#a> a acl:Authorization ; acl:accessTo <{doc}> ; acl:agent <urn:ots:user:x> ; acl:mode acl:Fly ."),
            // no agent
            format!("{acl}<#a> a acl:Authorization ; acl:accessTo <{doc}> ; acl:mode acl:Read ."),
            // not typed
            format!("{acl}<#a> acl:accessTo <{doc}> ; acl:agent <urn:ots:user:x> ; acl:mode acl:Read ."),
            // foreign subject
            format!("{acl}<http://evil/#a> a acl:Authorization ; acl:accessTo <{doc}> ; acl:agent <urn:ots:user:x> ; acl:mode acl:Read ."),
            // extra predicate
            format!("{acl}<#a> a acl:Authorization ; acl:accessTo <{doc}> ; acl:agent <urn:ots:user:x> ; acl:mode acl:Read ; acl:origin <http://app> ."),
            // blank node
            format!("{acl}[] a acl:Authorization ; acl:accessTo <{doc}> ; acl:agent <urn:ots:user:x> ; acl:mode acl:Read ."),
        ] {
            assert!(ok(&bad).is_err(), "should be refused: {bad}");
        }
    }

    #[test]
    fn acl_representation_lists_owner_and_client_nodes() {
        let (store, root) = fresh_open();
        let doc = format!("{root}r");
        write_owner_acl(&store, &doc, "alice").unwrap();
        put_acl(
            &store,
            &doc,
            &format!(
                "@prefix acl: <{ACL_NS}> .\n\
                 <#bob> a acl:Authorization ; acl:accessTo <{doc}> ; acl:agent <urn:ots:user:bob> ; acl:mode acl:Read ."
            ),
        );
        let nt = String::from_utf8(acl_ntriples(&store, &doc).unwrap()).unwrap();
        assert!(nt.contains(&format!("<{doc}.acl#owner>")));
        assert!(nt.contains(&format!("<{doc}.acl#bob>")));
        assert!(has_client_acl(&store, &doc).unwrap());
        delete_client_acl(&store, &doc).unwrap();
        let nt = String::from_utf8(acl_ntriples(&store, &doc).unwrap()).unwrap();
        assert!(nt.contains("#owner>") && !nt.contains("#bob>"));
        assert!(!has_client_acl(&store, &doc).unwrap());
    }
}
