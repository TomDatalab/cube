export type Problem = { file: string; cube: string; message: string };

export type EditorState = {
  mode: 'view' | 'edit' | 'saving';
  draft: string;
  // The version the draft started from.
  base: number;
  error: string | null;
  problems: Problem[];
  stale: boolean;
};

export type EditorAction = { type: 'edit' }
  | { type: 'change'; text: string }
  | { type: 'cancel' }
  | { type: 'save' }
  | { type: 'saved'; version: number; content: string }
  | { type: 'failed'; status: number; body: any };

export const initialEditor = (base: number): EditorState => ({
  mode: 'view', draft: '', base, error: null, problems: [], stale: false,
});

export function reduce(s: EditorState, a: EditorAction): EditorState {
  switch (a.type) {
    case 'edit':
      return { ...s, mode: 'edit', error: null, problems: [], stale: false };
    case 'change':
      return { ...s, draft: a.text };
    case 'cancel':
      return initialEditor(s.base);
    case 'save':
      return { ...s, mode: 'saving', error: null, problems: [], stale: false };
    case 'saved':
      return initialEditor(a.version);
    case 'failed': {
      const error: string = a.body?.error || `Erro ${a.status}`;
      return {
        ...s,
        mode: 'edit',
        error,
        problems: a.status === 400 && Array.isArray(a.body?.problems) ? a.body.problems : [],
        stale: a.status === 409,
      };
    }
    default:
      return s;
  }
}

export const problemLines = (problems: Problem[]): string[] => (
  problems.map((p) => (p.file ? `${p.file}: ${p.message}` : p.message))
);

export const SYNC_MESSAGE = 'Este arquivo é gerado a partir do catálogo do SofiaDB e é reescrito a cada sincronização. Para personalizar uma tabela, escreva o cubo dela em outro arquivo da organização, com outro nome; na próxima sincronização o cubo gerado sai';
export const JINJA_MESSAGE = 'Arquivos .jinja e .j2 não são editáveis aqui';

// Why a file cannot be edited, when the reason is one the UI knows.
export function readOnlyReason(file: { managedBy?: string | null; path?: string }): string | null {
  if (file.managedBy === 'sofiadb-sync') {
    return SYNC_MESSAGE;
  }
  if (/\.(jinja|j2)$/i.test(file.path ?? '')) {
    return JINJA_MESSAGE;
  }
  return null;
}
