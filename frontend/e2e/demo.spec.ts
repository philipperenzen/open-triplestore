import { test, expect } from '@playwright/test';

// Drives the public "Open Triplestore" demo through the browser. The stack
// (backend + frontend) and the seed are provided by playwright.config.ts +
// global-setup.ts. The demo is public, so no sign-in is needed.

test('the public organisation lists its category datasets', async ({ page }) => {
  await page.goto('/organisations');

  await expect(page.getByText('Open Triplestore').first()).toBeVisible();
  await page.getByText('Open Triplestore').first().click();

  // The org detail page lists its datasets, one per standards category.
  await expect(page.getByText('Core RDF & SPARQL')).toBeVisible();
  await expect(page.getByText('Reasoning & Ontologies')).toBeVisible();
  await expect(page.getByText('Spatial (GeoSPARQL)')).toBeVisible();
});

test('a GeoSPARQL saved query runs and returns cities', async ({ page }) => {
  await page.goto('/datasets');

  // Open the spatial demo dataset.
  await page.getByRole('link', { name: 'Spatial (GeoSPARQL)' }).click();
  await expect(page).toHaveURL(/\/datasets\/[^/]+$/);

  // Jump to its API services (saved queries).
  await page.getByRole('link', { name: /API Services/i }).click();
  await expect(page).toHaveURL(/\/api-services$/);

  // The seeded GeoSPARQL service is listed. Service cards are collapsed by
  // default, so expand it before the Run control is rendered, then run it.
  await expect(page.getByText('Cities within a bounding box')).toBeVisible();
  await page.getByText('Cities within a bounding box').click();
  await page.getByRole('button', { name: /^Run$/i }).first().click();
  await expect(page.getByText(/Amsterdam|Rotterdam|POINT/).first()).toBeVisible();
});

test('the capabilities dataset advertises every supported standard', async ({ page }) => {
  await page.goto('/datasets');
  await page.getByRole('link', { name: 'Platform Capabilities & Security' }).click();
  await page.getByRole('link', { name: /API Services/i }).click();

  await expect(page.getByText('Supported standards')).toBeVisible();
  // Expand the service card so the Run control is rendered, then run it.
  await page.getByText('Supported standards').click();
  await page.getByRole('button', { name: /^Run$/i }).first().click();
  await expect(page.getByText(/GeoSPARQL|SPARQL 1.1/).first()).toBeVisible();
});
