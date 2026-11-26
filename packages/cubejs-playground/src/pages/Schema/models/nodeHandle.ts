// The node-level handle a dependency line ends on: invisible, top centre.
// Inline style: React Flow's stylesheet is unlayered (see TableNode).
export const NODE_HANDLE_STYLE = {
  opacity: 0,
  pointerEvents: 'none' as const,
  top: 0,
  left: '50%',
};
