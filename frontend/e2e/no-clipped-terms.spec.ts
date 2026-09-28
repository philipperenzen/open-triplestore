import { test, expect, type Page } from '@playwright/test';

// "The layout should really try to always show the full IRI and text, not
// 'something…'".
//
// The triple table clipped almost everything it rendered. Measured on the demo
// instance before the fix: 110 elements on one screen with `scrollWidth`
// greater than `clientWidth` — a `.cell-body` 31px wide needing 280, predicate
// chips 12px wide needing 105. `td` carried
// `overflow: hidden; text-overflow: ellipsis; white-space: nowrap`, so a term
// longer than its column was cut at the column edge whatever the column was
// worth, and the flex `.cell-body` inside it had the automatic-minimum-size-of-0
// behaviour that `overflow: hidden` brings with it.
//
// jsdom has no layout, so none of the 836 component tests can see this. This
// spec measures real boxes in a real browser, in both labelling modes, and
// checks the obvious wrong fix too: wrapping must not simply move the overflow
// onto the document and give the page a horizontal scrollbar.
//
// The browse page is public, so it is driven anonymously, like query-browse.

/** Every element in the triple table whose text is a term, name or IRI. */
const TERM_SELECTOR = '.triple-row .cell-body, .triple-row .predicate, .triple-row .rdf-term';

type Clipped = { text: string; width: number; needs: number; cls: string };

async function clippedTerms(page: Page, selector: string): Promise<Clipped[]> {
  return page.evaluate((sel) => {
    const out: Clipped[] = [];
    for (const el of Array.from(document.querySelectorAll(sel))) {
      const e = el as HTMLElement;
      // A one-pixel tolerance: sub-pixel text metrics round against us.
      if (e.scrollWidth > e.clientWidth + 1) {
        out.push({
          text: (e.textContent ?? '').trim().slice(0, 60),
          width: e.clientWidth,
          needs: e.scrollWidth,
          cls: e.className.toString().replace(/svelte-\w+/g, '').trim(),
        });
      }
    }
    return out;
  }, selector);
}

function describe(clipped: Clipped[]): string {
  return clipped
    .slice(0, 6)
    .map((c) => `  ${c.cls || '?'}: "${c.text}" is ${c.width}px, needs ${c.needs}px`)
    .join('\n');
}

async function openBrowseWithRows(page: Page): Promise<void> {
  await page.goto('/browse');
  await expect(page.locator('.triple-row').first()).toBeVisible({ timeout: 20_000 });
}

/** The toolbar switch between prefixed names and whole IRIs. */
async function setFullIris(page: Page, full: boolean): Promise<void> {
  const button = page.getByRole('button', { name: full ? 'Prefixed names' : 'Full IRIs' });
  if (await button.count()) {
    await button.first().click();
    // The re-render is synchronous in Svelte, but the layout that follows is not.
    await page.waitForTimeout(150);
  }
}

test('no term in the triple table is cut off, with prefixed names', async ({ page }) => {
  await openBrowseWithRows(page);
  await setFullIris(page, false);

  const clipped = await clippedTerms(page, TERM_SELECTOR);

  expect(
    clipped,
    `${clipped.length} clipped term(s) in the triple table:\n${describe(clipped)}`,
  ).toEqual([]);
});

test('no term in the triple table is cut off when whole IRIs are asked for', async ({ page }) => {
  await openBrowseWithRows(page);
  await setFullIris(page, true);

  const clipped = await clippedTerms(page, TERM_SELECTOR);

  expect(
    clipped,
    `${clipped.length} clipped term(s) with full IRIs on:\n${describe(clipped)}`,
  ).toEqual([]);

  // And the reason the table used to clip is not fixed by handing the overflow
  // to the document: the page must not scroll sideways at a desktop width.
  const overflow = await page.evaluate(() => {
    const el = document.documentElement;
    return el.scrollWidth - el.clientWidth;
  });
  expect(overflow, 'the page gained a horizontal scrollbar instead of wrapping').toBeLessThanOrEqual(1);
});

test('a long IRI wraps onto more lines rather than being trimmed', async ({ page }) => {
  await openBrowseWithRows(page);
  await setFullIris(page, true);

  // Wrapping is what makes the whole text fit: the cell holding the longest
  // term must be taller than a single line of it.
  const measured = await page.evaluate(() => {
    let widest: { height: number; lineHeight: number; len: number } | null = null;
    for (const el of Array.from(document.querySelectorAll('.triple-row .cell-body'))) {
      const e = el as HTMLElement;
      const len = (e.textContent ?? '').trim().length;
      if (!widest || len > widest.len) {
        const lh = parseFloat(getComputedStyle(e).lineHeight) || 16;
        widest = { height: e.getBoundingClientRect().height, lineHeight: lh, len };
      }
    }
    return widest;
  });

  expect(measured, 'no terms rendered').not.toBeNull();
  // The demo's longest IRIs are comfortably wider than a 280px column, so the
  // cell holding one has to be at least two lines tall.
  expect(
    measured!.height,
    `the longest term (${measured!.len} chars) sits on one ${measured!.lineHeight}px line, so it is being trimmed`,
  ).toBeGreaterThan(measured!.lineHeight * 1.4);
});
