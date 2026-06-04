# Organisations

Organisations group users into teams and provide shared ownership of datasets. A user can belong to multiple organisations with different roles in each.

- **Membership** — Members share access to all `members`-visibility datasets owned by the organisation. Admins can add and remove members.
- **Dataset ownership** — When creating a dataset you can assign it to an organisation. The dataset then appears under the organisation's profile and is accessible to all members.
- **URL slug** — Each organisation has a unique slug used in URLs (`/organisations/{slug}`). Slugs are lowercase alphanumeric with hyphens and cannot be changed after creation.
- **Groups** — Organisations can be subdivided into groups — smaller teams with their own members. Group membership can be targeted by endpoint and graph ACL rules, and each group can own its own API services. Managed under the organisation at `/api/organisations/{org_id}/groups`.
- **Organisation API services** — An organisation (and each of its groups) can publish saved SPARQL queries as REST endpoints scoped to its datasets — see [API Services & AI Queries](/docs/api-services).

See also: [Datasets](/docs/datasets) and [Security & Access Control](/docs/security).
