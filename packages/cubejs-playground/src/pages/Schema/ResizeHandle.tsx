import { KeyboardEvent, PointerEvent } from 'react';

const STEP = 16;

// The panel's right edge: drag it, or focus it and use the arrow keys, to
// change the panel's width. Its parent must be positioned.
export function ResizeHandle({
  width, min, max, label, onResize,
}: {
  width: number;
  min: number;
  max: number;
  label: string;
  onResize: (width: number) => void;
}) {
  const onPointerDown = (e: PointerEvent<HTMLDivElement>) => {
    e.preventDefault();
    const startX = e.clientX;
    const startWidth = width;
    const handle = e.currentTarget;
    handle.setPointerCapture(e.pointerId);
    const previous = document.body.style.cursor;
    document.body.style.cursor = 'col-resize';
    const move = (ev: globalThis.PointerEvent) => onResize(startWidth + ev.clientX - startX);
    const up = () => {
      document.body.style.cursor = previous;
      handle.removeEventListener('pointermove', move);
      handle.removeEventListener('pointerup', up);
      handle.removeEventListener('pointercancel', up);
    };
    handle.addEventListener('pointermove', move);
    handle.addEventListener('pointerup', up);
    handle.addEventListener('pointercancel', up);
  };

  const onKeyDown = (e: KeyboardEvent<HTMLDivElement>) => {
    const step = { ArrowLeft: -STEP, ArrowRight: STEP }[e.key];
    if (step) {
      e.preventDefault();
      onResize(width + step);
    } else if (e.key === 'Home') {
      e.preventDefault();
      onResize(min);
    } else if (e.key === 'End') {
      e.preventDefault();
      onResize(max);
    }
  };

  return (
    <div
      role="separator"
      tabIndex={0}
      data-slot="resize-handle"
      aria-label={`Resize ${label}`}
      aria-orientation="vertical"
      aria-valuenow={width}
      aria-valuemin={min}
      aria-valuemax={max}
      title="Drag to resize"
      className="tw:absolute tw:top-0 tw:-right-1 tw:z-10 tw:h-full tw:w-2 tw:cursor-col-resize tw:touch-none tw:outline-none tw:after:absolute tw:after:inset-y-0 tw:after:left-1/2 tw:after:w-0.5 tw:after:-translate-x-1/2 tw:after:bg-transparent tw:after:transition-colors tw:hover:after:bg-primary tw:focus-visible:after:bg-primary"
      onPointerDown={onPointerDown}
      onKeyDown={onKeyDown}
    />
  );
}
