export type LicenseCategory =
  | 'public-domain'
  | 'cc'
  | 'open-data'
  | 'software'
  | 'gov'
  | 'proprietary';

export interface LicenseOption {
  iri: string;
  label: string;
  shortId: string;
  summary: string;
  url?: string;
  category: LicenseCategory;
}

export const LICENSES: LicenseOption[] = [
  {
    iri: 'https://creativecommons.org/publicdomain/zero/1.0/',
    label: 'CC0 1.0 (Public Domain Dedication)',
    shortId: 'CC0-1.0',
    summary: 'Waives all copyright. Anyone may copy, modify and redistribute, even commercially, without attribution.',
    url: 'https://creativecommons.org/publicdomain/zero/1.0/',
    category: 'public-domain',
  },
  {
    iri: 'https://creativecommons.org/licenses/by/4.0/',
    label: 'CC BY 4.0',
    shortId: 'CC-BY-4.0',
    summary: 'Reuse and redistribution allowed, including commercially, provided attribution is given.',
    url: 'https://creativecommons.org/licenses/by/4.0/',
    category: 'cc',
  },
  {
    iri: 'https://creativecommons.org/licenses/by-sa/4.0/',
    label: 'CC BY-SA 4.0',
    shortId: 'CC-BY-SA-4.0',
    summary: 'Attribution + ShareAlike: derivative works must be licensed under the same terms.',
    url: 'https://creativecommons.org/licenses/by-sa/4.0/',
    category: 'cc',
  },
  {
    iri: 'https://creativecommons.org/licenses/by-nc/4.0/',
    label: 'CC BY-NC 4.0',
    shortId: 'CC-BY-NC-4.0',
    summary: 'Attribution, non-commercial use only.',
    url: 'https://creativecommons.org/licenses/by-nc/4.0/',
    category: 'cc',
  },
  {
    iri: 'https://creativecommons.org/licenses/by-nd/4.0/',
    label: 'CC BY-ND 4.0',
    shortId: 'CC-BY-ND-4.0',
    summary: 'Attribution, no derivative works.',
    url: 'https://creativecommons.org/licenses/by-nd/4.0/',
    category: 'cc',
  },
  {
    iri: 'https://opendatacommons.org/licenses/odbl/1-0/',
    label: 'ODbL 1.0 (Open Database License)',
    shortId: 'ODbL-1.0',
    summary: 'Open license for databases: share, modify, attribute, and share-alike.',
    url: 'https://opendatacommons.org/licenses/odbl/1-0/',
    category: 'open-data',
  },
  {
    iri: 'https://opendatacommons.org/licenses/by/1-0/',
    label: 'ODC-BY 1.0',
    shortId: 'ODC-BY-1.0',
    summary: 'Open Data Commons Attribution: use freely with attribution.',
    url: 'https://opendatacommons.org/licenses/by/1-0/',
    category: 'open-data',
  },
  {
    iri: 'https://opendatacommons.org/licenses/pddl/1-0/',
    label: 'PDDL 1.0 (Public Domain Database)',
    shortId: 'PDDL-1.0',
    summary: 'Places a database in the public domain. No restrictions on reuse.',
    url: 'https://opendatacommons.org/licenses/pddl/1-0/',
    category: 'open-data',
  },
  {
    iri: 'http://www.nationalarchives.gov.uk/doc/open-government-licence/version/3/',
    label: 'Open Government Licence v3.0 (UK)',
    shortId: 'OGL-UK-3.0',
    summary: 'UK government open licence: reuse with attribution for any purpose.',
    url: 'http://www.nationalarchives.gov.uk/doc/open-government-licence/version/3/',
    category: 'gov',
  },
  {
    iri: 'https://data.europa.eu/elearning/eu-open-data-licences',
    label: 'EU Open Data — re-use under PSI Directive',
    shortId: 'EU-PSI',
    summary: 'European Public Sector Information Directive: free reuse of EU public-sector data.',
    url: 'https://data.europa.eu/en/publications/datastories/open-data-licensing',
    category: 'gov',
  },
  {
    iri: 'https://opensource.org/licenses/MIT',
    label: 'MIT License',
    shortId: 'MIT',
    summary: 'Permissive software license: do anything, keep the copyright notice.',
    url: 'https://opensource.org/licenses/MIT',
    category: 'software',
  },
  {
    iri: 'https://www.apache.org/licenses/LICENSE-2.0',
    label: 'Apache License 2.0',
    shortId: 'Apache-2.0',
    summary: 'Permissive software license with explicit patent grant.',
    url: 'https://www.apache.org/licenses/LICENSE-2.0',
    category: 'software',
  },
  {
    iri: 'https://www.gnu.org/licenses/gpl-3.0.html',
    label: 'GNU GPL v3.0',
    shortId: 'GPL-3.0',
    summary: 'Strong copyleft: derivative works must also be GPL-licensed.',
    url: 'https://www.gnu.org/licenses/gpl-3.0.html',
    category: 'software',
  },
  {
    iri: 'urn:proprietary:all-rights-reserved',
    label: 'Proprietary — All rights reserved',
    shortId: 'Proprietary',
    summary: 'No reuse rights granted. Contact the rights holder for permission.',
    category: 'proprietary',
  },
];

export const LICENSE_CATEGORY_LABEL: Record<LicenseCategory, string> = {
  'public-domain': 'Public domain',
  cc: 'Creative Commons',
  'open-data': 'Open data',
  software: 'Software',
  gov: 'Government',
  proprietary: 'Proprietary',
};

export function findLicense(iri: string | null | undefined): LicenseOption | null {
  if (!iri) return null;
  return LICENSES.find(l => l.iri === iri) || null;
}

export function searchLicenses(query: string): LicenseOption[] {
  const q = query.trim().toLowerCase();
  if (!q) return LICENSES;
  return LICENSES.filter(l =>
    l.label.toLowerCase().includes(q) ||
    l.shortId.toLowerCase().includes(q) ||
    l.summary.toLowerCase().includes(q) ||
    l.iri.toLowerCase().includes(q)
  );
}
