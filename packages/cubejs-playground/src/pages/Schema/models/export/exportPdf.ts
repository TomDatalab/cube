import {
  PrintSettings, Rect, marginMm, pageGrid, pageSizeMm,
} from './pages';
import { renderDiagram, token } from './renderDiagram';

const RATIO = 2;
// html-to-image scales a canvas down past 16384 px a side; asking for less
// keeps big diagrams sharp and whole.
const MAX_SIDE = 16384;

function loadImage(src: string): Promise<HTMLImageElement> {
  return new Promise((resolve, reject) => {
    const image = new Image();
    image.onload = () => resolve(image);
    image.onerror = () => reject(new Error('The diagram could not be drawn'));
    image.src = src;
  });
}

// The diagram as a PDF of the model's page size and orientation: one PNG,
// cut into the pages of `pageGrid`, each placed at the margin.
export async function exportPdf(
  viewportEl: HTMLElement,
  bounds: Rect,
  print: PrintSettings | undefined,
  fileName: string
): Promise<void> {
  const ratio = Math.min(RATIO, MAX_SIDE / Math.max(bounds.width, bounds.height));
  const [{ jsPDF: Pdf }, png] = await Promise.all([
    import('jspdf'),
    renderDiagram(viewportEl, bounds, 'png', ratio),
  ]);
  const image = await loadImage(png);
  // Image pixels per canvas unit, from the image itself: the renderer may
  // have scaled it down.
  const sx = image.naturalWidth / bounds.width;
  const sy = image.naturalHeight / bounds.height;
  const page = pageSizeMm(print);
  const margin = marginMm(print);
  const orientation = page.width > page.height ? 'landscape' : 'portrait';
  const format = [page.width, page.height];
  const pdf = new Pdf({ orientation, unit: 'mm', format });
  const background = token('--background');
  pageGrid(bounds, print).forEach((slice, i) => {
    const canvas = document.createElement('canvas');
    canvas.width = Math.ceil(slice.width * sx);
    canvas.height = Math.ceil(slice.height * sy);
    const context = canvas.getContext('2d');
    if (!context) {
      throw new Error('The diagram could not be drawn');
    }
    context.fillStyle = background;
    context.fillRect(0, 0, canvas.width, canvas.height);
    context.drawImage(
      image,
      (slice.x - bounds.x) * sx,
      (slice.y - bounds.y) * sy,
      canvas.width,
      canvas.height,
      0,
      0,
      canvas.width,
      canvas.height
    );
    if (i > 0) {
      pdf.addPage(format, orientation);
    }
    // 'FAST' deflates the PNG: jsPDF stores it raw otherwise.
    pdf.addImage(canvas, 'PNG', margin, margin, page.width - 2 * margin, page.height - 2 * margin, undefined, 'FAST');
  });
  pdf.save(fileName);
}
