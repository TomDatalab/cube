import { NodeResizeControl, NodeResizer } from '@xyflow/react';
import type { ControlPosition, OnResizeEnd } from '@xyflow/react';

// Redgate's square grips: React Flow's stylesheet is unlayered, hence inline.
const GRIP = {
  width: 8,
  height: 8,
  borderRadius: 0,
  border: '1px solid var(--ring)',
  background: 'var(--card)',
};
// React Flow puts grips on the corners only; Redgate has one in the middle
// of each side too.
const SIDES: ControlPosition[] = ['top', 'right', 'bottom', 'left'];

type ModelResizerProps = {
  visible: boolean;
  minWidth: number;
  minHeight: number;
  onResizeEnd: OnResizeEnd;
};

// The selected node's frame: drag any side or corner, or the grip in the
// middle of a side, to resize it.
export function ModelResizer({
  visible, minWidth, minHeight, onResizeEnd,
}: ModelResizerProps) {
  return (
    <>
      <NodeResizer
        isVisible={visible}
        minWidth={minWidth}
        minHeight={minHeight}
        color="var(--ring)"
        handleStyle={GRIP}
        onResizeEnd={onResizeEnd}
      />
      {visible ? SIDES.map((position) => (
        <NodeResizeControl
          key={position}
          position={position}
          minWidth={minWidth}
          minHeight={minHeight}
          style={GRIP}
          onResizeEnd={onResizeEnd}
        />
      )) : null}
    </>
  );
}
