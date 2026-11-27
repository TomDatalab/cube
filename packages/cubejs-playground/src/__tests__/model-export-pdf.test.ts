// @vitest-environment jsdom
import {
  afterEach, beforeEach, describe, expect, it, vi,
} from 'vitest';

import { exportPdf } from '../pages/Schema/models/export/exportPdf';
import { mmToPx } from '../pages/Schema/models/export/pages';

// jsdom has no canvas nor image decoding: html-to-image, jsPDF, the 2D
// context and Image are stand-ins whose calls the tests read.
const fake = vi.hoisted(() => ({
  toPng: null as any,
  natural: { width: 0, height: 0 },
  pdf: null as any,
  drawImage: null as any,
}));

vi.mock('html-to-image', () => ({
  toPng: (...args: unknown[]) => fake.toPng(...args),
  toSvg: vi.fn(),
}));

vi.mock('jspdf', () => ({
  // A constructor: `new jsPDF(options)` gets this record; an arrow cannot be
  // called with `new`.
  // eslint-disable-next-line prefer-arrow-callback
  jsPDF: vi.fn(function Pdf(options: unknown) {
    fake.pdf = {
      options, addPage: vi.fn(), addImage: vi.fn(), save: vi.fn(),
    };
    return fake.pdf;
  }),
}));

class FakeImage {
  onload: (() => void) | null = null;

  onerror: (() => void) | null = null;

  naturalWidth = 0;

  naturalHeight = 0;

  set src(_value: string) {
    this.naturalWidth = fake.natural.width;
    this.naturalHeight = fake.natural.height;
    setTimeout(() => this.onload?.(), 0);
  }
}

describe('exportPdf slicing', () => {
  const original = { Image: globalThis.Image, getContext: HTMLCanvasElement.prototype.getContext };

  beforeEach(() => {
    fake.toPng = vi.fn(async () => 'data:image/png;base64,AAAA');
    fake.drawImage = vi.fn();
    (globalThis as any).Image = FakeImage;
    HTMLCanvasElement.prototype.getContext = vi.fn(() => ({
      fillStyle: '',
      fillRect: vi.fn(),
      drawImage: fake.drawImage,
    })) as any;
  });

  afterEach(() => {
    (globalThis as any).Image = original.Image;
    HTMLCanvasElement.prototype.getContext = original.getContext;
  });

  const viewport = () => {
    const el = document.createElement('div');
    el.className = 'react-flow__viewport';
    return el;
  };
  // A4 landscape at 10 mm: each page's printable area in canvas units.
  const w = mmToPx(277);
  const h = mmToPx(190);
  const bounds = {
    x: 100, y: 50, width: 1500, height: 800,
  };

  it('slices a normal diagram at pixel ratio 2', async () => {
    fake.natural = { width: 3000, height: 1600 };
    await exportPdf(viewport(), bounds, undefined, 'Sales.pdf');
    expect(fake.toPng.mock.calls[0][1].pixelRatio).toBe(2);
    const sources = fake.drawImage.mock.calls.map((c: number[]) => c.slice(1, 5));
    const cw = Math.ceil(w * 2);
    const ch = Math.ceil(h * 2);
    expect(sources).toEqual([
      [0, 0, cw, ch],
      [w * 2, 0, cw, ch],
      [0, h * 2, cw, ch],
      [w * 2, h * 2, cw, ch],
    ]);
    expect(fake.pdf.addPage).toHaveBeenCalledTimes(3);
    expect(fake.pdf.addImage).toHaveBeenCalledTimes(4);
    expect(fake.pdf.addImage.mock.calls[0].slice(1)).toEqual(['PNG', 10, 10, 277, 190]);
    expect(fake.pdf.save).toHaveBeenCalledWith('Sales.pdf');
  });

  it('follows an image the renderer scaled down', async () => {
    // Half of bounds × 2: one image pixel per canvas unit.
    fake.natural = { width: 1500, height: 800 };
    await exportPdf(viewport(), bounds, undefined, 'Sales.pdf');
    const sources = fake.drawImage.mock.calls.map((c: number[]) => c.slice(1, 5));
    const cw = Math.ceil(w);
    const ch = Math.ceil(h);
    expect(sources).toEqual([
      [0, 0, cw, ch],
      [w, 0, cw, ch],
      [0, h, cw, ch],
      [w, h, cw, ch],
    ]);
    expect(fake.pdf.addPage).toHaveBeenCalledTimes(3);
    expect(fake.pdf.save).toHaveBeenCalledWith('Sales.pdf');
  });

  it('asks for less than 16384 px a side on a huge diagram', async () => {
    const huge = {
      x: 0, y: 0, width: 20000, height: 500,
    };
    fake.natural = { width: 16384, height: 409.6 };
    await exportPdf(viewport(), huge, { margin: 0 }, 'Big.pdf');
    expect(fake.toPng.mock.calls[0][1].pixelRatio).toBeCloseTo(16384 / 20000, 10);
    const pages = Math.ceil(20000 / mmToPx(297));
    expect(fake.drawImage).toHaveBeenCalledTimes(pages);
    const last = fake.drawImage.mock.calls[pages - 1];
    expect(last[1]).toBeCloseTo((pages - 1) * mmToPx(297) * (16384 / 20000), 6);
    expect(fake.pdf.addPage).toHaveBeenCalledTimes(pages - 1);
    expect(fake.pdf.save).toHaveBeenCalledWith('Big.pdf');
  });
});
