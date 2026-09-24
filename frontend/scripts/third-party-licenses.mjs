// Third-party licence notices for the built web UI.
//
// The production build minifies every npm package it bundles, and the minifier
// drops the packages' own @license / @preserve comments. MIT, BSD, ISC and
// Apache-2.0 all require the copyright notice, the licence text and (Apache
// §4(d)) any NOTICE file to travel with a binary redistribution, so the build
// writes them to dist/THIRD-PARTY-LICENSES.txt instead (served at
// /THIRD-PARTY-LICENSES.txt, shipped at /app/frontend/dist/ in the image).
//
// Which packages: the ones whose code actually ends up in dist/, not a guess
// from package.json. A Vite plugin records every node_modules module the
// bundler put into an output chunk, for the main bundle and for each web worker
// bundle (workers are separate builds, so they need their own plugin instance
// via `worker.plugins`). That also catches devDependencies whose runtime is
// compiled in (the Svelte runtime) and leaves out build-only tools (Vite,
// Rolldown, the Tailwind compiler). Packages whose files reach dist/ outside
// the module graph are passed in as `extraPackages` (CesiumJS's runtime assets,
// copied into dist/cesium; Tailwind's CSS, compiled into the stylesheet); an
// extra package can take its dependency closure from package-lock.json.
//
// What goes in, per package: the licence its package.json declares and every
// LICENSE / LICENCE / COPYING / NOTICE file in the package root, verbatim, plus
// the `<file>.LICENSE.txt` that webpack writes next to a pre-bundled file (the
// licences of the libraries inside swagger-ui-dist's bundle). A package that
// ships no such file contributes the copyright comment its main file opens
// with, if any. Identical texts are printed once, followed by the packages they
// belong to. CesiumJS gets its ThirdParty.json and a map of the files in
// dist/cesium/ThirdParty to the npm packages they come from.
//
// Material that reaches dist/ without an npm package's licence files is passed
// in as `otherMaterial` (see otherBundledMaterial): web-ifc.wasm, copied from
// public/wasm with the C/C++ libraries statically linked into it; the Lucide and
// Feather icon shapes drawn inline in GraphCanvas.svelte; the EPSG parameters
// in src/lib/viewer/crs.ts. Their licence texts come verbatim from Open
// Triplestore's LICENSES/ directory (one level above frontend/; the Docker
// frontend stage copies it there), and the build fails when one is missing.
//
// Usage:
//   vite.config.js:  const licenses = thirdPartyLicenses({ extraPackages: [...],
//                      otherMaterial: otherBundledMaterial(<LICENSES dir>) });
//                    plugins: [..., licenses.plugin()],
//                    worker: { plugins: () => [licenses.workerPlugin()] }
//   package.json:    "build": "vite build && node scripts/third-party-licenses.mjs --check dist"

import fs from 'node:fs';
import path from 'node:path';
import process from 'node:process';
import { fileURLToPath } from 'node:url';

export const OUTPUT_FILE = 'THIRD-PARTY-LICENSES.txt';

/** Heading of the section on material bundled outside npm (checked by --check). */
export const OTHER_SECTION = 'Material bundled outside npm';

const LICENSE_FILE_RE = /^(licen[cs]e|copying|notice|third[-_ ]?party[-_ ]?(notices?|licen[cs]es?))([-._ ].*)?$/i;

/** Normalise a text for de-duplication only; the printed text stays verbatim. */
function dedupKey(text) {
  return text.replace(/\r\n?/g, '\n').replace(/[ \t]+$/gm, '').trim();
}

/**
 * The node_modules package directory a module id belongs to, or null.
 * Handles scoped packages, nested node_modules, query strings (`?worker&url`)
 * and a leading NUL on ids that plugins derive from a real file.
 */
export function packageDirOf(moduleId) {
  const id = moduleId.replace(/^\0+/, '').split('?')[0].replace(/\\/g, '/');
  const marker = '/node_modules/';
  const at = id.lastIndexOf(marker);
  if (at < 0) return null;
  const rest = id.slice(at + marker.length).split('/');
  if (!rest[0] || rest[0].startsWith('.')) return null; // .vite, .pnpm caches
  const scoped = rest[0].startsWith('@');
  if (scoped && !rest[1]) return null;
  return id.slice(0, at + marker.length) + rest.slice(0, scoped ? 2 : 1).join('/');
}

function readJson(file) {
  try {
    return JSON.parse(fs.readFileSync(file, 'utf8'));
  } catch {
    return null;
  }
}

/** SPDX expression (or legacy `licenses` array) a package.json declares. */
export function declaredLicense(pkg) {
  if (!pkg) return null;
  const one = (l) => (typeof l === 'string' ? l : l && typeof l.type === 'string' ? l.type : null);
  if (pkg.license) return one(pkg.license);
  if (Array.isArray(pkg.licenses)) {
    const all = pkg.licenses.map(one).filter(Boolean);
    return all.length ? all.join(' OR ') : null;
  }
  return null;
}

/** LICENSE / LICENCE / COPYING / NOTICE files in a package root, sorted. */
export function licenseFilesIn(dir) {
  let names;
  try {
    names = fs.readdirSync(dir);
  } catch {
    return [];
  }
  return names
    .filter((n) => LICENSE_FILE_RE.test(n))
    .filter((n) => {
      try {
        return fs.statSync(path.join(dir, n)).isFile();
      } catch {
        return false;
      }
    })
    .sort();
}

/**
 * The comments a source file opens with (before its first line of code) that
 * carry a copyright or licence statement, verbatim. Used only for packages
 * that ship no licence file, whose notice then lives in the source.
 */
export function leadingNoticeComments(source) {
  const out = [];
  let i = source.startsWith('#!') ? source.indexOf('\n') + 1 : 0;
  for (;;) {
    while (i < source.length && /\s/.test(source[i])) i++;
    let end;
    if (source.startsWith('/*', i)) {
      end = source.indexOf('*/', i + 2);
      if (end < 0) break;
      end += 2;
    } else if (source.startsWith('//', i)) {
      end = i;
      while (source.startsWith('//', end)) {
        const nl = source.indexOf('\n', end);
        end = nl < 0 ? source.length : nl + 1;
        while (end < source.length && /[ \t]/.test(source[end])) end++;
      }
    } else {
      break;
    }
    const comment = source.slice(i, end).trimEnd();
    if (/copyright|\(c\)|©|licen[cs]e/i.test(comment)) out.push(comment);
    i = end;
  }
  return out;
}

/** The file a package's `main` points at (or index.js), relative to its dir. */
function mainFileOf(dir, pkg) {
  for (const cand of [pkg.main, pkg.main && `${pkg.main}.js`, 'index.js']) {
    if (!cand) continue;
    const file = path.join(dir, cand);
    try {
      if (fs.statSync(file).isFile()) return path.relative(dir, file);
    } catch {
      // try the next candidate
    }
  }
  return null;
}

/** Package description for a package directory, or null if it has no package.json. */
function describePackage(dir) {
  const pkg = readJson(path.join(dir, 'package.json'));
  if (!pkg || !pkg.name) return null;
  const repo = typeof pkg.repository === 'string' ? pkg.repository : pkg.repository?.url;
  return {
    name: pkg.name,
    version: pkg.version || '0.0.0',
    license: declaredLicense(pkg),
    homepage: pkg.homepage || repo || null,
    dir,
    files: licenseFilesIn(dir),
    mainFile: mainFileOf(dir, pkg),
    // Extra licence files that belong to specific bundled files
    // (webpack's `<bundle>.js.LICENSE.txt`), relative to `dir`.
    bundledFileLicenses: new Set(),
    via: new Set(),
  };
}

// ── package-lock.json: dependency closure of a package ─────────────────────────

function lockPackages(root) {
  const lock = readJson(path.join(root, 'package-lock.json'));
  return lock && lock.packages ? lock.packages : null;
}

/** Node's lookup: `<from>/node_modules/<name>`, then each ancestor, then the root. */
function resolveInLock(packages, fromKey, name) {
  let base = fromKey;
  for (;;) {
    const key = (base ? base + '/' : '') + 'node_modules/' + name;
    if (packages[key]) return key;
    if (!base) return null;
    const cut = base.lastIndexOf('/node_modules/');
    base = cut < 0 ? '' : base.slice(0, cut);
  }
}

/**
 * Lock keys (e.g. `node_modules/@cesium/engine`) of `name` and everything it
 * depends on at runtime (dependencies + optionalDependencies; peers are the
 * host's), as installed at the top level of `root`.
 */
export function lockClosure(root, name, packages = lockPackages(root)) {
  if (!packages) throw new Error(`third-party-licenses: no package-lock.json in ${root}`);
  const start = resolveInLock(packages, '', name);
  if (!start) throw new Error(`third-party-licenses: ${name} is not in package-lock.json`);
  const seen = new Set();
  const todo = [start];
  while (todo.length) {
    const key = todo.pop();
    if (seen.has(key)) continue;
    seen.add(key);
    const entry = packages[key] || {};
    const deps = { ...(entry.dependencies || {}), ...(entry.optionalDependencies || {}) };
    for (const dep of Object.keys(deps)) {
      const found = resolveInLock(packages, key, dep);
      if (found) todo.push(found);
    }
  }
  return [...seen].sort();
}

// ── collection ────────────────────────────────────────────────────────────────

/** Mutable set of package directories and the reasons they are included. */
export class PackageRegistry {
  constructor(root) {
    this.root = root;
    this.byDir = new Map();
  }

  add(dir, via, bundledFile) {
    let entry = this.byDir.get(dir);
    if (!entry) {
      entry = describePackage(dir);
      if (!entry) return null;
      this.byDir.set(dir, entry);
    }
    entry.via.add(via);
    if (bundledFile) {
      const sidecar = bundledFile + '.LICENSE.txt';
      if (fs.existsSync(sidecar)) entry.bundledFileLicenses.add(path.relative(dir, sidecar));
    }
    return entry;
  }

  addModuleIds(moduleIds, via) {
    for (const id of moduleIds) {
      const dir = packageDirOf(id);
      if (!dir) continue;
      this.add(dir, via, id.replace(/^\0+/, '').split('?')[0]);
    }
  }

  addExtra({ name, withDependencies = false, reason }) {
    const keys = withDependencies ? lockClosure(this.root, name) : [`node_modules/${name}`];
    for (const key of keys) {
      const dir = path.join(this.root, key);
      if (!fs.existsSync(path.join(dir, 'package.json'))) continue; // optional, not installed
      this.add(dir, key === `node_modules/${name}` ? reason : `${reason} (dependency of ${name})`);
    }
  }

  /** One entry per name@version, sorted. */
  packages() {
    const byKey = new Map();
    for (const e of this.byDir.values()) {
      const key = `${e.name}@${e.version}`;
      const prev = byKey.get(key);
      if (!prev) {
        byKey.set(key, e);
      } else {
        for (const v of e.via) prev.via.add(v);
      }
    }
    return [...byKey.values()].sort((a, b) =>
      a.name === b.name ? a.version.localeCompare(b.version) : a.name.localeCompare(b.name),
    );
  }
}

// ── Cesium: bundled third-party components and ThirdParty/ files ──────────────

function walkFiles(dir, base = dir) {
  const out = [];
  for (const d of fs.readdirSync(dir, { withFileTypes: true })) {
    const p = path.join(dir, d.name);
    if (d.isDirectory()) out.push(...walkFiles(p, base));
    else if (d.isFile() && d.name !== 'package.json') out.push(path.relative(base, p));
  }
  return out.sort();
}

function sameBytes(a, b) {
  try {
    return fs.readFileSync(a).equals(fs.readFileSync(b));
  } catch {
    return false;
  }
}

/**
 * CesiumJS's own list of bundled third-party components (ThirdParty.json) and,
 * for each file in the ThirdParty/ runtime directory that the build copies to
 * dist/cesium/ThirdParty, the npm package in Cesium's dependency closure that
 * ships a file of the same name (byte-identical or not).
 */
export function cesiumThirdParty(root, cesiumRuntimeDir) {
  const cesiumDir = path.join(root, 'node_modules', 'cesium');
  const components = readJson(path.join(cesiumDir, 'ThirdParty.json')) || [];
  const files = [];
  const tpDir = path.join(cesiumRuntimeDir, 'ThirdParty');
  if (fs.existsSync(tpDir)) {
    const closure = lockClosure(root, 'cesium').filter(
      (k) => !/node_modules\/(cesium|@cesium\/engine|@cesium\/widgets)$/.test(k),
    );
    for (const rel of walkFiles(tpDir)) {
      const base = path.basename(rel);
      let match = null;
      for (const key of closure) {
        const pkgDir = path.join(root, key);
        const hit = fs.existsSync(pkgDir) ? findFile(pkgDir, base) : null;
        if (hit) {
          const pkg = readJson(path.join(pkgDir, 'package.json')) || {};
          match = {
            name: pkg.name,
            version: pkg.version,
            identical: sameBytes(path.join(tpDir, rel), hit),
          };
          break;
        }
      }
      files.push({ file: `cesium/ThirdParty/${rel.replace(/\\/g, '/')}`, match });
    }
  }
  return { components, files };
}

function findFile(dir, name, depth = 3) {
  let entries;
  try {
    entries = fs.readdirSync(dir, { withFileTypes: true });
  } catch {
    return null;
  }
  for (const d of entries) if (d.isFile() && d.name === name) return path.join(dir, d.name);
  if (depth === 0) return null;
  for (const d of entries) {
    if (d.isDirectory() && d.name !== 'node_modules') {
      const hit = findFile(path.join(dir, d.name), name, depth - 1);
      if (hit) return hit;
    }
  }
  return null;
}

// ── Material bundled outside npm ──────────────────────────────────────────────

/** Where web-ifc.wasm comes from and what is linked into it (NOTICE, "web-ifc"). */
const WEB_IFC_COMMIT = 'f26c4beef0a668ebdb180d2b95a94097a1e21cef';
const CDT_COMMIT = '4d0c9026b8ec846fe544897e7111f8f9080d5f8a';
const LLVM = 'Apache-2.0 WITH LLVM-exception (legacy code: University of Illinois/NCSA or MIT)';

/**
 * The third-party material the web UI ships that no npm package's licence
 * files cover, with the texts (paths relative to `licensesDir`, Open
 * Triplestore's LICENSES/ directory) that must travel with it. Each item is
 * included when its trigger is in the build: `publicFile` (a file Vite copies
 * from public/ into dist/) or `modules` (a source module the bundler put into
 * an output chunk). Every `copyright` line must occur in its text (checked by
 * the unit test), so the metadata cannot drift from the texts.
 */
export function otherBundledMaterial(licensesDir) {
  return [
    {
      id: 'web-ifc.wasm',
      title: 'dist/wasm/web-ifc.wasm',
      publicFile: 'wasm/web-ifc.wasm',
      intro:
        'The IFC parser the web UI loads in the browser: web-ifc 0.0.77, byte-identical to ' +
        'web-ifc.wasm in the npm package web-ifc@0.0.77, unmodified, built with Emscripten 4.0.23. ' +
        'It is licensed under the Mozilla Public License 2.0; its Source Code Form (MPL-2.0 ' +
        `section 3.2) is https://github.com/ThatOpen/engine_web-ifc at commit ${WEB_IFC_COMMIT}. ` +
        'The libraries below are statically linked into it (commits as pinned in ' +
        'src/cpp/CMakeLists.txt at that web-ifc commit). The Emscripten runtime is also in the ' +
        "web-ifc package's JavaScript, which section 1 lists.",
      licensesDir,
      components: [
        {
          name: 'web-ifc 0.0.77',
          source: `https://github.com/ThatOpen/engine_web-ifc @ ${WEB_IFC_COMMIT}`,
          licence: 'MPL-2.0',
          copyright: [],
          note: 'By the web-ifc contributors (ThatOpen).',
          texts: ['web-ifc/web-ifc.txt'],
        },
        {
          name: 'glm',
          source: 'https://github.com/g-truc/glm @ 8d1fd52e5ab5590e2c81768ace50c72bae28f2ed',
          licence: 'MIT (offered under The Happy Bunny License or the MIT License; used under MIT)',
          copyright: ['Copyright (c) 2005 - G-Truc Creation'],
          texts: ['web-ifc/glm.txt'],
        },
        {
          name: 'stduuid',
          source: 'https://github.com/mariusbancila/stduuid @ 3afe7193facd5d674de709fccc44d5055e144d7a',
          licence: 'MIT',
          copyright: ['Copyright (c) 2017'],
          note: "Its LICENSE names no holder; the repository is Marius Bancila's.",
          texts: ['web-ifc/stduuid.txt'],
        },
        {
          name: 'spdlog 1.15.1',
          source: 'https://github.com/gabime/spdlog @ f355b3d58f7067eee1706ff3c801c2361011f3d5',
          licence: 'MIT',
          copyright: ['Copyright (c) 2016 Gabi Melman.'],
          texts: ['web-ifc/spdlog.txt'],
        },
        {
          name: '{fmt} 11.1.3 (the copy spdlog bundles)',
          source: 'https://github.com/fmtlib/fmt',
          licence: 'MIT, with an optional exception for object code',
          copyright: ['Copyright (c) 2012 - present, Victor Zverovich'],
          texts: ['web-ifc/fmt.txt'],
        },
        {
          name: 'earcut.hpp',
          source: 'https://github.com/mapbox/earcut.hpp @ 4811a2b69b91f6127a75e780de6e2113609ddabb',
          licence: 'ISC',
          copyright: ['Copyright (c) 2015, Mapbox'],
          texts: ['web-ifc/earcut.hpp.txt'],
        },
        {
          name: 'tinynurbs',
          source:
            'https://github.com/QuimMoya/tinynurbs @ 47115cd9b6e922b27bbc4ab01fdeac2e9ea597a4 ' +
            '(a fork of pradeep-pyro/tinynurbs)',
          licence: 'BSD-3-Clause',
          copyright: ['Copyright (c) 2017, Pradeep Kumar Jayaraman'],
          texts: ['web-ifc/tinynurbs.txt'],
        },
        {
          name: 'CDT',
          source: `https://github.com/artem-ogre/CDT @ ${CDT_COMMIT}`,
          licence: 'MPL-2.0',
          copyright: [],
          note: `Source Code Form (MPL-2.0 section 3.2): https://github.com/artem-ogre/CDT at commit ${CDT_COMMIT}.`,
          texts: ['web-ifc/CDT.txt'],
        },
        {
          name: "CDT's CDT/include/predicates.h",
          source: `https://github.com/artem-ogre/CDT @ ${CDT_COMMIT}`,
          licence: 'BSD-3-Clause',
          copyright: ['Copyright (c) 2019, William C. Lenthe'],
          texts: ['web-ifc/CDT-predicates.txt'],
        },
        {
          name: 'fast_float',
          source: 'https://github.com/fastfloat/fast_float @ 2b2395f9ac836ffca6404424bcc252bff7aa80e4',
          licence: 'MIT (offered under Apache-2.0, MIT or BSL-1.0; used under MIT)',
          copyright: ['Copyright (c) 2021 The fast_float authors'],
          texts: ['web-ifc/fast_float.txt'],
        },
        {
          name: 'Emscripten 4.0.23 runtime (embind and runtime)',
          source: 'https://github.com/emscripten-core/emscripten',
          licence: 'MIT / University of Illinois/NCSA',
          copyright: ['Copyright (c) 2010-2014 Emscripten authors'],
          texts: ['web-ifc/emscripten.txt'],
        },
        {
          name: 'musl libc (Emscripten system library)',
          source: 'https://musl.libc.org/',
          licence: 'MIT',
          copyright: ['Copyright © 2005-2020 Rich Felker, et al.'],
          texts: ['web-ifc/musl.txt'],
        },
        {
          name: 'LLVM libc++ (Emscripten system library)',
          source: 'https://github.com/llvm/llvm-project',
          licence: LLVM,
          copyright: [],
          texts: ['web-ifc/libcxx.txt'],
        },
        {
          name: 'LLVM libc++abi (Emscripten system library)',
          source: 'https://github.com/llvm/llvm-project',
          licence: LLVM,
          copyright: [],
          texts: ['web-ifc/libcxxabi.txt'],
        },
        {
          name: 'LLVM compiler-rt (Emscripten system library)',
          source: 'https://github.com/llvm/llvm-project',
          licence: LLVM,
          copyright: [],
          texts: ['web-ifc/compiler-rt.txt'],
        },
        {
          name: 'dlmalloc (Emscripten system library)',
          source: 'https://gee.cs.oswego.edu/dl/html/malloc.html',
          licence: 'Public domain (CC0), by Doug Lea',
          copyright: [],
          texts: [],
        },
      ],
    },
    {
      id: 'icons',
      title: 'Inline SVG icons of the graph view',
      modules: [/\/src\/components\/GraphCanvas\.svelte$/],
      intro:
        'The graph view (src/components/GraphCanvas.svelte) draws nine toolbar icons inline, with ' +
        'shapes copied from Lucide, v0.100.0-era geometry, in compact, geometrically equivalent ' +
        "form (the SVG attributes are the project's): pin and rotate-ccw are Lucide's own " +
        "drawings; search, maximize-2, zoom-in, zoom-out, download, settings and x are Feather's.",
      licensesDir,
      components: [
        {
          name: 'Lucide',
          source: 'https://github.com/lucide-icons/lucide',
          licence: 'ISC',
          copyright: [
            'Copyright (c) for portions of Lucide are held by Cole Bemis 2013-2022 as part of Feather ' +
              '(MIT). All other copyright (c) for Lucide are held by Lucide Contributors 2022.',
          ],
          texts: ['Lucide-ISC.txt'],
        },
        {
          name: 'Feather',
          source: 'https://github.com/feathericons/feather',
          licence: 'MIT',
          copyright: ['Copyright (c) 2013-2023 Cole Bemis'],
          texts: ['Feather-MIT.txt'],
        },
      ],
    },
    {
      id: 'epsg',
      title: 'EPSG Geodetic Parameter Dataset',
      modules: [/\/src\/lib\/viewer\/crs\.ts$/],
      intro:
        'The coordinate reference system parameters in the 3D viewer (src/lib/viewer/crs.ts) derive ' +
        'from the EPSG Geodetic Parameter Dataset, which is owned by IOGP, the International ' +
        'Association of Oil and Gas Producers (https://epsg.org). They are used under the EPSG ' +
        'Dataset Terms of Use (https://epsg.org/terms-of-use.html), which also apply to anyone ' +
        'they are passed on to; the dataset comes without warranty. epsg.io and PROJ recast them ' +
        'as proj4 strings, so they are not verbatim EPSG records.',
      licensesDir,
      components: [],
    },
  ];
}

/**
 * The items of `otherMaterial` whose trigger is in this build, with each
 * licence text read verbatim and, for module-triggered items, the output files
 * that contain the module. Throws when a text cannot be read, so a build that
 * lacks the LICENSES/ directory fails instead of shipping incomplete notices.
 */
export function resolveOtherMaterial(otherMaterial, { root, bundle }) {
  const chunks = Object.values(bundle || {}).filter((c) => c.type === 'chunk');
  const out = [];
  for (const item of otherMaterial) {
    let where = [];
    if (item.publicFile) {
      if (!fs.existsSync(path.join(root, 'public', item.publicFile))) continue;
      where = [`dist/${item.publicFile}`];
    } else if (item.modules) {
      where = chunks
        .filter((c) =>
          (c.moduleIds || []).some((id) =>
            item.modules.some((re) => re.test(id.replace(/^\0+/, '').split('?')[0].replace(/\\/g, '/'))),
          ),
        )
        .map((c) => `dist/${c.fileName}`)
        .sort();
      if (!where.length) continue;
    }
    const components = item.components.map((c) => ({
      ...c,
      texts: c.texts.map((rel) => {
        const file = path.join(item.licensesDir, rel);
        let text;
        try {
          text = fs.readFileSync(file, 'utf8');
        } catch (e) {
          throw new Error(
            `third-party-licenses: cannot read ${file} (${e.code || e.message}), the licence text ` +
              `for ${c.name} in ${item.title}; the LICENSES/ directory must be next to frontend/`,
          );
        }
        return { label: `LICENSES/${rel.replace(/\\/g, '/')}`, text };
      }),
    }));
    out.push({ ...item, where, components });
  }
  return out;
}

// ── rendering ────────────────────────────────────────────────────────────────

// How to recognise a complete copy of a licence among the texts collected, so
// a package that ships no licence file can be pointed at one. Every `all`
// phrase must occur in the text (whitespace-insensitive), no `absent` phrase.
const LICENCE_SIGNATURES = {
  'Apache-2.0': { all: ['Apache License', 'Version 2.0, January 2004', 'END OF TERMS AND CONDITIONS'] },
  MIT: { all: ['Permission is hereby granted, free of charge', 'THE SOFTWARE IS PROVIDED "AS IS"'] },
  'MPL-2.0': { all: ['Mozilla Public License Version 2.0', 'Exhibit A'] },
  'BSD-3-Clause': {
    all: ['Redistributions of source code must retain', 'Redistributions in binary form must reproduce', 'endorse or promote products'],
  },
  'BSD-2-Clause': {
    all: ['Redistributions of source code must retain', 'Redistributions in binary form must reproduce'],
    absent: ['endorse or promote products'],
  },
  ISC: { all: ['Permission to use, copy, modify, and/or distribute this software for any purpose'] },
};

const squash = (s) => s.replace(/\s+/g, ' ');

/** Licence identifiers in an SPDX expression, e.g. "(MIT OR Apache-2.0)". */
export function licenceIds(expr) {
  if (!expr) return [];
  const ids = [];
  for (const tok of expr.match(/[A-Za-z0-9.+-]+/g) || []) {
    if (/^(OR|AND|WITH)$/i.test(tok) || tok.endsWith('-exception')) continue;
    if (!ids.includes(tok)) ids.push(tok);
  }
  return ids;
}

/**
 * For each licence in LICENCE_SIGNATURES, the number of the shortest text among
 * `slots` that contains a complete copy of it: a plain licence file rather than
 * a compilation such as CesiumJS's LICENSE.md.
 */
export function referenceTexts(slots) {
  const best = {};
  for (const slot of slots) {
    const text = squash(slot.text);
    for (const [id, sig] of Object.entries(LICENCE_SIGNATURES)) {
      const matches =
        sig.all.every((p) => text.includes(squash(p))) && !(sig.absent || []).some((p) => text.includes(squash(p)));
      if (matches && (!best[id] || text.length < best[id].length)) best[id] = { n: slot.n, length: text.length };
    }
  }
  return Object.fromEntries(Object.entries(best).map(([id, b]) => [id, b.n]));
}

const RULE = '='.repeat(78);
const THIN = '-'.repeat(78);

function wrap(text, width = 78, indent = '') {
  const words = text.split(/\s+/).filter(Boolean);
  const lines = [];
  let line = indent;
  for (const w of words) {
    if (line.trim() && line.length + 1 + w.length > width) {
      lines.push(line);
      line = indent + w;
    } else {
      line = line.trim() ? `${line} ${w}` : indent + w;
    }
  }
  if (line.trim()) lines.push(line);
  return lines.join('\n');
}

/** Render the notice file. Pure apart from reading the licence files. */
export function renderNotices({ packages, cesium, other = [] }) {
  // Collect texts, de-duplicated; remember which package/file pairs use each.
  const texts = new Map(); // dedup key → slot
  const order = []; // slots in first-seen order: { text, users, n }
  const refs = new Map(); // package key → [text numbers]
  const addText = (pkgKey, text, label) => {
    if (!text.trim()) return null;
    const key = dedupKey(text);
    let slot = texts.get(key);
    if (!slot) {
      slot = { text, users: [], n: order.length + 1 };
      texts.set(key, slot);
      order.push(slot);
    }
    slot.users.push(`${pkgKey} (${label})`);
    const own = refs.get(pkgKey);
    if (!own.includes(slot.n)) own.push(slot.n);
    return slot;
  };
  const read = (file) => {
    try {
      return fs.readFileSync(file, 'utf8');
    } catch {
      return '';
    }
  };

  const fromSource = new Set(); // packages whose only notice is a source comment
  for (const p of packages) {
    const pkgKey = `${p.name}@${p.version}`;
    refs.set(pkgKey, []);
    const rels = [...p.files, ...[...p.bundledFileLicenses].sort()];
    for (const rel of rels) addText(pkgKey, read(path.join(p.dir, rel)), rel.replace(/\\/g, '/'));
    if (!refs.get(pkgKey).length && p.mainFile) {
      const rel = p.mainFile.replace(/\\/g, '/');
      for (const c of leadingNoticeComments(read(path.join(p.dir, p.mainFile)))) {
        if (addText(pkgKey, c, `comment at the top of ${rel}`)) fromSource.add(pkgKey);
      }
    }
  }

  // Material bundled outside npm: its texts join the same numbered list, so
  // identical texts (web-ifc's MPL, Lucide's ISC) are printed once.
  const otherKey = (item, c) => `${item.id}: ${c.name}`;
  for (const item of other) {
    for (const c of item.components) {
      refs.set(otherKey(item, c), []);
      for (const t of c.texts) addText(otherKey(item, c), t.text, t.label);
    }
  }

  // CesiumJS's LICENSE.md reproduces the notices of the third-party code it
  // bundles (its "Third-Party Code" section, keyed like ThirdParty.json).
  const cesiumPkg = packages.find((p) => p.name === 'cesium');
  const cesiumLicense = cesiumPkg
    ? order.find((s) => s.users.includes(`cesium@${cesiumPkg.version} (LICENSE.md)`))
    : null;
  const norm = (s) => String(s).toLowerCase().replace(/[^a-z0-9]/g, '');
  const cesiumComponents = new Set((cesium?.components || []).map((c) => norm(c.name)));
  const refTexts = referenceTexts(order);

  const out = [];
  const para = (text) => {
    out.push(wrap(text));
    out.push('');
  };
  out.push(RULE);
  out.push('Third-party software notices and licences: Open Triplestore web UI');
  out.push(RULE);
  out.push('');
  para(
    'Generated by frontend/scripts/third-party-licenses.mjs during the production build ' +
      '(npm run build) from the npm packages installed for that build. Do not edit it by hand.',
  );
  para(
    'This web UI (the dist/ directory) contains code from the npm packages listed in ' +
      'section 1: modules the bundler compiled into dist/assets/ (the application and its web ' +
      'workers); the CesiumJS runtime files the build copies, unmodified, into dist/cesium/, ' +
      'listed together with the dependency tree of the cesium package they were built from ' +
      '(some of those dependencies may contribute no code); and the Tailwind CSS base styles ' +
      'compiled into the stylesheet. The bundled code is minified, which removes the ' +
      'licence comments in its source; this file carries those notices instead. Each ' +
      'package is distributed under its own licence, shown next to it as declared in its ' +
      'package.json. The AGPL-3.0 and the Commons Clause that cover Open Triplestore do not ' +
      'apply to these packages.',
  );
  para(
    'Section 2 lists the third-party components CesiumJS bundles into its own build and ' +
      'the files in dist/cesium/ThirdParty/. Section 3 covers third-party material that ' +
      'reaches dist/ without an npm package: dist/wasm/web-ifc.wasm and the libraries ' +
      'statically linked into it, icon shapes drawn inline in the application code, and ' +
      'the EPSG parameters in the 3D viewer. Section 4 reproduces, verbatim, every licence, ' +
      'copying and notice file those packages ship (for a package that ships none, the ' +
      'copyright comment its main source file opens with) and the licence texts of the ' +
      'material in section 3. Each distinct text appears once, followed by the packages and ' +
      'files it comes from; the numbers in [brackets] in sections 1 and 3 refer to these texts.',
  );
  para(
    'Not covered here: the vocabularies in dist/vocab/ (see vocab/NOTICE.md) and the sample ' +
      'data in dist/samples/. Their notices are in the NOTICE file and the LICENSES/ directory ' +
      'of Open Triplestore (/app/NOTICE and /app/LICENSES/ in the Docker image).',
  );

  out.push(RULE);
  out.push(`1. Packages (${packages.length})`);
  out.push(RULE);
  out.push('');
  const missing = [];
  for (const p of packages) {
    const pkgKey = `${p.name}@${p.version}`;
    const nums = refs.get(pkgKey);
    let texts = nums.map((n) => `[${n}]`).join(' ');
    if (fromSource.has(pkgKey)) texts += ' (no licence file in the package; notice from its source)';
    if (!nums.length) {
      texts = 'no licence or notice file in the package';
      if (cesiumLicense && cesiumComponents.has(norm(p.name))) {
        texts += `; CesiumJS bundles it and gives its notice in the Third-Party Code section of [${cesiumLicense.n}]`;
      }
      const known = licenceIds(p.license).filter((id) => refTexts[id]);
      if (known.length) {
        texts += `; licence text: ${known.map((id) => `${id} [${refTexts[id]}]`).join(', ')} (as shipped by another package)`;
      }
      missing.push(pkgKey);
    }
    out.push(`${p.name} ${p.version}`);
    out.push(`  Licence: ${p.license || '(none declared in package.json)'}`);
    if (p.homepage) out.push(`  Source:  ${p.homepage}`);
    out.push(wrap(`Texts: ${texts}`, 78, '  '));
    out.push(wrap(`Included as: ${[...p.via].sort().join('; ')}`, 78, '  '));
    out.push('');
  }
  if (missing.length) {
    para(
      'Packages marked "no licence or notice file in the package" publish none in their npm ' +
        'package; their terms are the licence their package.json declares, shown above, ' +
        'whose full text section 3 reproduces from another package where marked "licence ' +
        'text" (read the copyright line in it as that package\'s, not theirs).',
    );
  }

  if (cesium) {
    out.push(RULE);
    out.push('2. CesiumJS third-party components');
    out.push(RULE);
    out.push('');
    para(
      'CesiumJS lists the following third-party code in its ThirdParty.json. The ' +
        '"Third-Party Code" section of its LICENSE.md' +
        (cesiumLicense ? ` ([${cesiumLicense.n}] in section 3)` : '') +
        ' gives their copyright notices and licence terms.',
    );
    for (const c of cesium.components) {
      const lic = Array.isArray(c.license) ? c.license.join(' OR ') : c.license || '';
      out.push(`  ${[c.name, c.version].filter(Boolean).join(' ')} - ${lic}${c.url ? ` - ${c.url}` : ''}`);
      if (c.notes) out.push(wrap(`(${c.notes})`, 78, '    '));
    }
    out.push('');
    if (cesium.files.length) {
      para(
        'Files in dist/cesium/ThirdParty/, copied unmodified from the cesium package, with ' +
          'the npm package among Cesium\'s dependencies that ships a file of the same name. ' +
          'A file without one is CesiumJS\'s own build of a component listed above.',
      );
      for (const f of cesium.files) {
        const m = f.match
          ? `${f.match.name} ${f.match.version}${f.match.identical ? ' (byte-identical)' : ''}`
          : `built by CesiumJS; see its LICENSE.md${cesiumLicense ? ` [${cesiumLicense.n}]` : ''}`;
        out.push(`  ${f.file}`);
        out.push(`    ${m}`);
      }
      out.push('');
    }
  }

  if (other.length) {
    out.push(RULE);
    out.push(`3. ${OTHER_SECTION}`);
    out.push(RULE);
    out.push('');
    for (const item of other) {
      out.push(THIN);
      out.push(item.title);
      out.push(THIN);
      if (item.where.length && item.where.join() !== item.title) {
        out.push(wrap(`In: ${item.where.join(', ')}`, 78, '  '));
      }
      out.push(wrap(item.intro, 78, '  '));
      out.push('');
      for (const c of item.components) {
        const nums = refs.get(otherKey(item, c));
        out.push(`  ${c.name}`);
        out.push(wrap(`Licence: ${c.licence}`, 78, '    '));
        if (c.source) out.push(wrap(`Source:  ${c.source}`, 78, '    '));
        for (const line of c.copyright) out.push(wrap(line, 78, '    '));
        if (c.note) out.push(wrap(c.note, 78, '    '));
        if (nums.length) out.push(wrap(`Texts: ${nums.map((n) => `[${n}]`).join(' ')}`, 78, '    '));
        out.push('');
      }
    }
  }

  out.push(RULE);
  out.push('4. Licence and notice texts');
  out.push(RULE);
  for (const slot of order) {
    out.push('');
    out.push(THIN);
    out.push(`[${slot.n}]`);
    for (const u of slot.users) out.push(wrap(u, 78, '  '));
    out.push(THIN);
    out.push('');
    out.push(slot.text.replace(/\r\n?/g, '\n').replace(/\s+$/, ''));
  }
  out.push('');
  return { text: out.join('\n'), missing, textCount: order.length };
}

// ── Vite plugin ──────────────────────────────────────────────────────────────

/**
 * @param {object} options
 * @param {string} [options.root] the frontend directory (default: the cwd)
 * @param {{name: string, withDependencies?: boolean, reason: string}[]} [options.extraPackages]
 * @param {string} [options.cesiumRuntimeDir] Build/Cesium directory the build copies to dist/cesium
 * @param {object[]} [options.otherMaterial] material bundled outside npm (see otherBundledMaterial)
 */
export function thirdPartyLicenses({
  root = process.cwd(),
  extraPackages = [],
  cesiumRuntimeDir,
  otherMaterial = [],
} = {}) {
  const registry = new PackageRegistry(root);
  const collect = (via) =>
    function generateBundle(_options, bundle) {
      for (const chunk of Object.values(bundle)) {
        if (chunk.type === 'chunk') registry.addModuleIds(chunk.moduleIds || [], via);
      }
    };
  return {
    workerPlugin() {
      return { name: 'ots-third-party-licenses:worker', apply: 'build', generateBundle: collect('bundled (web worker)') };
    },
    plugin() {
      return {
        name: 'ots-third-party-licenses',
        apply: 'build',
        enforce: 'post',
        generateBundle(options, bundle) {
          // Workers are bundled while the main build loads them, so their
          // modules are already recorded when the main bundle is generated.
          collect('bundled')(options, bundle);
          for (const extra of extraPackages) registry.addExtra(extra);
          const cesium = cesiumRuntimeDir ? cesiumThirdParty(root, cesiumRuntimeDir) : null;
          let other;
          try {
            other = resolveOtherMaterial(otherMaterial, { root, bundle });
          } catch (e) {
            this.error(e.message);
          }
          for (const item of otherMaterial) {
            if (!other.some((o) => o.id === item.id)) {
              this.warn(`${item.title} is configured but not in this build; ${OUTPUT_FILE} leaves it out`);
            }
          }
          const { text, missing } = renderNotices({ packages: registry.packages(), cesium, other });
          if (missing.length) {
            this.warn(`packages without a licence or notice file (see ${OUTPUT_FILE}): ${missing.join(', ')}`);
          }
          this.emitFile({ type: 'asset', fileName: OUTPUT_FILE, source: text });
        },
      };
    },
  };
}

// ── CLI: post-build check ────────────────────────────────────────────────────

/**
 * Check a built dist/ directory: the notice file exists, names at least one
 * package, and reproduces at least one licence text. Returns a list of problems.
 */
export function checkDist(distDir) {
  const file = path.join(distDir, OUTPUT_FILE);
  if (!fs.existsSync(file)) {
    return [`${file} is missing: is the thirdPartyLicenses() plugin still in vite.config.js?`];
  }
  const text = fs.readFileSync(file, 'utf8');
  const problems = [];
  const m = /^1\. Packages \((\d+)\)$/m.exec(text);
  if (!m || Number(m[1]) === 0) problems.push(`${file} lists no packages`);
  if (!/^\[1\]$/m.test(text)) problems.push(`${file} reproduces no licence texts`);
  // Material that reaches browsers without an npm package's notices must have
  // its own: web-ifc.wasm (and the libraries linked into it) whenever it is
  // shipped, and the EPSG notice whenever a bundle carries proj4 definitions.
  const other = text.split(`\n3. ${OTHER_SECTION}\n`)[1]?.split('\n4. Licence and notice texts\n')[0] || '';
  if (fs.existsSync(path.join(distDir, 'wasm', 'web-ifc.wasm'))) {
    if (!/^dist\/wasm\/web-ifc\.wasm$/m.test(other)) {
      problems.push(`${file} does not cover dist/wasm/web-ifc.wasm and the libraries linked into it`);
    }
  }
  if (bundlesMention(distDir, '+proj=') && !other.includes('EPSG Geodetic Parameter Dataset')) {
    problems.push(`${file} lacks the EPSG notice for the coordinate reference system parameters`);
  }
  return problems;
}

/** Whether any JavaScript file under dist/assets contains `needle`. */
function bundlesMention(distDir, needle) {
  const assets = path.join(distDir, 'assets');
  let names;
  try {
    names = fs.readdirSync(assets);
  } catch {
    return false;
  }
  return names
    .filter((n) => n.endsWith('.js'))
    .some((n) => {
      try {
        return fs.readFileSync(path.join(assets, n), 'utf8').includes(needle);
      } catch {
        return false;
      }
    });
}

if (process.argv[1] && fileURLToPath(import.meta.url) === path.resolve(process.argv[1])) {
  const args = process.argv.slice(2);
  if (args[0] !== '--check' || !args[1]) {
    console.error('usage: node scripts/third-party-licenses.mjs --check <dist-dir>');
    process.exit(2);
  }
  const problems = checkDist(path.resolve(args[1]));
  if (problems.length) {
    for (const p of problems) console.error(`third-party-licenses: ${p}`);
    process.exit(1);
  }
  const text = fs.readFileSync(path.join(path.resolve(args[1]), OUTPUT_FILE), 'utf8');
  const count = /^1\. Packages \((\d+)\)$/m.exec(text)[1];
  console.log(`third-party-licenses: ${args[1]}/${OUTPUT_FILE} covers ${count} packages`);
}
