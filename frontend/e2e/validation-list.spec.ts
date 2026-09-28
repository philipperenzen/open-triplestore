import { test, expect, request as pwRequest, type APIRequestContext, type Page } from '@playwright/test';

// The validation page's dataset list, as a person actually sees it.
//
// `.ds-name` is a flex item with `overflow: hidden`, which by the flexbox spec
// replaces its automatic minimum size with 0 — so the flex algorithm was free to
// shrink it away entirely, and did: measured 0.0px wide against a scrollWidth of
// 138px, on every row, in a panel that also carries a status pill and a "+ Link
// shapes…" picker. The names were in the DOM and invisible on screen.
//
// jsdom has no layout, so a component test cannot see this class of bug at all.
// This spec asserts the thing that was wrong: the name occupies real width and
// the text belongs to the dataset the API knows about.

const BACKEND = process.env.OTS_BACKEND_URL ?? 'http://localhost:7878';
const ADMIN = { username: 'e2e-admin', password: 'e2e-password-123' };

let api: APIRequestContext;
let token = '';

async function signIn(page: Page): Promise<void> {
  await page.goto('/login');
  await page.getByLabel('Username').fill(ADMIN.username);
  await page.getByLabel('Password').fill(ADMIN.password);
  await page.getByRole('button', { name: 'Login' }).click();
  await expect(page).toHaveURL(/\/$/);
}

test.beforeAll(async () => {
  api = await pwRequest.newContext({ baseURL: BACKEND });
  const res = await api.post('/api/auth/login', { data: ADMIN });
  expect(res.ok(), `login failed: ${res.status()}`).toBeTruthy();
  token = (await res.json()).access_token;
});

test.afterAll(async () => {
  await api.dispose();
});

test('every dataset name in the validation list is visible, not collapsed to zero width', async ({ page }) => {
  const body = await (await api.get('/api/datasets', { headers: { authorization: `Bearer ${token}` } })).json();
  const datasets: { name: string }[] = Array.isArray(body) ? body : (body.datasets ?? []);
  test.skip(datasets.length === 0, 'no datasets seeded on this backend');

  await signIn(page);
  await page.goto('/validation');

  const names = page.locator('.dataset-row .ds-name');
  await expect(names.first()).toBeVisible();
  expect(await names.count()).toBe(datasets.length);

  // Every row: real width, and the full name legible or honestly ellipsised —
  // never the 0px that hid all of them.
  const rows = await names.evaluateAll((els) =>
    els.map((el) => ({
      text: (el.textContent ?? '').trim(),
      width: el.getBoundingClientRect().width,
      scrollWidth: el.scrollWidth,
    })),
  );
  const known = new Set(datasets.map((d) => d.name));
  for (const row of rows) {
    expect(row.width, `"${row.text}" is ${row.width}px wide`).toBeGreaterThan(40);
    expect(known.has(row.text), `"${row.text}" is not a dataset the API lists`).toBeTruthy();
  }

  // And the panel itself does not scroll sideways to achieve it.
  const overflow = await page.locator('.ds-list').evaluate((el) => el.scrollWidth - el.clientWidth);
  expect(overflow).toBeLessThanOrEqual(1);
});

test('in the tightest layout the name keeps its floor and is not cut short', async ({ page }) => {
  // The panel is `minmax(280px, 320px)` until the 960px breakpoint collapses the
  // split entirely — so the *tightest* the panel ever gets is just above that
  // breakpoint, at its 280px floor. Going below 960 would make it full page
  // width, which is the one layout where the bug could not appear.
  await page.setViewportSize({ width: 1000, height: 900 });
  await signIn(page);
  await page.goto('/validation');

  const rows = page.locator('.dataset-row');
  await expect(rows.first()).toBeVisible();
  const panelWidth = await page.locator('.ds-list').evaluate((el) => el.getBoundingClientRect().width);
  expect(panelWidth, 'the split should not have collapsed at this width').toBeLessThan(340);

  const measured = await rows.evaluateAll((els) =>
    els.map((row) => {
      const name = row.querySelector('.ds-name') as HTMLElement | null;
      const chip = row.querySelector('.owner-chip') as HTMLElement | null;
      return {
        text: (name?.textContent ?? '').trim(),
        nameWidth: name?.getBoundingClientRect().width ?? 0,
        nameNeeds: name?.scrollWidth ?? 0,
        nameHeight: name?.getBoundingClientRect().height ?? 0,
        lineHeight: name ? parseFloat(getComputedStyle(name).lineHeight) || 16 : 16,
        chipWidth: chip?.getBoundingClientRect().width ?? 0,
        chipNeeds: chip?.scrollWidth ?? 0,
        titleWidth: (row.querySelector('.row-title') as HTMLElement | null)?.getBoundingClientRect().width ?? 0,
      };
    }),
  );

  // `min-width: 4.5rem` is the floor the fix gives the name: it is never
  // narrower than that. Before the fix every one of these was 0.
  const FLOOR = 72;
  for (const row of measured) {
    expect(row.nameWidth, `"${row.text}" is ${row.nameWidth}px`).toBeGreaterThanOrEqual(FLOOR - 1);
  }

  // Nothing is trimmed: a name too long for one line at this width takes two,
  // and the chip does the same, so neither reports needing more width than it
  // was given. (Until the truncation policy landed, both cut their text with
  // an ellipsis instead, which this could not have distinguished from a fit.)
  for (const row of measured) {
    expect(
      row.nameNeeds,
      `"${row.text}" needs ${row.nameNeeds}px but was given ${row.nameWidth}px, so it is being cut`,
    ).toBeLessThanOrEqual(row.nameWidth + 1);
    expect(row.chipWidth, `the owner chip of "${row.text}" is cut`).toBeGreaterThanOrEqual(
      row.chipNeeds - 1,
    );
  }

  // A name that does not fit on one line proves the wrapping is what makes it
  // fit, rather than the panel having grown or the text having been shortened.
  const longest = measured.reduce((a, b) => (b.text.length > a.text.length ? b : a));
  expect(
    longest.nameHeight,
    `the longest name "${longest.text}" sits on one line in a ${panelWidth}px panel`,
  ).toBeGreaterThan(longest.lineHeight * 1.4);
});
