import type { Cell } from './explorerTables';

const field = (v: Cell): string => {
  if (v === null) {
    return '';
  }
  const s = String(v);
  return /[",\r\n]/.test(s) ? `"${s.replace(/"/g, '""')}"` : s;
};

// A result as RFC 4180 CSV: a header line, then one line per row, each
// ending with CRLF; NULL is an empty field.
export function toCsv(columns: string[], rows: Cell[][]): string {
  return [columns, ...rows].map((line) => `${line.map(field).join(',')}\r\n`).join('');
}
