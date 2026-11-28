// The node-level handle a dependency line ends on: invisible, top centre.
// Inline style: React Flow's stylesheet is unlayered (see TableNode).
export const NODE_HANDLE_STYLE = {
  opacity: 0,
  pointerEvents: 'none' as const,
  top: 0,
  left: '50%',
};

// The dots a link is dragged from and dropped on (a column, a cube): big
// enough to grab at the canvas' usual zoom. Inline: React Flow's stylesheet
// is unlayered and would win over Tailwind classes.
export const LINK_HANDLE_STYLE = {
  width: 16,
  height: 16,
  background: 'var(--primary)',
  border: '2px solid var(--card)',
};
