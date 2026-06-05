import { test, expect } from '@playwright/test';

// Read-only coverage of the SPARQL workspace, the triple browser, and the dataset
// list against the seeded demo. These pages are public (no auth redirect), so —
// like demo.spec.ts — they are driven anonymously via page.goto.

test('the SPARQL workspace runs the default query and shows a result summary', async ({ page }) => {
  await page.goto('/sparql');

  // The editor ships with a prefilled "SELECT ?s ?p ?o … LIMIT 25"; just run it.
  await page.getByRole('button', { name: 'Execute' }).click();

  // A finished run renders an elapsed-time / row-count summary next to the editor.
  await expect(page.locator('.elapsed')).toBeVisible({ timeout: 15_000 });
});

test('the triple browser renders the subject/predicate/object table', async ({ page }) => {
  await page.goto('/browse');

  // The triple table's column headers are stable, i18n-driven anchors.
  await expect(page.getByText('Subject').first()).toBeVisible();
  await expect(page.getByText('Predicate').first()).toBeVisible();
  await expect(page.getByText('Object').first()).toBeVisible();
});

test('the dataset list shows the seeded demo datasets', async ({ page }) => {
  await page.goto('/datasets');

  // Seeded by global-setup; the same dataset demo.spec.ts relies on.
  await expect(page.getByText('Spatial (GeoSPARQL)')).toBeVisible();
});
