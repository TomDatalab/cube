// What the cube panel needs to edit a cube's members: the file it is written
// in, whether it may be edited there (and why not), and the text of a
// failed edit.

import { ApiError } from './api';
import { CubeEditError } from './cubeYaml';
import { problemLines } from './editor';
import type { OrgDetail, TreeCube } from './tree';

export type CubeFile = OrgDetail['files'][number];

export const cubeFileOf = (files: CubeFile[], cube: Pick<TreeCube, 'file'>): CubeFile | undefined => (
  files.find((f) => f.fileName === cube.file)
);

/**
 * Why the cube's members cannot be edited in the panel, or null when they
 * can. `routed`: the organization's model is in the database (the `/v1`
 * model routes save it).
 */
export function cubeEditBlock({ cube, file, routed }: {
  cube: Pick<TreeCube, 'type'>;
  file: CubeFile | undefined;
  routed: boolean;
}): string | null {
  if (cube.type === 'view') {
    return 'Views não são editadas aqui: abra o arquivo da view';
  }
  if (!routed || !file?.id || file.version == null) {
    return 'Este cubo é editado pelo seu arquivo';
  }
  if (file.editable === false) {
    return 'O arquivo deste cubo é somente leitura';
  }
  if (!/\.ya?ml$/i.test(file.fileName)) {
    return 'Só cubos em arquivos YAML são editados aqui: abra o arquivo';
  }
  return null;
}

/** `members[].name` is `<cube>.<member>`; the file names the member alone. */
export const shortName = (name: string) => name.slice(name.indexOf('.') + 1);

/** A failed edit, for the user: the reason and the model's problems. */
export function editFailure(e: unknown): { message: string; problems: string[] } {
  if (e instanceof CubeEditError) {
    return { message: e.message, problems: [] };
  }
  if (e instanceof ApiError && e.status === 409) {
    return { message: 'O arquivo mudou desde que foi aberto: tente de novo', problems: [] };
  }
  if (e instanceof ApiError) {
    return { message: e.message, problems: problemLines(e.problems ?? []) };
  }
  return { message: e instanceof Error ? e.message : String(e), problems: [] };
}
