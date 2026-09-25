/**
 * Data credits (lib/viewer/attribution): 3DBAG's CC BY 4.0 credit must reach
 * every surface that shows the bundled block — map, embed, 3D previews and the
 * Cesium globe (via the tileset's declared credits).
 */
import { describe, it, expect } from 'vitest';
import {
  THREEDBAG_CREDIT,
  carries3dbag,
  creditHtml,
  mapAttributionFor,
  tilesetCredits,
} from '../viewer/attribution';

const BAG_FILE = '/samples/schependomlaan-3dbag.city.json';

describe('creditHtml', () => {
  it('renders the 3DBAG credit verbatim, linking the copyright page and the licence', () => {
    const html = creditHtml(THREEDBAG_CREDIT);
    expect(html).toBe(
      '<a href="https://docs.3dbag.nl/en/copyright/" target="_blank" rel="noopener noreferrer">© 3DBAG by tudelft3d and 3DGI</a>' +
        ' (<a href="https://creativecommons.org/licenses/by/4.0/" target="_blank" rel="noopener noreferrer">CC BY 4.0</a>, modified)',
    );
  });

  it('escapes text and drops non-http links', () => {
    const html = creditHtml({ text: '<b>x</b> & "y"', url: 'javascript:alert(1)' });
    expect(html).toBe('&lt;b&gt;x&lt;/b&gt; &amp; &quot;y&quot;');
  });
});

describe('carries3dbag / mapAttributionFor', () => {
  it('detects 3DBAG from feed file links, fragment references included', () => {
    const zone = { files: [['cityjson', BAG_FILE]] as [string, string][] };
    const pand = { files: [['cityjson', `${BAG_FILE}#NL.IMBAG.Pand.0268100000007417`]] as [string, string][] };
    expect(carries3dbag([zone])).toBe(true);
    expect(carries3dbag([pand])).toBe(true);
    expect(mapAttributionFor([{ files: [] }, pand])).toBe(creditHtml(THREEDBAG_CREDIT));
  });

  it('detects 3DBAG from Model3D refs', () => {
    expect(carries3dbag([{ url: 'https://api.3dbag.nl/collections/pand/items/x' }])).toBe(true);
  });

  it('owes nothing for other content', () => {
    const ifc = { files: [['ifc', '/samples/duplex.ifc']] as [string, string][], url: '/x.glb' };
    expect(carries3dbag([ifc])).toBe(false);
    expect(carries3dbag(null)).toBe(false);
    expect(mapAttributionFor([ifc])).toBe('');
  });
});

describe('tilesetCredits', () => {
  it('reads the credits the server declares in asset.extras', () => {
    const asset = {
      version: '1.1',
      extras: {
        credits: [
          {
            text: '© 3DBAG by tudelft3d and 3DGI',
            url: 'https://docs.3dbag.nl/en/copyright/',
            license: 'CC BY 4.0',
            licenseUrl: 'https://creativecommons.org/licenses/by/4.0/',
            modified: true,
          },
        ],
      },
    };
    const credits = tilesetCredits(asset);
    expect(credits).toEqual([THREEDBAG_CREDIT]);
    expect(creditHtml(credits[0])).toBe(creditHtml(THREEDBAG_CREDIT));
  });

  it('ignores a missing or malformed declaration and unsafe links', () => {
    expect(tilesetCredits({ version: '1.1' })).toEqual([]);
    expect(tilesetCredits(undefined)).toEqual([]);
    expect(tilesetCredits({ extras: { credits: 'x' } })).toEqual([]);
    const [c] = tilesetCredits({ extras: { credits: [{ text: 'T', url: 'javascript:x' }, { text: ' ' }, null] } });
    expect(c).toEqual({ text: 'T', url: undefined, license: undefined, licenseUrl: undefined, modified: false });
    expect(tilesetCredits({ extras: { credits: [{ text: 'T' }, { text: ' ' }, null] } })).toHaveLength(1);
  });
});
