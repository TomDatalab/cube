// html-to-image writes every computed property, custom properties included,
// on every element of an SVG: about 26 KB per element. This keeps what
// changes the drawing:
// - no custom property: every value already has them resolved;
// - an inherited property is left out where the parent has the same value;
// - any property is left out where it has its initial value and so has the
//   parent (whether it inherits or not, it comes out the same).
// Only divs and spans lose the last two: the browser's own stylesheet gives
// other elements values of their own (a button's font, a list's margins),
// which neither the parent's nor the initial value would bring back.
// Borders and outlines are always kept: a width computes to 0 without a
// style, so their initial values depend on other properties.

const SVG_PREFIX = 'data:image/svg+xml;charset=utf-8,';

// Properties every browser inherits; a property missing here is only left
// out when it also has its initial value.
const INHERITED = new Set([
  'color', 'font', 'font-family', 'font-size', 'font-style', 'font-weight', 'font-variant', 'font-stretch',
  'font-kerning', 'font-feature-settings', 'font-variation-settings', 'font-optical-sizing', 'font-size-adjust',
  'font-synthesis', 'font-variant-ligatures', 'font-variant-caps', 'font-variant-numeric',
  'font-variant-east-asian', 'font-variant-alternates', 'font-variant-position', 'font-palette',
  'letter-spacing', 'word-spacing', 'line-height', 'text-align', 'text-align-last', 'text-indent',
  'text-transform', 'text-shadow', 'text-rendering', 'white-space', 'white-space-collapse', 'text-wrap',
  'text-wrap-mode', 'text-wrap-style', 'word-break', 'overflow-wrap', 'word-wrap', 'line-break', 'hyphens',
  'tab-size', 'direction', 'writing-mode', 'text-orientation', 'visibility', 'cursor', 'list-style',
  'list-style-type', 'list-style-position', 'list-style-image', 'quotes', 'caret-color', 'border-collapse',
  'border-spacing', 'caption-side', 'empty-cells', 'orphans', 'widows', 'fill', 'fill-opacity', 'fill-rule',
  'stroke', 'stroke-width', 'stroke-opacity', 'stroke-dasharray', 'stroke-dashoffset', 'stroke-linecap',
  'stroke-linejoin', 'stroke-miterlimit', 'paint-order', 'clip-rule', 'shape-rendering', 'image-rendering',
  'text-anchor', 'dominant-baseline', '-webkit-font-smoothing', '-webkit-text-fill-color',
  '-webkit-text-stroke-color', '-webkit-text-stroke-width', 'pointer-events',
]);

const ALWAYS_KEPT = /^(border|outline|column-rule|rule)(-|$)/;
const PLAIN = new Set(['div', 'span']);

// A style attribute's declarations, split on the semicolons outside
// parentheses and quotes (a url() may hold one).
export function declarations(style: string): Map<string, string> {
  const found = new Map<string, string>();
  let depth = 0;
  let quote = '';
  let start = 0;
  const take = (end: number) => {
    const text = style.slice(start, end);
    const colon = text.indexOf(':');
    if (colon > 0) {
      found.set(text.slice(0, colon).trim(), text.slice(colon + 1).trim());
    }
    start = end + 1;
  };
  for (let i = 0; i < style.length; i += 1) {
    const ch = style[i];
    if (quote) {
      if (ch === quote && style[i - 1] !== '\\') {
        quote = '';
      }
    } else if (ch === '"' || ch === "'") {
      quote = ch;
    } else if (ch === '(') {
      depth += 1;
    } else if (ch === ')') {
      depth = Math.max(0, depth - 1);
    } else if (ch === ';' && depth === 0) {
      take(i);
    }
  }
  take(style.length);
  return found;
}

// The initial computed values, from an element with `all: initial` and
// nothing else (a position of its own would change its display too),
// clipped out of sight by a box around it until `done`.
function liveInitial(): { read(name: string): string; done(): void } {
  const box = document.createElement('div');
  box.style.cssText = 'position: absolute; width: 0; height: 0; overflow: hidden';
  const probe = document.createElement('div');
  probe.style.setProperty('all', 'initial');
  box.appendChild(probe);
  document.body.appendChild(box);
  const computed = getComputedStyle(probe);
  const cache = new Map<string, string>();
  return {
    read: (name) => {
      if (!cache.has(name)) {
        cache.set(name, computed.getPropertyValue(name).trim());
      }
      return cache.get(name)!;
    },
    done: () => box.remove(),
  };
}

export function compactSvgMarkup(svg: string, initialOf: (name: string) => string): string {
  const doc = new DOMParser().parseFromString(svg, 'image/svg+xml');
  if (doc.getElementsByTagName('parsererror').length) {
    return svg;
  }
  const own = new Map<Element, Map<string, string>>();
  Array.from(doc.getElementsByTagName('*')).forEach((el) => {
    const style = el.getAttribute('style');
    if (style === null) {
      return;
    }
    const decls = declarations(style);
    own.set(el, decls);
    // The nearest ancestor with a style: what this element inherits from.
    let up = el.parentElement;
    while (up && !own.has(up)) {
      up = up.parentElement;
    }
    const parent = up && PLAIN.has(el.localName) ? own.get(up)! : null;
    const kept: string[] = [];
    decls.forEach((value, name) => {
      if (name.startsWith('--')) {
        return;
      }
      if (parent && !ALWAYS_KEPT.test(name) && parent.get(name) === value
        && (INHERITED.has(name) || initialOf(name) === value)) {
        return;
      }
      kept.push(`${name}: ${value};`);
    });
    el.setAttribute('style', kept.join(' '));
  });
  return new XMLSerializer().serializeToString(doc);
}

// The SVG data URL html-to-image made, with the same drawing and far less
// style text.
export function compactSvg(dataUrl: string, initialOf?: (name: string) => string): string {
  if (!dataUrl.startsWith(SVG_PREFIX)) {
    return dataUrl;
  }
  const live = initialOf ? null : liveInitial();
  try {
    const svg = decodeURIComponent(dataUrl.slice(SVG_PREFIX.length));
    return SVG_PREFIX + encodeURIComponent(compactSvgMarkup(svg, initialOf ?? live!.read));
  } finally {
    live?.done();
  }
}
