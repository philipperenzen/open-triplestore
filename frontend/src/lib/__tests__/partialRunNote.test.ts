// A run that left out graphs its caller may not read answers `partial: true`;
// PartialRunNote is what says so on screen, for validation (not recorded) and
// for inference (some rules did not run).
import { describe, it, expect, beforeAll } from 'vitest';
import { render } from '@testing-library/svelte';
import { init, addMessages } from 'svelte-i18n';
import en from '../i18n/en.json';
import PartialRunNote from '../../components/PartialRunNote.svelte';

beforeAll(() => {
  addMessages('en', en as unknown as Parameters<typeof addMessages>[1]);
  init({ fallbackLocale: 'en', initialLocale: 'en' });
});

describe('PartialRunNote', () => {
  it('says a validation run was not recorded by default', () => {
    const { container } = render(PartialRunNote);
    const note = container.querySelector('[role="note"]');
    expect(note?.textContent).toContain(en.components.partialRunNote.validate);
  });

  it('says which rules did not run for inference', () => {
    const { container } = render(PartialRunNote, { kind: 'infer' });
    const note = container.querySelector('[role="note"]');
    expect(note?.textContent).toContain(en.components.partialRunNote.infer);
    expect(note?.textContent).not.toContain(en.components.partialRunNote.validate);
  });
});
