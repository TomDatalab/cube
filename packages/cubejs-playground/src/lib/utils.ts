import { type ClassValue, clsx } from 'clsx';
import { extendTailwindMerge } from 'tailwind-merge';

// Tailwind runs with the `tw:` prefix (src/shadcn.css): merging must know it.
const twMerge = extendTailwindMerge({ prefix: 'tw' });

/** Joins class names, the later Tailwind utilities winning (shadcn/ui). */
export function cn(...inputs: ClassValue[]) {
  return twMerge(clsx(inputs));
}
