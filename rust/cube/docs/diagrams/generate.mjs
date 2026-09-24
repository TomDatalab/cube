// Generates the architecture diagrams of the Rust backend.
//
// Each diagram is described once below and written twice:
//   <name>.excalidraw  editable at https://excalidraw.com (File → Open)
//   <name>.svg         hand-drawn rendering embedded in the README
//
// The SVG uses roughjs, the library Excalidraw draws with, and embeds the
// Virgil font (Excalidraw's handwriting face, SIL Open Font License 1.1,
// https://github.com/excalidraw/virgil) so it renders the same everywhere.
//
// Usage (Node.js is only needed to regenerate the pictures):
//   npm install roughjs@4
//   curl -LO https://unpkg.com/@excalidraw/excalidraw@0.17.6/dist/excalidraw-assets/Virgil.woff2
//   VIRGIL_WOFF2=./Virgil.woff2 node generate.mjs
// The font is required, so regenerated SVGs match the committed ones. For a
// quick local preview only, `--allow-system-font` falls back to a system
// cursive font; do not commit SVGs made that way.

import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import rough from 'roughjs';

const OUT = path.dirname(fileURLToPath(import.meta.url));
const FONT = (() => {
  const file = process.env.VIRGIL_WOFF2;
  if (file && fs.existsSync(file)) return fs.readFileSync(file).toString('base64');
  if (process.argv.includes('--allow-system-font')) {
    console.warn('warning: Virgil not embedded; these SVGs are for preview only');
    return null;
  }
  console.error(file
    ? `VIRGIL_WOFF2=${file} does not exist.`
    : 'VIRGIL_WOFF2 is not set.');
  console.error('The diagrams embed the Virgil font; see the usage notes at the top of generate.mjs.');
  process.exit(1);
})();

// Excalidraw's palette.
const C = {
  ink: '#1e1e1e',
  grey: '#868e96',
  red: '#e03131', redBg: '#ffc9c9',
  orange: '#f08c00', orangeBg: '#ffec99',
  green: '#2f9e44', greenBg: '#b2f2bb',
  blue: '#1971c2', blueBg: '#a5d8ff',
  violet: '#6741d9', violetBg: '#d0bfff',
  teal: '#0c8599', tealBg: '#99e9f2',
  white: '#ffffff',
};

// ------------------------------------------------------------------- model
// A diagram is a list of shapes:
//   { box: id, x, y, w, h, text, stroke, fill, size?, dashed?, title? }
//   { label: text, x, y, size?, color?, align? }
//   { arrow: [fromId, toId] | points: [[x,y],...], text?, stroke?, dashed?, both? }

function edgePoint(b, tx, ty) {
  // Where the segment from the box centre towards (tx, ty) leaves the box.
  const cx = b.x + b.w / 2, cy = b.y + b.h / 2;
  const dx = tx - cx, dy = ty - cy;
  if (dx === 0 && dy === 0) return [cx, cy];
  const sx = (b.w / 2) / Math.abs(dx || 1e-9), sy = (b.h / 2) / Math.abs(dy || 1e-9);
  const s = Math.min(sx, sy);
  return [cx + dx * s, cy + dy * s];
}

function resolveArrow(a, boxes) {
  if (a.points) return a.points;
  const [f, t] = a.arrow.map((id) => boxes[id]);
  const fc = [f.x + f.w / 2, f.y + f.h / 2], tc = [t.x + t.w / 2, t.y + t.h / 2];
  return [edgePoint(f, ...tc), edgePoint(t, ...fc)];
}

// --------------------------------------------------------------------- svg
function esc(s) {
  return s.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;');
}

function textSvg(lines, x, y, { size = 20, color = C.ink, align = 'middle', weight } = {}) {
  const lh = size * 1.25;
  const top = y - ((lines.length - 1) * lh) / 2;
  return lines.map((l, i) =>
    `<text x="${x}" y="${top + i * lh}" font-size="${size}" fill="${color}" text-anchor="${align}"` +
    ` dominant-baseline="central"${weight ? ` font-weight="${weight}"` : ''}>${esc(l)}</text>`).join('');
}

function toSvg(d) {
  const gen = rough.generator();
  let seed = 7;
  const opt = (o) => ({ roughness: 1.1, bowing: 1, strokeWidth: 1.6, seed: seed++, ...o });
  const paths = (drawable) => gen.toPaths(drawable).map((p) =>
    `<path d="${p.d}" stroke="${p.stroke}" stroke-width="${p.strokeWidth}" fill="${p.fill || 'none'}"` +
    `${drawable.options.strokeLineDash ? ` stroke-dasharray="${drawable.options.strokeLineDash.join(' ')}"` : ''}/>`).join('');

  const boxes = Object.fromEntries(d.shapes.filter((s) => s.box).map((s) => [s.box, s]));
  let body = '';
  for (const s of d.shapes) {
    if (s.box) {
      body += paths(gen.rectangle(s.x, s.y, s.w, s.h, opt({
        stroke: s.stroke || C.ink,
        fill: s.fill,
        fillStyle: s.fillStyle || 'hachure',
        hachureGap: 9,
        fillWeight: 1.2,
        strokeLineDash: s.dashed ? [8, 8] : undefined,
        roughness: s.title ? 0.6 : 1.1,
      })));
      if (s.title) {
        const right = s.titleAlign === 'end';
        body += textSvg([s.title], s.titleX ?? (right ? s.x + s.w - 14 : s.x + 14), s.y + 22,
          { size: s.titleSize || 22, align: right ? 'end' : 'start', color: s.stroke || C.ink });
      }
      if (s.text) {
        const lines = s.text.split('\n');
        body += textSvg(lines, s.x + s.w / 2, s.y + s.h / 2 + (s.title ? 12 : 0), { size: s.size || 18 });
      }
    } else if (s.label) {
      body += textSvg(s.label.split('\n'), s.x, s.y, { size: s.size || 18, color: s.color || C.ink, align: s.align || 'middle' });
    }
  }
  for (const s of d.shapes.filter((a) => a.arrow || a.points)) {
    const pts = resolveArrow(s, boxes);
    const stroke = s.stroke || C.ink;
    const o = opt({ stroke, strokeWidth: 1.8, strokeLineDash: s.dashed ? [7, 7] : undefined });
    body += paths(gen.linearPath(pts, o));
    const heads = s.both ? [[pts[1], pts[0]], [pts[pts.length - 2], pts[pts.length - 1]]] : [[pts[pts.length - 2], pts[pts.length - 1]]];
    for (const [[x1, y1], [x2, y2]] of heads) {
      const ang = Math.atan2(y2 - y1, x2 - x1), L = 14, W = 0.45;
      body += paths(gen.linearPath([[x2 - L * Math.cos(ang - W), y2 - L * Math.sin(ang - W)], [x2, y2],
        [x2 - L * Math.cos(ang + W), y2 - L * Math.sin(ang + W)]], opt({ stroke, strokeWidth: 1.8 })));
    }
    if (s.text) {
      const mid = s.textAt || [(pts[0][0] + pts[pts.length - 1][0]) / 2, (pts[0][1] + pts[pts.length - 1][1]) / 2];
      body += textSvg(s.text.split('\n'), mid[0], mid[1], { size: 15, color: C.grey });
    }
  }

  const font = FONT
    ? `@font-face{font-family:Virgil;src:url(data:font/woff2;base64,${FONT}) format("woff2");}`
    : '';
  return `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 ${d.width} ${d.height}" width="${d.width}" height="${d.height}">` +
    `<style>${font}text{font-family:Virgil,"Segoe Print","Comic Sans MS",cursive;}</style>` +
    `<rect width="100%" height="100%" fill="${C.white}"/>${body}</svg>\n`;
}

// -------------------------------------------------------------- excalidraw
function toExcalidraw(d) {
  let n = 0;
  const id = (p) => `${p}-${++n}`;
  const base = (o) => ({
    angle: 0, strokeColor: C.ink, backgroundColor: 'transparent', fillStyle: 'hachure',
    strokeWidth: 2, strokeStyle: 'solid', roughness: 1, opacity: 100, groupIds: [], frameId: null,
    roundness: null, seed: 1000 + n, version: 1, versionNonce: 2000 + n, isDeleted: false,
    boundElements: [], updated: 1, link: null, locked: false, ...o,
  });
  const text = (t, x, y, size, color, containerId, align = 'center') => {
    const lines = t.split('\n');
    const w = Math.max(...lines.map((l) => l.length)) * size * 0.55, h = lines.length * size * 1.25;
    return base({
      id: id('text'), type: 'text', x: align === 'left' ? x : x - w / 2, y: y - h / 2, width: w, height: h,
      strokeColor: color, text: t, originalText: t, fontSize: size, fontFamily: 1, textAlign: align,
      verticalAlign: 'middle', containerId, lineHeight: 1.25, baseline: size, autoResize: true,
    });
  };

  const els = [];
  const boxes = {};
  for (const s of d.shapes) {
    if (s.box) {
      const r = base({
        id: id('box'), type: 'rectangle', x: s.x, y: s.y, width: s.w, height: s.h,
        strokeColor: s.stroke || C.ink, backgroundColor: s.fill || 'transparent',
        fillStyle: s.fillStyle || 'hachure', strokeStyle: s.dashed ? 'dashed' : 'solid',
        roundness: { type: 3 },
      });
      boxes[s.box] = { el: r, shape: s };
      els.push(r);
      if (s.text && !s.title) {
        const t = text(s.text, s.x + s.w / 2, s.y + s.h / 2, s.size || 18, C.ink, r.id);
        r.boundElements.push({ type: 'text', id: t.id });
        els.push(t);
      } else if (s.text) {
        els.push(text(s.text, s.x + s.w / 2, s.y + s.h / 2 + 12, s.size || 18, C.ink, null));
      }
      if (s.title) {
        const t = text(s.title, s.titleX ?? s.x + 14, s.y + 22, s.titleSize || 22, s.stroke || C.ink, null, 'left');
        if (s.titleAlign === 'end' && s.titleX === undefined) { t.x = s.x + s.w - 14 - t.width; t.textAlign = 'right'; }
        els.push(t);
      }
    } else if (s.label) {
      els.push(text(s.label, s.x, s.y, s.size || 18, s.color || C.ink, null, s.align === 'start' ? 'left' : 'center'));
    }
  }
  const shapes = Object.fromEntries(Object.entries(boxes).map(([k, v]) => [k, v.shape]));
  for (const s of d.shapes.filter((a) => a.arrow || a.points)) {
    const pts = resolveArrow(s, shapes);
    const [x0, y0] = pts[0];
    const rel = pts.map(([x, y]) => [x - x0, y - y0]);
    const xs = rel.map((p) => p[0]), ys = rel.map((p) => p[1]);
    const a = base({
      id: id('arrow'), type: 'arrow', x: x0, y: y0,
      width: Math.max(...xs) - Math.min(...xs), height: Math.max(...ys) - Math.min(...ys),
      strokeColor: s.stroke || C.ink, strokeStyle: s.dashed ? 'dashed' : 'solid', roundness: { type: 2 },
      points: rel, lastCommittedPoint: null, startArrowhead: s.both ? 'arrow' : null, endArrowhead: 'arrow',
      startBinding: null, endBinding: null,
    });
    if (s.arrow) {
      const [f, t] = s.arrow.map((k) => boxes[k].el);
      a.startBinding = { elementId: f.id, focus: 0, gap: 4 };
      a.endBinding = { elementId: t.id, focus: 0, gap: 4 };
      f.boundElements.push({ type: 'arrow', id: a.id });
      t.boundElements.push({ type: 'arrow', id: a.id });
    }
    els.push(a);
    if (s.text) {
      const mid = s.textAt || [(pts[0][0] + pts[pts.length - 1][0]) / 2, (pts[0][1] + pts[pts.length - 1][1]) / 2];
      els.push(text(s.text, mid[0], mid[1], 15, C.grey, null));
    }
  }
  return JSON.stringify({
    type: 'excalidraw', version: 2, source: 'https://github.com/TomDatalab/cube',
    elements: els, appState: { viewBackgroundColor: C.white, gridSize: null }, files: {},
  }, null, 2) + '\n';
}

// ---------------------------------------------------------------- diagrams
const diagrams = {};

// 1. Before and after -------------------------------------------------------
{
  const L = 40, R = 760, W = 620;
  const node = (id, y, text, stroke, fill, x = L + 30, w = W - 60, h = 58) =>
    ({ box: id, x, y, w, h, text, stroke, fill });
  diagrams['before-after'] = {
    width: 1420, height: 900,
    shapes: [
      { label: 'Before: Node.js backend', x: L + W / 2, y: 40, size: 30, color: C.red },
      { label: 'After: one Rust binary', x: R + W / 2, y: 40, size: 30, color: C.green },

      { box: 'node', x: L, y: 80, w: W, h: 700, stroke: C.red, dashed: true, title: 'node process (+ Neon native addon)' },
      node('gw', 130, 'Express API gateway (TypeScript)\nREST · GraphQL · WebSocket', C.red, C.redBg),
      node('core', 218, 'server-core: cube.js config, JS hooks', C.red, C.redBg),
      node('compiler', 306, 'schema compiler (JavaScript)\n.js / .py / .yml models', C.red, C.redBg),
      node('neon', 394, 'Neon bridge  JS ⇄ Rust', C.orange, C.orangeBg, L + 30, 340),
      node('tess', 482, 'Tesseract planner (Rust)', C.orange, C.orangeBg, L + 30, 280),
      node('cubesql', 482, 'SQL API cubesql (Rust)', C.orange, C.orangeBg, L + 330, 260),
      node('orch', 570, 'query orchestrator (TypeScript)\ncache · queue · pre-aggregations', C.red, C.redBg),
      node('drivers', 672, '33 driver packages on npm\n(JDBC drivers need a JVM)', C.red, C.redBg, L + 30, W - 60, 80),

      { box: 'rust', x: R, y: 80, w: W, h: 700, stroke: C.green, title: 'cube-server  (no Node.js)' },
      node('api', 130, 'axum: REST · GraphQL · WebSocket · Playground', C.blue, C.blueBg, R + 30),
      node('sqlapi', 218, 'SQL API (cubesql) on Rust services', C.blue, C.blueBg, R + 30),
      node('model', 306, 'cubemodel: YAML + Jinja → meta', C.violet, C.violetBg, R + 30),
      node('planner', 394, 'cubeplanner + Tesseract: 24 dialects', C.violet, C.violetBg, R + 30),
      node('rorch', 482, 'cubeorch · cubequeue · cubecache', C.teal, C.tealBg, R + 30),
      node('rcfg', 570, 'cubeconfig: CUBEJS_* env + cube.yml', C.teal, C.tealBg, R + 30),
      node('rdrivers', 672, 'cubedriver: 27 native drivers\n(no JVM, no Instant Client)', C.green, C.greenBg, R + 30, W - 60, 80),

      { arrow: ['gw', 'core'] }, { arrow: ['core', 'compiler'] }, { arrow: ['compiler', 'neon'] },
      { arrow: ['neon', 'tess'] }, { arrow: ['tess', 'orch'] }, { arrow: ['orch', 'drivers'] },
      { arrow: ['cubesql', 'neon'], text: 'TransportService\ncalls back into Node', textAt: [L + 500, 420] },

      { arrow: ['api', 'sqlapi'] }, { arrow: ['sqlapi', 'model'] }, { arrow: ['model', 'planner'] },
      { arrow: ['planner', 'rorch'] }, { arrow: ['rorch', 'rcfg'] }, { arrow: ['rcfg', 'rdrivers'] },

      { points: [[L + W + 20, 430], [R - 20, 430]], text: 'rewritten', textAt: [(L + W + R) / 2, 405], stroke: C.green },

      { label: '~90k lines of TypeScript/JavaScript + 33 npm driver packages', x: L + W / 2, y: 820, size: 18, color: C.grey },
      { label: '~107k lines of Rust in 12 crates · 1,334 tests · one ~70 MB image', x: R + W / 2, y: 820, size: 18, color: C.grey },
      { label: 'The same CUBEJS_* variables, YAML models and HTTP contract on both sides', x: 710, y: 868, size: 20 },
    ],
  };
}

// 2. Inside cube-server ----------------------------------------------------
{
  const b = (id, x, y, w, h, text, stroke, fill, size) => ({ box: id, x, y, w, h, text, stroke, fill, size });
  diagrams['architecture'] = {
    width: 1500, height: 1080,
    shapes: [
      { label: 'Inside cube-server', x: 750, y: 36, size: 32 },

      b('bi', 60, 80, 260, 70, 'BI tools · psql\n(Postgres wire)', C.ink, C.white),
      b('apps', 400, 80, 300, 70, 'Apps · @cubejs-client\nREST · WebSocket', C.ink, C.white),
      b('gql', 780, 80, 280, 70, 'GraphQL clients', C.ink, C.white),
      b('browser', 1140, 80, 300, 70, 'Browser: Playground\n& Vizard (static files)', C.ink, C.white),

      { box: 'bin', x: 30, y: 190, w: 1440, h: 750, stroke: C.green, title: 'cube-server  (single process, tokio)', titleX: 250, titleSize: 20 },

      b('sql', 60, 250, 300, 80, 'SQL API\ncubesql + cubesqlbridge', C.blue, C.blueBg),
      b('http', 400, 250, 640, 80, 'axum router: /cube/v1/load · sql · dry-run · meta\nsubscribe · /cube/ws · /cube/graphql (cubegraphql)', C.blue, C.blueBg),
      b('static', 1080, 250, 360, 80, 'Playground assets\n/playground/* helper API', C.blue, C.blueBg),

      b('auth', 400, 370, 300, 70, 'cubeauth\nJWT · JWK · API scopes', C.violet, C.violetBg),
      b('tenants', 740, 370, 300, 70, 'tenant registry\n(cube.yml tenants)', C.violet, C.violetBg),
      b('query', 400, 480, 300, 70, 'cubequery\nnormalize · validate', C.violet, C.violetBg),
      b('model', 740, 480, 300, 70, 'cubemodel\nYAML + Jinja → meta', C.violet, C.violetBg),

      b('planner', 400, 590, 640, 80, 'cubeplanner over Tesseract (cubesqlplanner)\n24 SQL dialects, chosen per data source', C.orange, C.orangeBg),

      b('orch', 400, 710, 300, 90, 'cubeorch\nquery cache · refresh keys\npre-aggregations', C.teal, C.tealBg),
      b('queue', 740, 710, 300, 90, 'cubequeue + cubecache\nqueue · "Continue wait"\nstreams', C.teal, C.tealBg),

      b('driver', 60, 840, 980, 76, 'cubedriver: Driver trait + 27 drivers (one Cargo feature each)', C.green, C.greenBg),
      b('config', 1080, 370, 360, 110, 'cubeconfig\nCUBEJS_* environment\n+ cube.yml (data sources,\ntenants, API, refresh)', C.ink, C.white),
      b('reload', 1080, 520, 360, 70, 'model reload\n(watch model/, hot swap)', C.ink, C.white),

      b('dbs', 60, 985, 980, 70, 'Postgres · Snowflake · BigQuery · Databricks · ClickHouse · Trino · DuckDB · Oracle · …', C.ink, C.white, 17),
      b('cubestore', 1080, 985, 360, 70, 'Cube Store\n(pre-aggregations)', C.ink, C.white),

      { arrow: ['bi', 'sql'] }, { arrow: ['apps', 'http'] }, { arrow: ['gql', 'http'] }, { arrow: ['browser', 'static'] },
      { arrow: ['http', 'auth'] }, { arrow: ['auth', 'query'] }, { arrow: ['auth', 'tenants'] },
      { arrow: ['tenants', 'model'] }, { arrow: ['query', 'planner'] }, { arrow: ['model', 'planner'] },
      { arrow: ['planner', 'orch'] }, { arrow: ['orch', 'queue'], both: true },
      { points: [[210, 330], [210, 630], [400, 630]], text: 'SQL → REST query:\nsame planner and\norchestrator', textAt: [305, 480] },
      { points: [[550, 800], [550, 840]] },
      { points: [[550, 916], [550, 985]] },
      { points: [[1040, 878], [1260, 878], [1260, 985]], text: 'pre-aggregations via\nthe Cube Store driver', textAt: [1300, 820], dashed: true },
      { points: [[1260, 480], [1260, 520]] },
      { points: [[1080, 555], [1040, 515]] },
    ],
  };
}

// 3. Migration roadmap -----------------------------------------------------
{
  const steps = [
    ['1', 'Foundations', 'server · auth · query\nmodel · Postgres'],
    ['2', 'Planner', 'Tesseract without JS\n/v1/sql · dry-run'],
    ['3', 'Orchestrator', 'cache · queue\n/v1/load'],
    ['4', 'Pre-aggs', 'loader · partitions\nrefresh keys'],
    ['5', 'SQL API', 'cubesql on Rust\nservices'],
    ['6', 'Drivers', '27 native drivers\n24 dialects'],
    ['7', 'Remaining API', 'WebSocket · GraphQL\nPlayground'],
    ['8', 'Delete Node.js', 'remove the\n@cubejs-backend/*\npackages'],
  ];
  const shapes = [{ label: 'Strangler-fig migration: one surface at a time, same HTTP contract', x: 870, y: 40, size: 28 }];
  steps.forEach(([n, title, text], i) => {
    const x = 40 + i * 212, done = i < 7;
    shapes.push({ box: `s${i}`, x, y: 110, w: 188, h: 150, title: `${n}. ${title}`, titleSize: 18, text,
      stroke: done ? C.green : C.orange, fill: done ? C.greenBg : C.orangeBg, size: 15 });
    shapes.push({ label: done ? '✓ done' : 'next', x: x + 94, y: 290, size: 18, color: done ? C.green : C.orange });
    if (i > 0) shapes.push({ arrow: [`s${i - 1}`, `s${i}`] });
  });
  shapes.push({ label: 'Each step: port the Node.js code and its tests, keep paths, bodies, status codes and CUBEJS_* names,\n' +
    'fail with a named error where something is not supported yet, never degrade silently.', x: 870, y: 360, size: 18, color: C.grey });
  diagrams['migration-roadmap'] = { width: 1740, height: 420, shapes };
}

for (const [name, d] of Object.entries(diagrams)) {
  fs.writeFileSync(path.join(OUT, `${name}.svg`), toSvg(d));
  fs.writeFileSync(path.join(OUT, `${name}.excalidraw`), toExcalidraw(d));
  console.log(`${name}: svg + excalidraw`);
}
