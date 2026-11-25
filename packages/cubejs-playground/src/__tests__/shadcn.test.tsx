import { renderToStaticMarkup } from 'react-dom/server';
import { describe, expect, it } from 'vitest';

import { Button } from '@/components/ui/button';
import { cn } from '@/lib/utils';

describe('shadcn/ui', () => {
  it('cn lets the later Tailwind utility win', () => {
    expect(cn('px-2 py-1', false && 'hidden', 'px-4')).toBe('py-1 px-4');
  });

  it('renders a Button with its slot and variant classes', () => {
    const html = renderToStaticMarkup(<Button variant="outline">Save</Button>);
    expect(html).toContain('data-slot="button"');
    expect(html).toContain('border');
    expect(html).toContain('>Save</button>');
  });
});
