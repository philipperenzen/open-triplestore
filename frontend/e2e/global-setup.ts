import type { FullConfig } from '@playwright/test';

// The demo seed only runs once an admin exists. On a fresh backend the first
// registered user becomes super_admin, which triggers the (idempotent) seed.
// Register that admin, then wait for the public org to appear.
const BACKEND = process.env.OTS_BACKEND_URL ?? 'http://localhost:7878';

async function json(res: Response): Promise<any> {
  return res.json().catch(() => null);
}

export default async function globalSetup(_config: FullConfig) {
  const reg = await fetch(`${BACKEND}/api/auth/register`, {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({
      username: 'e2e-admin',
      email: 'e2e@opentriplestore.test',
      password: 'e2e-password-123',
    }),
  }).catch(() => null);

  // 409 means the admin already exists (re-used data dir) — fine.
  if (reg && !reg.ok && reg.status !== 409) {
    throw new Error(`e2e admin registration failed: ${reg.status}`);
  }

  const deadline = Date.now() + 30_000;
  while (Date.now() < deadline) {
    const res = await fetch(`${BACKEND}/api/organisations`).catch(() => null);
    if (res && res.ok) {
      const body = await json(res);
      const list = Array.isArray(body) ? body : (body?.organisations ?? []);
      if (list.some((o: any) => o?.slug === 'open-triplestore' || o?.name === 'Open Triplestore')) {
        return;
      }
    }
    await new Promise((r) => setTimeout(r, 500));
  }
  throw new Error('demo "Open Triplestore" organisation was not seeded within 30s');
}
