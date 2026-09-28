// @vitest-environment node
// Unit tests for the build's third-party licence notice generator.
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { afterAll, describe, expect, it } from 'vitest';
import {
  OTHER_SECTION,
  PackageRegistry,
  checkDist,
  leadingNoticeComments,
  licenceIds,
  lockClosure,
  otherBundledMaterial,
  packageDirOf,
  renderNotices,
  resolveOtherMaterial,
} from './third-party-licenses.mjs';

// The repository's LICENSES/ directory, as vite.config.js passes it.
const LICENSES = fileURLToPath(new URL('../../LICENSES', import.meta.url));

const tmp = fs.mkdtempSync(path.join(os.tmpdir(), 'ots-licences-'));
afterAll(() => fs.rmSync(tmp, { recursive: true, force: true }));

function writePkg(rel, pkg, files = {}) {
  const dir = path.join(tmp, rel);
  fs.mkdirSync(dir, { recursive: true });
  fs.writeFileSync(path.join(dir, 'package.json'), JSON.stringify(pkg));
  for (const [name, text] of Object.entries(files)) {
    fs.mkdirSync(path.dirname(path.join(dir, name)), { recursive: true });
    fs.writeFileSync(path.join(dir, name), text);
  }
  return dir;
}

describe('packageDirOf', () => {
  it('maps module ids to their package directory', () => {
    expect(packageDirOf('/a/node_modules/leaflet/dist/leaflet-src.js')).toBe('/a/node_modules/leaflet');
    expect(packageDirOf('/a/node_modules/@cesium/engine/Source/Core/Cartesian3.js')).toBe(
      '/a/node_modules/@cesium/engine',
    );
    expect(packageDirOf('/a/node_modules/x/node_modules/@s/y/lib/z.js')).toBe('/a/node_modules/x/node_modules/@s/y');
    expect(packageDirOf('\0/a/node_modules/y/index.js?commonjs-proxy')).toBe('/a/node_modules/y');
  });

  it('ignores application code, caches and bare scopes', () => {
    expect(packageDirOf('/a/src/main.ts')).toBeNull();
    expect(packageDirOf('/a/node_modules/.vite/deps/x.js')).toBeNull();
    expect(packageDirOf('/a/node_modules/@scope')).toBeNull();
  });
});

describe('leadingNoticeComments', () => {
  it('keeps the opening comments that carry a notice, and stops at code', () => {
    const src = [
      '#!/usr/bin/env node',
      '/* jshint forin: false */',
      '/*',
      ' * Copyright 2015 Someone',
      ' */',
      '// Licensed under MIT',
      '// see LICENSE',
      "'use strict';",
      '/* Copyright later, not a header */',
    ].join('\n');
    expect(leadingNoticeComments(src)).toEqual([
      '/*\n * Copyright 2015 Someone\n */',
      '// Licensed under MIT\n// see LICENSE',
    ]);
  });
});

describe('licenceIds', () => {
  it('splits SPDX expressions', () => {
    expect(licenceIds('(MIT OR Apache-2.0)')).toEqual(['MIT', 'Apache-2.0']);
    expect(licenceIds('Apache-2.0 WITH LLVM-exception')).toEqual(['Apache-2.0']);
    expect(licenceIds(null)).toEqual([]);
  });
});

describe('lockClosure', () => {
  it('follows dependencies and optional dependencies with Node lookup, not peers', () => {
    const packages = {
      '': { dependencies: { a: '1' } },
      'node_modules/a': { dependencies: { b: '1' }, optionalDependencies: { c: '1' }, peerDependencies: { p: '1' } },
      'node_modules/a/node_modules/b': { dependencies: { d: '1' } },
      'node_modules/b': {},
      'node_modules/c': {},
      'node_modules/d': {},
      'node_modules/p': {},
    };
    expect(lockClosure(tmp, 'a', packages)).toEqual([
      'node_modules/a',
      'node_modules/a/node_modules/b',
      'node_modules/c',
      'node_modules/d',
    ]);
  });
});

describe('renderNotices', () => {
  const mit = 'MIT License\n\nCopyright (c) 2020 A\n\nPermission is hereby granted, free of charge, ...';
  const root = path.join(tmp, 'app');
  const a = writePkg('app/node_modules/a', { name: 'a', version: '1.0.0', license: 'MIT' }, { LICENSE: mit });
  const b = writePkg('app/node_modules/b', { name: 'b', version: '2.0.0', license: 'MIT' }, { 'LICENSE.md': `${mit}\r\n` });
  const c = writePkg(
    'app/node_modules/c',
    { name: 'c', version: '3.0.0', license: 'Apache-2.0' },
    { LICENSE: 'Apache License text', NOTICE: 'c\nCopyright 2021 C Inc.', 'dist/bundle.js': 'x', 'dist/bundle.js.LICENSE.txt': '/*! inner lib, MIT */' },
  );
  const d = writePkg('app/node_modules/d', { name: 'd', version: '4.0.0', license: 'BSD-3-Clause', main: 'lib/d.js' }, {
    'lib/d.js': '/* Copyright (c) 1999 D */\nmodule.exports = 1;\n',
  });
  const e = writePkg('app/node_modules/e', { name: 'e', version: '5.0.0', license: 'ISC' }, { 'index.js': 'module.exports = 1;' });

  const registry = new PackageRegistry(root);
  registry.addModuleIds(
    [
      path.join(a, 'index.js'),
      path.join(b, 'index.js'),
      path.join(c, 'dist/bundle.js'),
      path.join(d, 'lib/d.js'),
      `\0${path.join(e, 'index.js')}?commonjs-proxy`,
      path.join(root, 'src/main.ts'),
    ],
    'bundled',
  );
  const { text, missing } = renderNotices({ packages: registry.packages(), cesium: null });

  it('lists every bundled package once, with its declared licence', () => {
    expect(text).toMatch(/^1\. Packages \(5\)$/m);
    for (const name of ['a 1.0.0', 'b 2.0.0', 'c 3.0.0', 'd 4.0.0', 'e 5.0.0']) {
      expect(text).toContain(`\n${name}\n`);
    }
    expect(text).toContain('  Licence: Apache-2.0');
  });

  it('prints identical texts once and keeps NOTICE files and bundle sidecars', () => {
    expect(text.split('Copyright (c) 2020 A').length - 1).toBe(1);
    expect(text).toMatch(/a@1\.0\.0 \(LICENSE\)\n {2}b@2\.0\.0 \(LICENSE\.md\)/);
    expect(text).toContain('c\nCopyright 2021 C Inc.');
    expect(text).toContain('c@3.0.0 (dist/bundle.js.LICENSE.txt)');
  });

  it('falls back to the source header, then reports packages with no notice at all', () => {
    expect(text).toContain('d@4.0.0 (comment at the top of lib/d.js)');
    expect(text).toContain('/* Copyright (c) 1999 D */');
    expect(missing).toEqual(['e@5.0.0']);
  });

  it('passes the post-build check', () => {
    const dist = path.join(tmp, 'dist');
    expect(checkDist(dist)[0]).toMatch(/is missing/);
    fs.mkdirSync(dist, { recursive: true });
    fs.writeFileSync(path.join(dist, 'THIRD-PARTY-LICENSES.txt'), text);
    expect(checkDist(dist)).toEqual([]);
  });
});

describe('material bundled outside npm', () => {
  const material = otherBundledMaterial(LICENSES);
  const squash = (t) => t.replace(/\s+/g, ' ');

  it('quotes every copyright line from the licence text it ships', () => {
    for (const item of material) {
      for (const c of item.components) {
        const texts = c.texts.map((rel) => squash(fs.readFileSync(path.join(LICENSES, rel), 'utf8')));
        for (const line of c.copyright) {
          expect(
            texts.some((t) => t.includes(squash(line))),
            `${item.id}: ${c.name}: "${line}" is not in ${c.texts.join(', ')}`,
          ).toBe(true);
        }
      }
    }
  });

  it("covers every licence text of web-ifc.wasm's linked libraries", () => {
    const wasm = material.find((m) => m.id === 'web-ifc.wasm');
    const shipped = wasm.components.flatMap((c) => c.texts.map((t) => path.basename(t)));
    const onDisk = fs.readdirSync(path.join(LICENSES, 'web-ifc')).sort();
    expect([...shipped].sort()).toEqual(onDisk);
  });

  it('keeps the EPSG attribution that crs.ts states', () => {
    const crs = fs.readFileSync(fileURLToPath(new URL('../src/lib/viewer/crs.ts', import.meta.url)), 'utf8');
    const epsg = material.find((m) => m.id === 'epsg');
    for (const phrase of ['EPSG Geodetic Parameter Dataset', 'https://epsg.org/terms-of-use.html', 'IOGP']) {
      expect(squash(crs.replace(/^\s*\/\/ ?/gm, ''))).toContain(phrase);
      expect(epsg.intro).toContain(phrase);
    }
  });

  const root = path.join(tmp, 'other-app');
  fs.mkdirSync(path.join(root, 'public', 'wasm'), { recursive: true });
  fs.writeFileSync(path.join(root, 'public', 'wasm', 'web-ifc.wasm'), 'wasm');
  const bundle = {
    'assets/GraphView-abc.js': {
      type: 'chunk',
      fileName: 'assets/GraphView-abc.js',
      moduleIds: [path.join(root, 'src/components/GraphCanvas.svelte')],
    },
    'assets/index.css': { type: 'asset', fileName: 'assets/index.css' },
  };

  it('includes an item only when its file or module is in the build', () => {
    const resolved = resolveOtherMaterial(material, { root, bundle });
    expect(resolved.map((r) => r.id)).toEqual(['web-ifc.wasm', 'icons']);
    expect(resolved[1].where).toEqual(['dist/assets/GraphView-abc.js']);
    const glm = resolved[0].components.find((c) => c.name === 'glm');
    expect(glm.texts[0].label).toBe('LICENSES/web-ifc/glm.txt');
    expect(glm.texts[0].text).toContain('Copyright (c) 2005 - G-Truc Creation');
  });

  it('fails when a licence text is missing', () => {
    const broken = otherBundledMaterial(path.join(tmp, 'no-such-licences-dir'));
    expect(() => resolveOtherMaterial(broken, { root, bundle })).toThrow(/cannot read .*web-ifc\.txt/);
  });

  it('renders the material with its texts, sharing identical ones with npm packages', () => {
    const pkgRoot = path.join(tmp, 'other-pkgs');
    const lucideLicence = fs.readFileSync(path.join(LICENSES, 'Lucide-ISC.txt'), 'utf8');
    const lucide = writePkg('other-pkgs/node_modules/lucide-x', { name: 'lucide-x', version: '1.0.0', license: 'ISC' }, {
      LICENSE: lucideLicence,
    });
    const registry = new PackageRegistry(pkgRoot);
    registry.addModuleIds([path.join(lucide, 'index.js')], 'bundled');
    const other = resolveOtherMaterial(material, { root, bundle });
    const { text } = renderNotices({ packages: registry.packages(), cesium: null, other });
    expect(text).toContain(`\n3. ${OTHER_SECTION}\n`);
    expect(text).toMatch(/^dist\/wasm\/web-ifc\.wasm$/m);
    expect(squash(text)).toContain(
      'https://github.com/ThatOpen/engine_web-ifc at commit f26c4beef0a668ebdb180d2b95a94097a1e21cef',
    );
    expect(text).toContain('In: dist/assets/GraphView-abc.js');
    expect(text).toContain('\n4. Licence and notice texts\n');
    // The Lucide text is printed once, for the package and the inline icons.
    expect(text.split('All other copyright (c) for Lucide are held by Lucide Contributors 2022.').length - 1).toBe(1);
    expect(text).toMatch(/lucide-x@1\.0\.0 \(LICENSE\)\n {2}icons: Lucide \(LICENSES\/Lucide-ISC\.txt\)/);
    // BSD's binary-form clause: the notice of predicates.h travels.
    expect(text).toContain('Copyright (c) 2019, William C. Lenthe');

    // The post-build check wants the wasm's section whenever dist ships it,
    // and the EPSG notice whenever a bundle carries proj4 definitions.
    const dist = path.join(tmp, 'other-dist');
    fs.mkdirSync(path.join(dist, 'wasm'), { recursive: true });
    fs.mkdirSync(path.join(dist, 'assets'), { recursive: true });
    fs.writeFileSync(path.join(dist, 'wasm', 'web-ifc.wasm'), 'wasm');
    fs.writeFileSync(path.join(dist, 'assets', 'crs-1.js'), 'const d="+proj=sterea +lat_0=52"');
    fs.writeFileSync(path.join(dist, 'THIRD-PARTY-LICENSES.txt'), text);
    expect(checkDist(dist)).toEqual([expect.stringMatching(/lacks the EPSG notice/)]);
    const withEpsg = renderNotices({
      packages: registry.packages(),
      cesium: null,
      other: resolveOtherMaterial(material, {
        root,
        bundle: {
          ...bundle,
          'assets/crs-1.js': { type: 'chunk', fileName: 'assets/crs-1.js', moduleIds: [path.join(root, 'src/lib/viewer/crs.ts')] },
        },
      }),
    }).text;
    fs.writeFileSync(path.join(dist, 'THIRD-PARTY-LICENSES.txt'), withEpsg);
    expect(checkDist(dist)).toEqual([]);
    const withoutOther = renderNotices({ packages: registry.packages(), cesium: null }).text;
    fs.writeFileSync(path.join(dist, 'THIRD-PARTY-LICENSES.txt'), withoutOther);
    expect(checkDist(dist)).toEqual([
      expect.stringMatching(/does not cover dist\/wasm\/web-ifc\.wasm/),
      expect.stringMatching(/lacks the EPSG notice/),
    ]);
  });
});
