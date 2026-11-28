// The @font-face rules an export embeds: only the faces its text is drawn
// with, each in one format (woff2 when the face has it). Left to itself,
// html-to-image embeds every face of the page in every format, megabytes of
// fonts for a diagram of two tables.

const FONT_FACE_RULE = 5;

const MIME: Record<string, string> = {
  woff2: 'font/woff2',
  woff: 'font/woff',
  truetype: 'font/ttf',
  opentype: 'font/otf',
};

type Face = {
  // As the rule names it, and lowercased to compare.
  name: string;
  family: string;
  style: string;
  // The weights it covers: one, or a range.
  weights: [number, number];
  unicodeRange: string;
  src: string;
  base: string;
};

type Use = { family: string[]; weight: number; style: string };

const unquote = (name: string) => name.trim().replace(/^['"]|['"]$/g, '').toLowerCase();

function weightOf(value: string): number {
  if (value === 'bold') {
    return 700;
  }
  const n = Number.parseInt(value, 10);
  return Number.isNaN(n) ? 400 : n;
}

function rulesOf(list: CSSRuleList | undefined, out: CSSRule[]) {
  Array.from(list ?? []).forEach((rule) => {
    out.push(rule);
    const nested = rule as CSSRule & { cssRules?: CSSRuleList; styleSheet?: CSSStyleSheet | null };
    if (nested.styleSheet) {
      rulesOf(readable(nested.styleSheet), out);
    } else if (nested.cssRules) {
      rulesOf(nested.cssRules, out);
    }
  });
}

// A sheet of another origin cannot be read; its fonts are not embedded.
function readable(sheet: CSSStyleSheet): CSSRuleList | undefined {
  try {
    return sheet.cssRules;
  } catch {
    return undefined;
  }
}

function facesOf(doc: Document): Face[] {
  const rules: CSSRule[] = [];
  Array.from(doc.styleSheets).forEach((sheet) => rulesOf(readable(sheet), rules));
  return rules.filter((r) => r.type === FONT_FACE_RULE).map((r) => {
    const { style } = r as CSSFontFaceRule;
    const weights = (style.getPropertyValue('font-weight') || '400').trim().split(/\s+/).map(weightOf);
    const name = style.getPropertyValue('font-family').trim().replace(/^['"]|['"]$/g, '');
    return {
      name,
      family: name.toLowerCase(),
      style: (style.getPropertyValue('font-style') || 'normal').trim(),
      weights: [weights[0], weights[1] ?? weights[0]] as [number, number],
      unicodeRange: style.getPropertyValue('unicode-range').trim(),
      src: style.getPropertyValue('src'),
      base: r.parentStyleSheet?.href ?? doc.baseURI,
    };
  });
}

// The family, weight and style of every piece of text under `root`.
function usesOf(root: HTMLElement, kept: (node: HTMLElement) => boolean): Use[] {
  const found = new Map<string, Use>();
  const add = (el: Element) => {
    let at: Element | null = el;
    while (at && at !== root.parentElement) {
      if (at instanceof HTMLElement && !kept(at)) {
        return;
      }
      at = at.parentElement;
    }
    const css = getComputedStyle(el);
    const use = {
      family: css.fontFamily.split(',').map(unquote).filter(Boolean),
      weight: weightOf(css.fontWeight || '400'),
      style: css.fontStyle || 'normal',
    };
    found.set(`${use.family.join(',')}|${use.weight}|${use.style}`, use);
  };
  const walker = root.ownerDocument.createTreeWalker(root, NodeFilter.SHOW_TEXT);
  for (let node = walker.nextNode(); node; node = walker.nextNode()) {
    if (node.textContent?.trim() && node.parentElement) {
      add(node.parentElement);
    }
  }
  root.querySelectorAll('input, textarea').forEach(add);
  return [...found.values()];
}

// The face the browser draws `use` with: of the first family that has
// faces, the one of its style closest in weight.
function faceFor(use: Use, faces: Face[]): Face | null {
  const family = use.family.find((name) => faces.some((f) => f.family === name));
  if (!family) {
    return null;
  }
  const ofFamily = faces.filter((f) => f.family === family);
  const ofStyle = ofFamily.filter((f) => f.style === use.style);
  const pool = ofStyle.length ? ofStyle : ofFamily;
  const distance = (f: Face) => {
    const [lo, hi] = f.weights;
    if (use.weight < lo) {
      return lo - use.weight;
    }
    return use.weight > hi ? use.weight - hi : 0;
  };
  return pool.reduce((best, f) => (distance(f) < distance(best) ? f : best));
}

// The face's source in one format: woff2 when it has one, else its first.
function sourceOf(face: Face): { url: string; format: string } | null {
  const sources = [...face.src.matchAll(/url\(\s*(['"]?)([^'")]+)\1\s*\)\s*(?:format\(\s*['"]?([^'")]+)['"]?\s*\))?/g)]
    .map((m) => ({ url: m[2], format: (m[3] ?? '').toLowerCase() }));
  const chosen = sources.find((s) => s.format === 'woff2') ?? sources[0];
  if (!chosen) {
    return null;
  }
  return { url: new URL(chosen.url, face.base).href, format: chosen.format || 'woff2' };
}

function base64(bytes: Uint8Array): string {
  let text = '';
  const CHUNK = 0x8000;
  for (let i = 0; i < bytes.length; i += CHUNK) {
    text += String.fromCharCode(...bytes.subarray(i, i + CHUNK));
  }
  return btoa(text);
}

async function embedded(face: Face): Promise<string | null> {
  const source = sourceOf(face);
  if (!source) {
    return null;
  }
  if (source.url.startsWith('data:')) {
    return source.url;
  }
  try {
    const res = await fetch(source.url);
    if (!res.ok) {
      return null;
    }
    const bytes = new Uint8Array(await res.arrayBuffer());
    return `data:${MIME[source.format] ?? 'font/woff2'};base64,${base64(bytes)}`;
  } catch {
    // The text falls back to another font rather than the export failing.
    return null;
  }
}

// The CSS for html-to-image's `fontEmbedCSS`: empty when the text uses no
// web font.
export async function fontEmbedCSS(root: HTMLElement, kept: (node: HTMLElement) => boolean): Promise<string> {
  const faces = facesOf(root.ownerDocument);
  if (!faces.length) {
    return '';
  }
  const needed = new Set<Face>();
  usesOf(root, kept).forEach((use) => {
    const face = faceFor(use, faces);
    if (face) {
      needed.add(face);
    }
  });
  const rules = await Promise.all([...needed].map(async (face) => {
    const url = await embedded(face);
    if (!url) {
      return null;
    }
    const { format } = sourceOf(face)!;
    const weight = face.weights[0] === face.weights[1] ? `${face.weights[0]}` : face.weights.join(' ');
    return [
      '@font-face {',
      ` font-family: '${face.name.replace(/'/g, "\\'")}';`,
      ` font-style: ${face.style};`,
      ` font-weight: ${weight};`,
      face.unicodeRange ? ` unicode-range: ${face.unicodeRange};` : '',
      ` src: url(${url}) format('${format}');`,
      ' }',
    ].join('');
  }));
  return rules.filter(Boolean).join('\n');
}
