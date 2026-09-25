import { describe, it, expect } from 'vitest';
import { llmServiceHealthView, LLM_SERVICES } from '../llmServiceHealth.js';
import type { LlmHealth, LlmServiceStatus } from '../api.js';
import en from '../i18n/en.json';
import nl from '../i18n/nl.json';

const SERVICES: LlmServiceStatus[] = [
  { id: 'chat', model: 'llama3.1:8b', listed: true },
  { id: 'sparql', model: 'qwen2.5-coder', listed: false },
  { id: 'shacl', model: 'mistral', listed: null },
];

function health(over: Partial<LlmHealth> = {}): LlmHealth {
  return {
    gateway: 'http://llm.internal:8000',
    reachable: true,
    configured: true,
    chat_model: 'llama3.1:8b',
    services: SERVICES,
    ...over,
  };
}

describe('llmServiceHealthView — not fetched yet', () => {
  it('shows a pending gateway and no service rows for null and undefined', () => {
    for (const status of [null, undefined]) {
      expect(llmServiceHealthView(status)).toEqual({
        gateway: { state: 'pending', detailKey: 'nav.llmChecking', address: null },
        services: [],
      });
    }
  });
});

describe('llmServiceHealthView — reachable gateway', () => {
  it('marks the gateway ok and keeps its address for the tooltip', () => {
    expect(llmServiceHealthView(health()).gateway).toEqual({
      state: 'ok',
      detailKey: 'nav.llmReachable',
      address: 'http://llm.internal:8000',
    });
  });

  it('gives one row per service: listed true/null is ok, listed false warns', () => {
    expect(llmServiceHealthView(health()).services).toEqual([
      { id: 'chat', labelKey: 'nav.llmServiceChat', model: 'llama3.1:8b', state: 'ok', noteKey: null },
      { id: 'sparql', labelKey: 'nav.llmServiceSparql', model: 'qwen2.5-coder', state: 'warn', noteKey: 'nav.llmModelNotListed' },
      // null = the gateway answered without a model list (its /health fallback): not a problem.
      { id: 'shacl', labelKey: 'nav.llmServiceShacl', model: 'mistral', state: 'ok', noteKey: null },
    ]);
  });

  it('treats a missing listed flag like null (ok)', () => {
    const rows = llmServiceHealthView(
      health({ services: [{ id: 'chat', model: 'm' } as LlmServiceStatus] }),
    ).services;
    expect(rows).toEqual([
      { id: 'chat', labelKey: 'nav.llmServiceChat', model: 'm', state: 'ok', noteKey: null },
    ]);
  });

  it('is reachable regardless of configured (the built-in default can answer)', () => {
    const view = llmServiceHealthView(health({ configured: false }));
    expect(view.gateway.state).toBe('ok');
    expect(view.services.map((s) => s.state)).toEqual(['ok', 'warn', 'ok']);
  });
});

describe('llmServiceHealthView — the request itself failed', () => {
  it('claims nothing: an unknown, neutral gateway row and no service rows', () => {
    // llmHealth()'s fallback for a network error, a proxy 5xx or a non-JSON
    // body — not "not configured", which is a fact only the server can report.
    expect(llmServiceHealthView({ reachable: false, error: true })).toEqual({
      gateway: { state: 'pending', detailKey: 'nav.llmUnknown', address: null },
      services: [],
    });
  });
});

describe('llmServiceHealthView — unreachable gateway', () => {
  it('warns, with warning rows, when the gateway is configured (the LLM is optional, never red)', () => {
    const view = llmServiceHealthView(
      health({
        reachable: false,
        services: SERVICES.map((s) => ({ ...s, listed: null })),
      }),
    );
    expect(view.gateway).toEqual({
      state: 'warn',
      detailKey: 'nav.llmUnreachable',
      address: 'http://llm.internal:8000',
    });
    expect(view.services).toEqual([
      { id: 'chat', labelKey: 'nav.llmServiceChat', model: 'llama3.1:8b', state: 'warn', noteKey: null },
      { id: 'sparql', labelKey: 'nav.llmServiceSparql', model: 'qwen2.5-coder', state: 'warn', noteKey: null },
      { id: 'shacl', labelKey: 'nav.llmServiceShacl', model: 'mistral', state: 'warn', noteKey: null },
    ]);
  });

  it('is "not configured" with no rows when configured is false', () => {
    expect(
      llmServiceHealthView(health({ reachable: false, configured: false, gateway: 'http://127.0.0.1:8000' })),
    ).toEqual({
      gateway: { state: 'warn', detailKey: 'nav.llmNotConfigured', address: 'http://127.0.0.1:8000' },
      services: [],
    });
  });

  it('is "not configured" when an older server omits configured', () => {
    const { configured: _omit, ...older } = health({ reachable: false });
    const view = llmServiceHealthView(older);
    expect(view.gateway.state).toBe('warn');
    expect(view.gateway.detailKey).toBe('nav.llmNotConfigured');
    expect(view.services).toEqual([]);
  });

  it('treats a bare { reachable: false } (no configured field) as not configured', () => {
    expect(llmServiceHealthView({ reachable: false })).toEqual({
      gateway: { state: 'warn', detailKey: 'nav.llmNotConfigured', address: null },
      services: [],
    });
  });

  it('only accepts configured === true, not a truthy stand-in', () => {
    const view = llmServiceHealthView(
      health({ reachable: false, configured: 'yes' as unknown as boolean }),
    );
    expect(view.gateway.state).toBe('warn');
    expect(view.services).toEqual([]);
  });
});

describe('llmServiceHealthView — malformed or partial services', () => {
  it('has no service rows when services is missing (older server)', () => {
    const { services: _omit, ...older } = health();
    const view = llmServiceHealthView(older);
    expect(view.gateway.state).toBe('ok');
    expect(view.services).toEqual([]);
  });

  it('has no service rows when services is not an array', () => {
    for (const services of [null, {}, 'chat', 3]) {
      const view = llmServiceHealthView(
        health({ services: services as unknown as LlmServiceStatus[] }),
      );
      expect(view.gateway.state).toBe('ok');
      expect(view.services).toEqual([]);
    }
    const down = llmServiceHealthView(
      health({ reachable: false, services: null as unknown as LlmServiceStatus[] }),
    );
    expect(down.gateway.state).toBe('warn');
    expect(down.services).toEqual([]);
  });

  it('ignores unknown ids and junk entries, and keeps the order chat, sparql, shacl', () => {
    const view = llmServiceHealthView(
      health({
        services: [
          { id: 'embeddings', model: 'nomic', listed: true },
          { id: 'shacl', model: 'c', listed: true },
          null as unknown as LlmServiceStatus,
          'sparql' as unknown as LlmServiceStatus,
          { id: 'chat', model: 'a', listed: false },
        ],
      }),
    );
    expect(view.services.map((s) => [s.id, s.model, s.state])).toEqual([
      ['chat', 'a', 'warn'],
      ['shacl', 'c', 'ok'],
    ]);
  });

  it('uses the first entry when an id repeats', () => {
    const view = llmServiceHealthView(
      health({
        services: [
          { id: 'chat', model: 'first', listed: true },
          { id: 'chat', model: 'second', listed: false },
        ],
      }),
    );
    expect(view.services).toEqual([
      { id: 'chat', labelKey: 'nav.llmServiceChat', model: 'first', state: 'ok', noteKey: null },
    ]);
  });

  it('shows an empty model name rather than a non-string one', () => {
    const view = llmServiceHealthView(
      health({ services: [{ id: 'chat', model: 42 as unknown as string, listed: true }] }),
    );
    expect(view.services[0].model).toBe('');
  });

  it('drops a non-string or empty gateway address', () => {
    expect(llmServiceHealthView(health({ gateway: '' })).gateway.address).toBeNull();
    expect(
      llmServiceHealthView(health({ gateway: 7 as unknown as string })).gateway.address,
    ).toBeNull();
  });
});

describe('llmServiceHealthView — translation keys', () => {
  // The popover passes these keys to $t() as variables, which the literal-key
  // scan in i18nKeysResolve.test.ts cannot see; check them here instead.
  function lookup(dict: unknown, key: string): unknown {
    return key.split('.').reduce<unknown>(
      (node, part) => (node && typeof node === 'object' ? (node as Record<string, unknown>)[part] : undefined),
      dict,
    );
  }

  it('emits only keys that exist in both dictionaries', () => {
    const views = [
      llmServiceHealthView(null),
      llmServiceHealthView(health()),
      llmServiceHealthView(health({ reachable: false })),
      llmServiceHealthView({ reachable: false }),
      llmServiceHealthView({ reachable: false, error: true }),
    ];
    const keys = new Set<string>(['nav.llmServices', 'nav.llmGateway']);
    for (const view of views) {
      keys.add(view.gateway.detailKey);
      for (const row of view.services) {
        keys.add(row.labelKey);
        if (row.noteKey) keys.add(row.noteKey);
      }
    }
    for (const { labelKey } of LLM_SERVICES) keys.add(labelKey);
    // All five gateway words, three feature names, the heading, the gateway
    // label and the not-listed note.
    expect(keys.size).toBe(11);
    for (const key of keys) {
      expect(typeof lookup(en, key), `en: ${key}`).toBe('string');
      expect(typeof lookup(nl, key), `nl: ${key}`).toBe('string');
    }
  });
});
