import { test, expect, type Page } from '@playwright/test';

// End-to-end coverage of the data-import wizard: sign in, upload a Turtle file,
// create a new personal dataset, run the import, and confirm the success state.
// The stack + the seeded `e2e-admin` super-admin come from playwright.config.ts +
// global-setup.ts.
//
// Like the SHACL Studio spec, the app keeps its access token in-memory (api.ts),
// so after signing in we reach /import via the sidebar (client-side) rather than
// page.goto — a full navigation would drop the token and show the sign-in card.

const ADMIN = { username: 'e2e-admin', password: 'e2e-password-123' };

async function signIn(page: Page): Promise<void> {
  await page.goto('/login');
  await page.getByLabel('Username').fill(ADMIN.username);
  await page.getByLabel('Password').fill(ADMIN.password);
  await page.getByRole('button', { name: 'Login' }).click();
  await expect(page).toHaveURL(/\/$/);
}

test('imports a Turtle file into a new dataset and reports success', async ({ page }) => {
  await signIn(page);

  // Client-side nav into the wizard keeps the in-memory token alive.
  await page.getByRole('link', { name: 'Import data' }).click();
  await expect(page).toHaveURL(/\/import$/);

  // Step 1 — upload a small Turtle file through the hidden file input. The wizard
  // auto-derives a target graph for triple formats, which enables "Next".
  await page
    .locator('input[type="file"][accept]')
    .first()
    .setInputFiles({
      name: 'e2e-import.ttl',
      mimeType: 'text/turtle',
      buffer: Buffer.from(
        '<http://example.org/e2e/thing> <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <http://example.org/e2e/Widget> .\n' +
          '<http://example.org/e2e/thing> <http://www.w3.org/2000/01/rdf-schema#label> "E2E Widget" .\n',
      ),
    });
  const next1 = page.getByRole('button', { name: 'Next' });
  await expect(next1).toBeEnabled();
  await next1.click();

  // Step 2 — personal account + a uniquely named new dataset (avoids name clashes
  // across retries / re-used data dirs).
  await page.getByRole('button', { name: /Personal account/i }).click();
  await page.getByRole('button', { name: /Create new dataset/i }).click();
  await page.getByLabel('Dataset Name').fill(`E2E import ${Date.now()}`);
  await page.getByRole('button', { name: 'Next' }).click();

  // Step 3 — run the import and confirm the success state.
  const importBtn = page.getByRole('button', { name: 'Import Now' });
  await expect(importBtn).toBeEnabled();
  await importBtn.click();
  await expect(page.getByText('Import completed successfully!')).toBeVisible({ timeout: 20_000 });
});
