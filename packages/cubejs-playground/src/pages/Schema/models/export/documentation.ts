import type { ModelDefinition, ModelTable } from '../modelGraph';

export type DocumentationInput = {
  name: string;
  description?: string;
  def: ModelDefinition;
  columnsOf: (t: ModelTable) => { name: string; type: string }[];
  // The diagram as an image data URL; none leaves its section out.
  diagram: string | null;
  // The resolved values of --background, --foreground, --muted and --border.
  colors: Record<string, string>;
  generatedAt: string;
};

const ENTITIES: Record<string, string> = {
  '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;',
};

// Every piece of user text goes through here.
export const escapeHtml = (s: unknown): string => String(s ?? '').replace(/[&<>"']/g, (c) => ENTITIES[c]);

const e = escapeHtml;

const table = (head: string[], rows: unknown[][]): string => (rows.length
  ? `<table><thead><tr>${head.map((h) => `<th>${e(h)}</th>`).join('')}</tr></thead><tbody>${
    rows.map((r) => `<tr>${r.map((c) => `<td>${e(c)}</td>`).join('')}</tr>`).join('')
  }</tbody></table>`
  : '');

const list = (title: string, items: string[]): string => (items.length
  ? `<h4>${e(title)}</h4><ul>${items.map((i) => `<li>${e(i)}</li>`).join('')}</ul>`
  : '');

const yesNo = (v?: boolean): string => {
  if (v === undefined) {
    return '';
  }
  return v ? 'Yes' : 'No';
};

const comment = (c?: string): string => (c ? `<p class="comment">${e(c)}</p>` : '');

function tableSection(t: ModelTable, columns: { name: string; type: string }[]): string {
  const { draft } = t;
  const cols = draft
    ? draft.columns.map((c) => [c.name, c.type, c.nullable ? 'Yes' : 'No', c.primaryKey ? 'Yes' : '', c.comment ?? ''])
    : columns.map((c) => [c.name, c.type, '', '', '']);
  const pk = draft?.columns.filter((c) => c.primaryKey).map((c) => c.name) ?? [];
  return `<section class="object" id="table-${e(t.alias)}"><h3>${e(t.alias)} <small>${e(t.schema)}.${e(t.table)}</small></h3>${
    comment(t.comment)
  }${table(['Name', 'Type', 'Nullable', 'PK', 'Comment'], cols)}${
    list('Keys', [
      ...(pk.length ? [`Primary key${draft?.primaryKeyName ? ` ${draft.primaryKeyName}` : ''} (${pk.join(', ')})`] : []),
      ...(draft?.uniqueKeys ?? []).map((u) => `Unique ${u.name} (${u.columns.join(', ')})`),
    ])
  }${
    list('Indexes', (draft?.indexes ?? []).map((i) => `${i.name} (${i.columns.map((c) => c.name + (c.desc ? ' DESC' : '')).join(', ')})${i.unique ? ' unique' : ''}`))
  }${
    list('Checks', (draft?.checks ?? []).map((c) => `${c.name}: ${c.expression}`))
  }</section>`;
}

export function documentationHtml(input: DocumentationInput): string {
  const {
    name, description, def, columnsOf, diagram, colors, generatedAt,
  } = input;
  const views = def.views ?? [];
  const cubes = def.cubes ?? [];
  const notes = def.notes ?? [];
  const cubeName = (id: string) => cubes.find((c) => c.id === id)?.name ?? id;
  const end = (x: { alias: string; column: string }) => `${x.alias}.${x.column}`;

  const sections: { id: string; title: string; body: string }[] = [];
  if (diagram) {
    sections.push({ id: 'diagram', title: 'Diagram', body: `<img src="${e(diagram)}" alt="${e(`Diagram of ${name}`)}">` });
  }
  sections.push({
    id: 'tables',
    title: 'Tables',
    body: def.tables.map((t) => tableSection(t, columnsOf(t))).join('') || '<p class="comment">No tables.</p>',
  });
  sections.push({
    id: 'references',
    title: 'References',
    body: table(
      ['From', 'To', 'Cardinality', 'Mandatory'],
      def.joins.map((j) => [
        end(j.from),
        end(j.to),
        j.cardinality ?? '',
        yesNo(j.mandatory),
      ])
    ) || '<p class="comment">No references.</p>',
  });
  sections.push({
    id: 'views',
    title: 'Views',
    body: views.map((v) => `<section class="object"><h3>${e(v.name)} <small>${e(v.schema)}</small></h3>${comment(v.comment)}${
      table(['Name', 'Type'], v.columns.map((c) => [c.name, c.type]))
    }<pre>${e(v.sql)}</pre></section>`).join('') || '<p class="comment">No views.</p>',
  });
  sections.push({
    id: 'cubes',
    title: 'Cubes',
    body: `${cubes.map((c) => `<section class="object"><h3>${e(c.name)}${c.title ? ` <small>${e(c.title)}</small>` : ''}</h3>${
      comment(c.description)
    }<p>Source: ${e(c.source.kind)}${c.source.table ? ` ${e(c.source.table)}` : ''}</p>${
      c.source.sql ? `<pre>${e(c.source.sql)}</pre>` : ''
    }${table(['Dimension', 'Type', 'Column or SQL'], c.dimensions.map((d) => [d.name, d.type, d.column ?? d.sql ?? '']))}${
      table(['Measure', 'Type', 'Column or SQL'], c.measures.map((m) => [m.name, m.type, m.column ?? m.sql ?? '']))
    }${table(['Segment', 'SQL'], c.segments.map((s) => [s.name, s.sql]))}</section>`).join('') || '<p class="comment">No cubes.</p>'}${
      table(
        ['From', 'To', 'Relationship', 'On'],
        (def.cubeJoins ?? []).map((j) => [cubeName(j.from), cubeName(j.to), j.relationship, j.on.map((o) => `${o.from} = ${o.to}`).join(', ')])
      )
    }`,
  });
  sections.push({
    id: 'notes',
    title: 'Notes',
    body: notes.map((n) => `<p class="note">${e(n.text)}</p>`).join('') || '<p class="comment">No notes.</p>',
  });

  const color = (k: string) => e(colors[k] ?? 'inherit');
  const css = `body{margin:0;padding:2rem;max-width:64rem;margin-inline:auto;font:14px/1.5 system-ui,sans-serif;background:${color('--background')};color:${color('--foreground')}}`
    + `table{border-collapse:collapse;width:100%;margin:.5rem 0}th,td{border:1px solid ${color('--border')};padding:.25rem .5rem;text-align:left}th{background:${color('--muted')}}`
    + `pre{background:${color('--muted')};border:1px solid ${color('--border')};padding:.75rem;overflow:auto}img{max-width:100%}`
    + `small,.comment{opacity:.7}.object{margin:1rem 0;padding-top:.5rem;border-top:1px solid ${color('--border')}}.note{background:${color('--muted')};padding:.5rem}`;

  return `<!doctype html><html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>${e(name)}</title><style>${css}</style></head><body><header><h1>${e(name)}</h1>${
    comment(description ?? def.description)
  }<p class="comment">Generated ${e(generatedAt)}</p></header><nav><ul>${
    sections.map((s) => `<li><a href="#${s.id}">${e(s.title)}</a></li>`).join('')
  }</ul></nav>${sections.map((s) => `<section id="${s.id}"><h2>${e(s.title)}</h2>${s.body}</section>`).join('')}</body></html>`;
}
