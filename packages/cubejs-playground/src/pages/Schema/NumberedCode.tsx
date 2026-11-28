import { useMemo, useRef } from 'react';
import Prism from 'prismjs';

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

// The language PrismCode colors files with, so editing looks like viewing.
const LANGUAGE = 'javascript';

// The text as Prism colors it. Prism escapes it; the extra line lets the last,
// empty line of the text show (a trailing newline alone has no height).
export function highlighted(text: string): string {
  const grammar = Prism.languages[LANGUAGE];
  const body = grammar ? Prism.highlight(text, grammar, LANGUAGE) : escape(text);
  return `${body}\n`;
}

function escape(text: string): string {
  return text.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;');
}

// A file being edited, with its line numbers and the view's colors: the
// colored code is drawn under a textarea whose own text is transparent, so
// typing, the caret and the selection stay the browser's. The textarea
// scrolls; the colored code and the gutter follow it.
export function CodeEditor({
  value, disabled, onChange, rows = 24,
}: {
  value: string;
  disabled?: boolean;
  onChange: (text: string) => void;
  rows?: number;
}) {
  const gutter = useRef<HTMLDivElement>(null);
  const colored = useRef<HTMLPreElement>(null);
  const html = useMemo(() => highlighted(value), [value]);
  return (
    <div data-slot="code-editor" className="numbered-code numbered-code-editor" style={{ height: rows * 20 + 16 }}>
      <div ref={gutter} className="numbered-code-gutter-scroll">
        <Gutter lines={lineCount(value)} />
      </div>
      <div className="numbered-code-input">
        <pre ref={colored} aria-hidden="true" data-slot="code-colors" className={`language-${LANGUAGE}`}>
          {/* eslint-disable-next-line react/no-danger -- Prism's output, escaped by it */}
          <code className={`language-${LANGUAGE}`} dangerouslySetInnerHTML={{ __html: html }} />
        </pre>
        <textarea
          aria-label="File content"
          spellCheck={false}
          wrap="off"
          value={value}
          disabled={disabled}
          onChange={(e) => onChange(e.target.value)}
          onScroll={(e) => {
            const { scrollTop, scrollLeft } = e.currentTarget;
            if (gutter.current) {
              gutter.current.scrollTop = scrollTop;
            }
            if (colored.current) {
              colored.current.scrollTop = scrollTop;
              colored.current.scrollLeft = scrollLeft;
            }
          }}
        />
      </div>
    </div>
  );
}
