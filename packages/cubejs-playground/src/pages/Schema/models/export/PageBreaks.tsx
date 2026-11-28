import { ViewportPortal } from '@xyflow/react';

import type { Rect } from './pages';

// Where the PDF cuts the diagram: each page's printable area as a dashed
// rectangle under the nodes. Exports leave it out.
export function PageBreaks({ pages }: { pages: Rect[] }) {
  return (
    <ViewportPortal>
      <div data-slot="model-page-breaks" className="tw:pointer-events-none" aria-hidden>
        {pages.map((p) => (
          <div
            key={`${p.x}:${p.y}`}
            data-slot="model-page-break"
            className="tw:absolute tw:border tw:border-dashed"
            style={{
              left: p.x,
              top: p.y,
              width: p.width,
              height: p.height,
              borderColor: 'var(--muted-foreground)',
            }}
          />
        ))}
      </div>
    </ViewportPortal>
  );
}
