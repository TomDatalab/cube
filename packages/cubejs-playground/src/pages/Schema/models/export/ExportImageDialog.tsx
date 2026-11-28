import { useState } from 'react';
import type { Node } from '@xyflow/react';

import { Button } from '@/components/ui/button';
import {
  Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle,
} from '@/components/ui/dialog';
import { Label } from '@/components/ui/label';
import { RadioGroup, RadioGroupItem } from '@/components/ui/radio-group';
import { Spinner, notify } from '@/components/kit';

import { download } from './download';
import { diagramBounds } from './pages';
import { renderDiagram } from './renderDiagram';

type Format = 'png' | 'svg';
type Scope = 'all' | 'selection';

export type ExportImageDialogProps = {
  open: boolean;
  // The model's name: the file is `{name}.png` or `{name}.svg`.
  name: string;
  nodes: Node[];
  // The canvas, around React Flow's viewport.
  target(): HTMLElement | null;
  onClose(): void;
};

function Choice({
  id, value, label, disabled,
}: { id: string; value: string; label: string; disabled?: boolean }) {
  return (
    <div className="tw:flex tw:items-center tw:gap-2">
      <RadioGroupItem id={id} value={value} aria-label={label} disabled={disabled} />
      <Label htmlFor={id} className={disabled ? 'tw:opacity-50' : undefined}>{label}</Label>
    </div>
  );
}

// Redgate's Export model to image: PNG or SVG, of the whole diagram or the
// selection. It changes nothing.
export function ExportImageDialog({
  open, name, nodes, target, onClose,
}: ExportImageDialogProps) {
  const [format, setFormat] = useState<Format>('png');
  const [scope, setScope] = useState<Scope>('all');
  const [running, setRunning] = useState(false);
  const hasSelection = nodes.some((n) => n.selected);
  const shownScope = hasSelection ? scope : 'all';

  const run = async () => {
    const el = target();
    const bounds = diagramBounds(nodes, shownScope);
    if (!el || !bounds) {
      return;
    }
    setRunning(true);
    try {
      download(await renderDiagram(el, bounds, format), `${name || 'model'}.${format}`);
      onClose();
    } catch (e: any) {
      notify.error(e?.message ?? 'The image could not be made');
    } finally {
      setRunning(false);
    }
  };

  return (
    <Dialog open={open} onOpenChange={(next) => !next && onClose()}>
      <DialogContent className="tw:sm:max-w-md">
        <DialogHeader>
          <DialogTitle>Export model to image</DialogTitle>
          <DialogDescription>The diagram as it shows on the canvas, without the minimap.</DialogDescription>
        </DialogHeader>
        <div className="tw:grid tw:grid-cols-2 tw:gap-4">
          <div className="tw:flex tw:flex-col tw:gap-2">
            <span className="tw:text-sm tw:font-semibold">Format</span>
            <RadioGroup value={format} onValueChange={(v) => setFormat(v as Format)}>
              <Choice id="export-image-png" value="png" label="PNG" />
              <Choice id="export-image-svg" value="svg" label="SVG" />
            </RadioGroup>
          </div>
          <div className="tw:flex tw:flex-col tw:gap-2">
            <span className="tw:text-sm tw:font-semibold">Scope</span>
            <RadioGroup value={shownScope} onValueChange={(v) => setScope(v as Scope)}>
              <Choice id="export-image-all" value="all" label="Whole diagram" />
              <Choice id="export-image-selection" value="selection" label="Selection" disabled={!hasSelection} />
            </RadioGroup>
          </div>
        </div>
        <DialogFooter>
          <Button variant="outline" onClick={onClose}>Cancel</Button>
          <Button disabled={running} onClick={run}>
            {running ? <Spinner className="tw:text-current" /> : null}
            Export
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
