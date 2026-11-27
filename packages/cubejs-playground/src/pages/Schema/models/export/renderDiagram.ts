import type { Rect } from './pages';

// What an export leaves out: the minimap, the zoom controls, the handles and
// the page-break overlay.
const LEFT_OUT = ['react-flow__minimap', 'react-flow__controls', 'react-flow__handle'];
const PAGE_BREAKS = 'model-page-breaks';

function kept(node: HTMLElement): boolean {
  const { classList, dataset } = node;
  if (!classList) {
    // Text and other nodes without classes.
    return true;
  }
  return !LEFT_OUT.some((c) => classList.contains(c)) && dataset?.slot !== PAGE_BREAKS;
}

// A token of the theme as the browser resolves it.
export function token(name: string): string {
  return getComputedStyle(document.documentElement).getPropertyValue(name).trim();
}

// The diagram within `bounds`, at one canvas unit per CSS pixel, as a data
// URL. `viewportEl` is React Flow's viewport or an element around it.
export async function renderDiagram(
  viewportEl: HTMLElement,
  bounds: Rect,
  format: 'png' | 'svg',
  pixelRatio = 1
): Promise<string> {
  const { toPng, toSvg } = await import('html-to-image');
  const viewport = viewportEl.classList.contains('react-flow__viewport')
    ? viewportEl
    : viewportEl.querySelector<HTMLElement>('.react-flow__viewport') ?? viewportEl;
  const options = {
    backgroundColor: token('--background'),
    width: bounds.width,
    height: bounds.height,
    pixelRatio,
    style: {
      width: `${bounds.width}px`,
      height: `${bounds.height}px`,
      transform: `translate(${-bounds.x}px, ${-bounds.y}px) scale(1)`,
    },
    filter: kept,
  };
  return format === 'svg' ? toSvg(viewport, options) : toPng(viewport, options);
}
