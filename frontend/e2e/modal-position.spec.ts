import { test, expect, request as pwRequest, type APIRequestContext, type Page } from '@playwright/test';

// Every dialog in the app opened in the middle of the *page* rather than the
// middle of the viewport.
//
// `.route-view` carries `animation: routeIn … both`, and `both` includes
// `forwards`: once the entrance finishes, the final keyframe keeps applying.
// Its final frame says `transform: none`, but a filled animation still sets the
// property, so the computed value is `matrix(1, 0, 0, 1, 0, 0)` rather than the
// initial `none` — and any transform other than `none` makes an element the
// containing block for its `position: fixed` descendants.
//
// So `.modal-backdrop`, which is `position: fixed; inset: 0` and expects to be
// the viewport, was instead the size of the routed page: measured 5052px tall
// against a 900px viewport on a dataset page. `align-items: center` then put
// the dialog 2500px down, out of sight, and the scroll position it appeared at
// bore no relation to where the user was.
//
// The fix is `backwards` instead of `both`: the entrance still starts from its
// first frame with no flash, and afterwards the element goes back to its own
// styles, where `transform` is `none`.
//
// This cannot be caught in jsdom, which has neither layout nor animations.

const BACKEND = process.env.OTS_BACKEND_URL ?? 'http://localhost:7878';
const ADMIN = { username: 'e2e-admin', password: 'e2e-password-123' };

let api: APIRequestContext;
let token = '';
let ownDatasetId = '';

test.beforeAll(async () => {
  api = await pwRequest.newContext({ baseURL: BACKEND });
  const res = await api.post('/api/auth/login', { data: ADMIN });
  expect(res.ok(), `login failed: ${res.status()}`).toBeTruthy();
  token = (await res.json()).access_token;

  // The dialog under test is only offered to someone who may edit the dataset,
  // so the test owns one rather than assuming anything about the seeded set.
  const headers = { authorization: `Bearer ${token}` };
  const me = await (await api.get('/api/auth/me', { headers })).json();
  const created = await api.post('/api/datasets', {
    headers,
    data: {
      name: 'e2e modal position',
      owner_type: 'user',
      owner_id: String(me.id),
      visibility: 'private',
    },
  });
  expect(
    created.ok(),
    `dataset create failed: ${created.status()} ${await created.text()}`,
  ).toBeTruthy();
  ownDatasetId = (await created.json()).id;
});

test.afterAll(async () => {
  if (ownDatasetId) {
    await api.delete(`/api/datasets/${ownDatasetId}`, {
      headers: { authorization: `Bearer ${token}` },
    });
  }
  await api.dispose();
});

async function signIn(page: Page): Promise<void> {
  await page.goto('/login');
  await page.getByLabel('Username').fill(ADMIN.username);
  await page.getByLabel('Password').fill(ADMIN.password);
  await page.getByRole('button', { name: 'Login' }).click();
  await expect(page).toHaveURL(/\/$/);
}

test('a routed page does not become the containing block for fixed overlays', async ({ page }) => {
  await page.goto('/browse');
  await expect(page.locator('.route-view')).toBeVisible();
  // Let the entrance animation finish; the bug is in what it leaves behind.
  await page.waitForTimeout(600);

  const measured = await page.evaluate(() => {
    const el = document.querySelector('.route-view') as HTMLElement;
    return {
      transform: getComputedStyle(el).transform,
      finished: el.getAnimations().every((a) => a.playState === 'finished'),
    };
  });

  expect(measured.finished, 'the entrance animation should have settled').toBe(true);
  expect(
    measured.transform,
    'a transform other than "none" makes .route-view the containing block for every fixed modal',
  ).toBe('none');
});

test('the dataset page-settings dialog opens in the viewport, not down the page', async ({ page }) => {
  await signIn(page);
  await page.goto(`/datasets/${ownDatasetId}`);

  const edit = page.getByRole('button', { name: 'Edit page' });
  await expect(edit).toBeVisible({ timeout: 20_000 });
  await edit.click();

  const backdrop = page.locator('.modal-backdrop');
  await expect(backdrop).toBeVisible();

  const measured = await page.evaluate(() => {
    const back = document.querySelector('.modal-backdrop') as HTMLElement;
    const box = document.querySelector('.modal-box') as HTMLElement;
    const b = back.getBoundingClientRect();
    const d = box.getBoundingClientRect();
    return {
      backdropHeight: b.height,
      backdropTop: b.top,
      dialogTop: d.top,
      dialogBottom: d.bottom,
      viewport: window.innerHeight,
    };
  });

  // A `position: fixed; inset: 0` backdrop is the viewport. It was the page.
  expect(
    Math.abs(measured.backdropHeight - measured.viewport),
    `the backdrop is ${Math.round(measured.backdropHeight)}px tall in a ${measured.viewport}px viewport, so it is being contained by the routed page`,
  ).toBeLessThanOrEqual(2);
  expect(Math.abs(measured.backdropTop)).toBeLessThanOrEqual(2);

  // And the dialog it centres is actually on screen.
  expect(measured.dialogTop).toBeGreaterThanOrEqual(-1);
  expect(measured.dialogBottom).toBeLessThanOrEqual(measured.viewport + 1);
});
