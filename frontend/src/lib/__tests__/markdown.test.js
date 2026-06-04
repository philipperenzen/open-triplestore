import { describe, it, expect } from 'vitest';
import { renderMarkdown, slugify } from '../markdown.js';

describe('slugify', () => {
  it('lowercases and hyphenates, dropping punctuation', () => {
    expect(slugify('Hello World')).toBe('hello-world');
    expect(slugify('SHACL & ShEx Validation')).toBe('shacl-shex-validation');
    expect(slugify('  spaced out  ')).toBe('spaced-out');
    expect(slugify('')).toBe('');
  });
});

describe('renderMarkdown', () => {
  it('renders headings with slug ids and returns a flat TOC', () => {
    const { html, headings } = renderMarkdown('# Title\n\n## Section One\n\ntext\n\n## Section Two');
    expect(html).toContain('id="section-one"');
    expect(headings).toEqual([
      { id: 'title', text: 'Title', level: 1 },
      { id: 'section-one', text: 'Section One', level: 2 },
      { id: 'section-two', text: 'Section Two', level: 2 },
    ]);
  });

  it('dedupes repeated heading ids', () => {
    const { headings } = renderMarkdown('## Notes\n\na\n\n## Notes\n\nb');
    expect(headings.map((h) => h.id)).toEqual(['notes', 'notes-1']);
  });

  it('renders GFM tables and fenced code blocks', () => {
    const { html } = renderMarkdown('| A | B |\n|---|---|\n| 1 | 2 |\n\n```turtle\nex:a a ex:B .\n```');
    expect(html).toContain('<table>');
    expect(html).toContain('<code');
  });

  it('strips dangerous markup', () => {
    const { html } = renderMarkdown('hi <img src=x onerror=alert(1)> <script>alert(1)</script>');
    expect(html).not.toContain('onerror');
    expect(html.toLowerCase()).not.toContain('<script');
  });

  it('rewrites repo-relative .md links to /docs routes, leaving others alone', () => {
    const { html } = renderMarkdown('[a](shacl.md) [b](./dcat.md#x) [c](https://ex.org/y.md) [d](#frag)');
    expect(html).toContain('href="/docs/shacl"');
    expect(html).toContain('href="/docs/dcat#x"');
    expect(html).toContain('href="https://ex.org/y.md"');
    expect(html).toContain('href="#frag"');
  });

  it('returns empty for blank input', () => {
    expect(renderMarkdown('')).toEqual({ html: '', headings: [] });
    expect(renderMarkdown(null)).toEqual({ html: '', headings: [] });
  });
});

describe('renderMarkdown code highlighting', () => {
  it('highlights turtle fences with token spans', () => {
    const { html } = renderMarkdown('```turtle\nex:Bridge a owl:Class ;\n  skos:prefLabel "Brug"@nl .\n```');
    expect(html).toContain('class="tok-pname"'); // ex:Bridge / owl:Class
    expect(html).toContain('class="tok-str"'); // "Brug"
    expect(html).toContain('class="tok-kw"'); // the bare predicate `a`
  });

  it('flags SPARQL keywords and tokenizes braces in sparql fences', () => {
    const { html } = renderMarkdown('```sparql\nSELECT ?s WHERE { ?s a ex:T }\n```');
    expect(html).toContain('<span class="tok-kw">SELECT</span>');
    expect(html).toContain('<span class="tok-kw">WHERE</span>');
    expect(html).toContain('class="tok-punct"'); // { } now tokenized
  });

  it('highlights json fences', () => {
    const { html } = renderMarkdown('```json\n{ "regime": "rdfs" }\n```');
    expect(html).toContain('class="tok-key"');
    expect(html).toContain('class="tok-str"');
  });

  it('leaves plain and unknown-language fences untouched', () => {
    const { html } = renderMarkdown('```\nplain\n```\n\n```bash\necho hi\n```');
    expect(html).not.toContain('class="tok-');
  });

  it('keeps highlighted code escaped (no script execution)', () => {
    const { html } = renderMarkdown('```turtle\nex:x rdfs:comment "<script>alert(1)</script>" .\n```');
    expect(html.toLowerCase()).not.toContain('<script');
    expect(html).toContain('&lt;script&gt;');
  });
});
