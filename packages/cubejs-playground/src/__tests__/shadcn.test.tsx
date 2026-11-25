import { renderToStaticMarkup } from 'react-dom/server';
import { describe, expect, it } from 'vitest';

import { Button } from '@/components/ui/button';
import { cn } from '@/lib/utils';

describe('shadcn/ui', () => {
  it('cn lets the later Tailwind utility win', () => {
    expect(cn('tw:px-2 tw:py-1', false && 'tw:hidden', 'tw:px-4')).toBe('tw:py-1 tw:px-4');
  });

  it('renders a Button with its slot and variant classes', () => {
    const html = renderToStaticMarkup(<Button variant="outline">Save</Button>);
    expect(html).toContain('data-slot="button"');
    expect(html).toContain('tw:border');
    expect(html).toContain('>Save</button>');
  });
});
