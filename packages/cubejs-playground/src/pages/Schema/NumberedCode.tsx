import { useMemo, useRef } from 'react';

import PrismCode from '../../PrismCode';

// How many lines a text has: an empty one still shows line 1.
export function lineCount(text: string): number {
  return text.split('\n').length;
}

// The gutter: one number per line, right aligned.
function Gutter({ lines, className }: { lines: number; className?: string }) {
  const numbers = useMemo(() => Array.from({ length: lines }, (_, i) => i + 1).join('\n'), [lines]);
  return (
    <pre aria-hidden="true" data-slot="line-numbers" className={`numbered-code-gutter ${className ?? ''}`}>
      {numbers}
    </pre>
  );
}

// The code beside the gutter, padded like it so line 1 sits next to 1.
const CODE_STYLE = {
  margin: 0, padding: '8px 12px', flex: '1 0 auto', background: 'transparent',
} as const;

// A file shown with its line numbers. Lines do not wrap, so the numbers stay
// next to their line; the gutter stays put while the code scrolls sideways.
export function CodeView({ code }: { code: string }) {
  return (
    <div data-slot="code-view" className="numbered-code">
      <Gutter lines={lineCount(code)} className="numbered-code-sticky" />
      <PrismCode code={code} style={CODE_STYLE} />
    </div>
  );
}

// A file being edited, with its line numbers. The textarea scrolls on its own;
// the gutter follows it.
export function CodeEditor({
  value, disabled, onChange, rows = 24,
}: {
  value: string;
  disabled?: boolean;
  onChange: (text: string) => void;
  rows?: number;
}) {
  const gutter = useRef<HTMLDivElement>(null);
  return (
    <div data-slot="code-editor" className="numbered-code numbered-code-editor" style={{ height: rows * 20 + 16 }}>
      <div ref={gutter} className="numbered-code-gutter-scroll">
        <Gutter lines={lineCount(value)} />
      </div>
      <textarea
        aria-label="File content"
        spellCheck={false}
        wrap="off"
        value={value}
        disabled={disabled}
        onChange={(e) => onChange(e.target.value)}
        onScroll={(e) => {
          if (gutter.current) {
            gutter.current.scrollTop = e.currentTarget.scrollTop;
          }
        }}
      />
    </div>
  );
}
