import { describe, it, expect, afterEach, vi } from 'vitest';

// Precedence under test (highest first): VITE_<SERVICE>_URL > runtime config
// (/config.json, via setRuntimeServiceOverrides) > /registry discovery > the
// localhost DEFAULTS. VITE_OVERRIDES is computed once at module load from
// import.meta.env, so the "VITE_*_URL wins" case needs a fresh module
// instance per stubbed env — the other cases just exercise the live module.

describe('serviceRegistry precedence', () => {
  afterEach(() => {
    vi.unstubAllEnvs();
    vi.resetModules();
  });

  it('falls back to the localhost default when nothing overrides it', async () => {
    const { getService } = await import('../serviceRegistry.js');
    expect(getService('lm-studio')).toBe('http://localhost:1234');
  });

  it('a runtime-config override wins over the default', async () => {
    const { getService, setRuntimeServiceOverrides } = await import('../serviceRegistry.js');
    setRuntimeServiceOverrides({ triplestore: 'https://runtime.example.com' });
    expect(getService('triplestore')).toBe('https://runtime.example.com');
  });

  it('getServiceMap reflects every name at its resolved precedence', async () => {
    const { getService, getServiceMap, setRuntimeServiceOverrides } = await import(
      '../serviceRegistry.js'
    );
    setRuntimeServiceOverrides({ 'form-app': 'https://forms.example.com' });
    const map = getServiceMap();
    expect(map['form-app']).toBe('https://forms.example.com');
    expect(map['triplestore']).toBe(getService('triplestore'));
  });

  it('a VITE_<SERVICE>_URL build-time env var wins over a runtime-config override', async () => {
    vi.stubEnv('VITE_TRIPLESTORE_URL', 'https://build-time.example.com/');
    vi.resetModules();
    const { getService, setRuntimeServiceOverrides } = await import('../serviceRegistry.js');
    setRuntimeServiceOverrides({ triplestore: 'https://runtime.example.com' });
    // Trailing slash is stripped so callers can concatenate paths safely.
    expect(getService('triplestore')).toBe('https://build-time.example.com');
  });

  it('setRuntimeServiceOverrides broadcasts a service-change event', async () => {
    const { SERVICE_CHANGE_EVENT, setRuntimeServiceOverrides } = await import(
      '../serviceRegistry.js'
    );
    const handler = vi.fn();
    window.addEventListener(SERVICE_CHANGE_EVENT, handler as EventListener);
    setRuntimeServiceOverrides({ ollama: 'https://ollama.example.com' });
    expect(handler).toHaveBeenCalledTimes(1);
    window.removeEventListener(SERVICE_CHANGE_EVENT, handler as EventListener);
  });
});
