export interface ThemeOption {
  iri: string;
  label: string;
  summary: string;
}

const BASE = 'http://publications.europa.eu/resource/authority/data-theme/';

export const THEMES: ThemeOption[] = [
  { iri: BASE + 'AGRI', label: 'Agriculture, fisheries, forestry & food', summary: 'Farming, fishing, forestry, food production and rural affairs.' },
  { iri: BASE + 'ECON', label: 'Economy & finance', summary: 'Macroeconomics, business, banking, public finance, trade.' },
  { iri: BASE + 'EDUC', label: 'Education, culture & sport', summary: 'Schools, training, arts, heritage and sport.' },
  { iri: BASE + 'ENER', label: 'Energy', summary: 'Production, supply and consumption of energy.' },
  { iri: BASE + 'ENVI', label: 'Environment', summary: 'Ecosystems, biodiversity, pollution and climate.' },
  { iri: BASE + 'GOVE', label: 'Government & public sector', summary: 'Public administration, elections, policy and budgets.' },
  { iri: BASE + 'HEAL', label: 'Health', summary: 'Healthcare, disease, medicine and public health.' },
  { iri: BASE + 'INTR', label: 'International issues', summary: 'Foreign affairs, development aid and international relations.' },
  { iri: BASE + 'JUST', label: 'Justice, legal system & public safety', summary: 'Law, courts, crime and emergency services.' },
  { iri: BASE + 'REGI', label: 'Regions & cities', summary: 'Regional and urban data; spatial planning.' },
  { iri: BASE + 'SOCI', label: 'Population & society', summary: 'Demographics, social conditions and quality of life.' },
  { iri: BASE + 'TECH', label: 'Science & technology', summary: 'Research, innovation and information technology.' },
  { iri: BASE + 'TRAN', label: 'Transport', summary: 'Roads, rail, aviation, shipping and mobility.' },
];

export function findTheme(iri: string): ThemeOption | null {
  return THEMES.find(t => t.iri === iri) || null;
}

export function searchThemes(query: string): ThemeOption[] {
  const q = query.trim().toLowerCase();
  if (!q) return THEMES;
  return THEMES.filter(t =>
    t.label.toLowerCase().includes(q) ||
    t.summary.toLowerCase().includes(q) ||
    t.iri.toLowerCase().includes(q)
  );
}

export interface AdmsStatusOption {
  iri: string;
  label: string;
  summary: string;
}

export const ADMS_STATUSES: AdmsStatusOption[] = [
  { iri: 'http://purl.org/adms/status/Completed', label: 'Completed', summary: 'The dataset is finished and considered stable.' },
  { iri: 'http://purl.org/adms/status/UnderDevelopment', label: 'Under development', summary: 'Active work in progress; structure may change.' },
  { iri: 'http://purl.org/adms/status/Deprecated', label: 'Deprecated', summary: 'Superseded; consumers should migrate to a newer dataset.' },
  { iri: 'http://purl.org/adms/status/Withdrawn', label: 'Withdrawn', summary: 'No longer available or supported.' },
];

export function findAdmsStatus(iri: string): AdmsStatusOption | null {
  return ADMS_STATUSES.find(s => s.iri === iri) || null;
}
