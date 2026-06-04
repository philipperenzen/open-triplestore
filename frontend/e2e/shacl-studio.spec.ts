import { test, expect, type Page } from '@playwright/test';

// Smoke coverage for the consolidated SHACL Studio (/shacl/*). The stack
// (backend + frontend) and the seeded `e2e-admin` super-admin are provided by
// playwright.config.ts + global-setup.ts.
//
// Two facts shape how these tests authenticate and navigate:
//   1. Every Studio page is auth-gated — onMount does
//      `if (!$isAuthenticated) navigate('/login')`.
//   2. The app keeps its access token in-memory (api.ts: "avoids localStorage
//      XSS exposure"); only the refresh token is a cookie.
// A full page.goto('/shacl') after sign-in would drop the in-memory token and
// race the silent boot-refresh against the auth guard. So we sign in via the
// form once, then move around the SPA *client-side* (sidebar + Studio tabs),
// which keeps the token alive and `$isAuthenticated` true throughout.

const ADMIN = { username: 'e2e-admin', password: 'e2e-password-123' };

// The sub-nav present on every /shacl/* page — a stable, i18n-independent anchor.
const studioNav = (page: Page) => page.getByRole('navigation', { name: 'SHACL Studio sections' });

async function signIn(page: Page): Promise<void> {
  await page.goto('/login');
  await page.getByLabel('Username').fill(ADMIN.username);
  await page.getByLabel('Password').fill(ADMIN.password);
  await page.getByRole('button', { name: 'Login' }).click();
  // On success the app replaces the URL with the home route.
  await expect(page).toHaveURL(/\/$/);
}

// Enter the Studio through the sidebar so navigation stays client-side.
async function openStudio(page: Page): Promise<void> {
  await page.getByRole('link', { name: 'Validate', exact: true }).click();
  await expect(page).toHaveURL(/\/shacl$/);
  await expect(studioNav(page)).toBeVisible();
}

test.describe('SHACL Studio', () => {
  test.beforeEach(async ({ page }) => {
    await signIn(page);
    await openStudio(page);
  });

  test('overview surfaces the four sections and quick-start guidance', async ({ page }) => {
    const nav = studioNav(page);
    for (const tab of ['Overview', 'Shapes', 'Pipelines', 'Results']) {
      await expect(nav.getByRole('link', { name: tab, exact: true })).toBeVisible();
    }
    await expect(page.getByRole('heading', { name: 'Quick start' })).toBeVisible();
  });

  test('navigates Shapes → Pipelines → Results client-side', async ({ page }) => {
    const nav = studioNav(page);

    await nav.getByRole('link', { name: 'Shapes', exact: true }).click();
    await expect(page).toHaveURL(/\/shacl\/shapes$/);
    await expect(page.getByRole('button', { name: 'New shape graph' }).first()).toBeVisible();

    await nav.getByRole('link', { name: 'Pipelines', exact: true }).click();
    await expect(page).toHaveURL(/\/shacl\/pipelines$/);
    await expect(page.getByRole('heading', { name: 'Validation pipelines' })).toBeVisible();

    await nav.getByRole('link', { name: 'Results', exact: true }).click();
    await expect(page).toHaveURL(/\/shacl\/results$/);
    await expect(page.getByRole('heading', { name: 'Recent runs' })).toBeVisible();
  });

  test('creates a shape set and lands in its editor', async ({ page }) => {
    await studioNav(page).getByRole('link', { name: 'Shapes', exact: true }).click();
    await expect(page).toHaveURL(/\/shacl\/shapes$/);

    await page.getByRole('button', { name: 'New shape graph' }).first().click();

    const dialog = page.getByRole('dialog');
    await expect(dialog.getByRole('heading', { name: 'New shape graph' })).toBeVisible();
    await dialog.getByPlaceholder('e.g. Book shapes').fill(`E2E shapes ${Date.now()}`);
    await dialog.getByRole('button', { name: 'Create' }).click();

    // submitCreate() persists the set then routes to /shacl/shapes/:id.
    await expect(page).toHaveURL(/\/shacl\/shapes\/[^/]+$/);
  });

  test('opens the new-pipeline editor', async ({ page }) => {
    await studioNav(page).getByRole('link', { name: 'Pipelines', exact: true }).click();
    await expect(page).toHaveURL(/\/shacl\/pipelines$/);

    await page.getByRole('link', { name: 'New pipeline' }).click();
    await expect(page).toHaveURL(/\/shacl\/pipelines\/new$/);
    await expect(page.getByRole('heading', { name: 'What to validate' })).toBeVisible();
  });
});
