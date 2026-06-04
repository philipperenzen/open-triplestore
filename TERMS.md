# Terms of Use (Template)

> **This is a template, not legal advice.** Review with qualified counsel and
> adapt it to your specific deployment before relying on it. These terms govern
> the **use of the Open Triplestore software**; if you operate an instance for
> other people, you are responsible for setting your **own** end-user terms (see
> §8).

---

## 1. Acceptance

By downloading, installing, running, modifying, or otherwise using the Open
Triplestore software ("the Software"), you agree to these terms and to the terms
of the Software's licence (§2). If you do not agree, do not use the Software.

## 2. The Software is licensed, not sold — AGPL-3.0 + Commons Clause

The Software is **source-available, not OSI "open source"**. It is licensed under
the **GNU Affero General Public License v3.0, as modified by the Commons Clause
License Condition v1.0** — the full text is in [`LICENSE`](LICENSE). Your rights
and obligations are defined by that licence; in case of any conflict, **the
licence prevails over this summary**.

Key points (summary only — read the [`LICENSE`](LICENSE)):

- ✅ **Free to use, self-host, study, and modify**, including by companies, at no
  cost.
- ✅ **Strong copyleft / network-use source availability (AGPL § 13).** If you run
  a modified version as a network-accessible service, you must make the
  corresponding source of your modifications available to the users of that
  service.
- ❌ **No "Selling" (Commons Clause).** You may **not** sell the Software, offer it
  as a **paid or hosted service**, or charge for support/consulting whose value
  derives substantially from the Software's functionality. The Commons Clause
  overrides the AGPL's permission to charge a fee for conveying the Software.
- 📌 **Notices.** You must preserve copyright, licence, and the Commons Clause
  notices, and pass them on with any copies or modified versions.

If you need terms beyond these (for example, a commercial licence permitting
paid hosting), contact the maintainer: **philipperenzen@gmail.com**.

## 3. "AS IS" — no warranty

**The Software is provided "AS IS", without warranty of any kind**, express or
implied, including but not limited to the implied warranties of merchantability,
fitness for a particular purpose, title, and non-infringement, consistent with
the disclaimer of warranty in the [`LICENSE`](LICENSE) (AGPL-3.0 §§ 15–16). The
entire risk as to the quality and performance of the Software is with you.

The Software is **early-stage** (see [SECURITY.md](SECURITY.md) and the
project's versioning). It may contain defects, and behaviour may change between
releases.

## 4. Limitation of liability

To the maximum extent permitted by applicable law, and consistent with the
[`LICENSE`](LICENSE), **in no event will the authors, copyright holders, or
contributors be liable** for any direct, indirect, incidental, special,
exemplary, or consequential damages (including loss of data, profits, or
business interruption) arising out of the use of or inability to use the
Software, even if advised of the possibility of such damages.

Nothing in these terms excludes or limits liability that cannot be excluded or
limited under applicable law.

## 5. Your responsibilities as an operator

If you deploy the Software, **you are the operator and (typically) the data
controller** for everything you run on it. You are responsible for, among other
things:

- securing your deployment (see [SECURITY.md](SECURITY.md) and the security
  guidance in [docs/gdpr.md](docs/gdpr.md)) — setting a strong `JWT_SECRET`,
  enabling `SECURE_COOKIES` behind HTTPS, configuring a reverse proxy/TLS,
  restricting bind addresses, and applying updates;
- the lawfulness of the data you load and process, and compliance with privacy
  and other laws (see [PRIVACY.md](PRIVACY.md) and [docs/gdpr.md](docs/gdpr.md));
- the configuration of any optional external integrations you enable (LLM
  endpoint, OIDC/SAML/OAuth identity providers, alert webhook/SMTP, validation
  platform, service registry) and compliance with **their** terms;
- managing user accounts, roles, ACLs, and access to your data;
- backups, retention, and deletion.

## 6. Acceptable use

You agree not to use the Software to:

- violate any applicable law or regulation, or infringe the rights of others;
- store or process data you have no lawful basis to handle;
- attempt to circumvent the access controls, authentication, or audit mechanisms
  of an instance you are not authorised to administer;
- gain unauthorised access to, disrupt, or overload any instance or connected
  service;
- remove or obscure licence, copyright, or attribution notices.

Operators may define additional acceptable-use rules for their own end users
(§8).

## 7. Third-party components and services

The Software depends on third-party open-source components (see
[`NOTICE`](NOTICE), [`AUTHORS`](AUTHORS), and `Cargo.toml`/`Cargo.lock`), each
under its own licence. Optional integrations contact services **you** configure;
your use of those services is governed by **their** terms and privacy policies.
The bundled web UI may load assets from public CDNs in the user's browser (see
[PRIVACY.md](PRIVACY.md) §5.1).

## 8. Operators must set their own end-user terms

These terms govern your use of the **Software**. They are **not** a contract
between you and your end users. **If you make an Open Triplestore instance
available to others, you must publish your own Terms of Service and Privacy
Notice** governing that relationship — covering at least: who may use the
service, acceptable use, data ownership and retention, availability and support,
your liability, and how users exercise their rights. Use [PRIVACY.md](PRIVACY.md)
and [docs/gdpr.md](docs/gdpr.md) as starting points, and have your terms reviewed
by counsel.

## 9. Changes

The maintainers may update the Software and any project-provided template
documents. Operators are responsible for versioning and communicating their own
end-user terms.

## 10. Governing law

The [`LICENSE`](LICENSE) governs your licence to the Software. For any additional
operator-facing terms you publish, **specify your own governing law and
jurisdiction** in the block below.

## 11. For operators — fill in

> Complete and adapt before publishing your own end-user terms; remove this quote
> block when done. **Have the result reviewed by counsel.**

- **Operating entity:** _[your legal entity name and address]_
- **Service name & scope:** _[what you offer to end users]_
- **Contact:** _[support / legal contact]_
- **Governing law & jurisdiction:** _[choose]_
- **End-user acceptable-use rules:** _[your specifics]_
- **Service levels / availability / support:** _[your specifics, if any]_
- **Fees (if any) and how they relate to the Commons Clause:** _[note that you
  may not "Sell" the Software as defined in the LICENSE]_
- **Last updated:** _[date]_

---

*See also: [LICENSE](LICENSE) · [PRIVACY.md](PRIVACY.md) ·
[GDPR compliance guide](docs/gdpr.md) · [SECURITY.md](SECURITY.md).*
