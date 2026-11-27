// @vitest-environment jsdom
import { describe, expect, it } from 'vitest';

import { documentationHtml } from '../pages/Schema/models/export/documentation';
import { fixtureColumnsOf, modelFixture } from './fixtures/model-fixture';

const colors = {
  '--background': 'white', '--foreground': 'black', '--muted': 'gray', '--border': 'silver',
};
const make = (over: Record<string, unknown> = {}, def = modelFixture()) => documentationHtml({
  name: 'Sales',
  def,
  columnsOf: fixtureColumnsOf,
  diagram: 'data:image/svg+xml;base64,AAAA',
  colors,
  generatedAt: '2026-11-27T00:00:00Z',
  ...over,
});

describe('documentation', () => {
  it("lists the model's objects", () => {
    const html = make();
    const at = ['diagram', 'tables', 'references', 'views', 'cubes', 'notes'].map((id) => html.indexOf(`id="${id}"`));
    expect(at.every((i) => i > 0)).toBe(true);
    expect([...at].sort((a, b) => a - b)).toEqual(at);
    expect(html).toContain('<title>Sales</title>');
    expect(html).toContain('<img src="data:image/svg+xml;base64,AAAA"');
    for (const c of ['id', 'customer_id', 'email', 'amount']) {
      expect(html).toContain(`<td>${c}</td>`);
    }
    expect(html).toContain('orders_amount_ck: amount &gt;= 0');
    expect(html).toContain('<td>orders.customer_id</td><td>customers.id</td><td>0..*</td>');
    expect(html).toMatch(/<pre>SELECT \* FROM public\.orders WHERE amount &gt; 100<\/pre>/);
    expect(html).toContain('<td>total</td>');
    expect(html).toContain('href="#cubes"');
    expect(html).not.toMatch(/src="http|href="http/);
  });

  it('escapes user text', () => {
    const def = modelFixture();
    def.tables[0].comment = '<script>alert(1)</script> & "x"';
    const html = make({}, def);
    expect(html).toContain('&lt;script&gt;alert(1)&lt;/script&gt; &amp; &quot;x&quot;');
    expect(html).not.toContain('<script');
  });

  it('without a diagram has no #diagram section', () => {
    const html = make({ diagram: null });
    expect(html).not.toContain('id="diagram"');
    expect(html).toContain('id="tables"');
  });
});
