import {
  Alert, Button, Modal, Space, Spin, Typography,
} from 'antd';
import {
  PointerEvent as ReactPointerEvent, useEffect, useLayoutEffect, useMemo, useRef, useState,
} from 'react';

import { AdminApi, User } from '../api';
import {
  applyPositions, bounds, edgeGeometry, EdgeGeometry, expandFor, fitView, highlight, layout, LaidOutNode,
  movedBeyond, NODE_W, nodeRows, Point, Positions, View, zoomAt,
} from './layout';
import { AccessEdge, AccessGraph, AccessNodeKind, Reason } from './types';

type Props = {
  api: AdminApi;
  user: User | null;
  onClose: () => void;
};

// theme tokens (src/theme/dark-tokens.ts) with today's color as the fallback;
// applied through `style`, since `var()` in a presentation attribute is not
// reliable across browsers
const ACCENT = 'var(--pg-accent, #7a77ff)';
const ACCENT_BG = 'var(--pg-graph-accent-bg, #e8e7ff)';
const LINE = 'var(--pg-graph-line, #d9d8e8)';
const MUTED = 'var(--pg-graph-muted, #6b6a85)';
const FG = 'var(--pg-graph-fg, #1f1d3a)';
const DERIVED = 'var(--pg-graph-derived, #9c9ab8)';
const PANEL = 'var(--pg-graph-panel, #f6f6fb)';
const NODE_BG = 'var(--pg-graph-node, #fff)';

const KIND_COLOR: Record<AccessNodeKind, string> = {
  user: 'var(--pg-graph-user, #1f1d3a)',
  organization: 'var(--pg-graph-organization, #2f7d6d)',
  workspace: 'var(--pg-graph-workspace, #6a66f0)',
  connection: 'var(--pg-graph-connection, #b8641e)',
};

const MONO = 'ui-monospace, SFMono-Regular, Menlo, monospace';
const NARROW = '(max-width: 820px)';
const IDENTITY: View = { x: 0, y: 0, k: 1 };
const ZOOM_STEP = 1.25;

/** A pointer held down on a node (to move it) or on the background (to pan). */
type Drag = {
  pointerId: number;
  /** The node being moved; none while panning. */
  node?: string;
  /** Pressed on the node's fold toggle: a click there flips it. */
  toggle: boolean;
  /** Pointer at the start, in SVG viewport coordinates. */
  start: Point;
  /** The node's position, or the view's translation, at the start. */
  origin: Point;
  moved: boolean;
};

function useNarrow() {
  const [narrow, setNarrow] = useState(() => window.matchMedia(NARROW).matches);
  useEffect(() => {
    const query = window.matchMedia(NARROW);
    const onChange = () => setNarrow(query.matches);
    query.addEventListener('change', onChange);
    return () => query.removeEventListener('change', onChange);
  }, []);
  return narrow;
}

function Edge({ geometry, hl }: { geometry: EdgeGeometry; hl: boolean }) {
  const { edge, d, label } = geometry;
  const derived = edge.kind === 'derived';
  let stroke = derived ? DERIVED : FG;
  if (hl) {
    stroke = ACCENT;
  }
  return (
    <g>
      <path
        d={d}
        fill="none"
        style={{ stroke }}
        strokeWidth={hl ? 2.5 : 1.5}
        strokeDasharray={derived ? '5 4' : undefined}
        markerEnd={`url(#access-arrow${hl ? '-hl' : ''})`}
      />
      {label ? (
        <g>
          <rect
            x={label.x - label.w / 2}
            y={label.y - 9}
            width={label.w}
            height={18}
            rx={9}
            style={{ fill: hl ? ACCENT_BG : PANEL, stroke: hl ? ACCENT : LINE }}
          />
          <text x={label.x} y={label.y + 3.5} fontSize={10.5} textAnchor="middle" style={{ fill: hl ? ACCENT : MUTED }} fontFamily={MONO}>
            {edge.label}
          </text>
        </g>
      ) : null}
    </g>
  );
}

type NodeProps = {
  node: LaidOutNode;
  /** Unique among the nodes, for the clip path id. */
  index: number;
  hl: boolean;
  grabbing: boolean;
  /** Keyboard only: a pointer click on the toggle goes through onPointerDown. */
  onToggle: () => void;
  onPointerDown: (e: ReactPointerEvent<SVGGElement>) => void;
};

function Node({
  node, index, hl, grabbing, onToggle, onPointerDown,
}: NodeProps) {
  const color = KIND_COLOR[node.kind];
  const rows = nodeRows(node);
  const clip = `access-clip-${index}`;
  const tx = node.x + 16;
  const pill = node.hiddenCount ? `+${node.hiddenCount} workspaces` : 'collapse';
  return (
    <g onPointerDown={onPointerDown} style={{ cursor: grabbing ? 'grabbing' : 'grab' }}>
      <clipPath id={clip}>
        <rect x={node.x + 8} y={node.y} width={NODE_W - 16} height={node.h} />
      </clipPath>
      <rect
        x={node.x}
        y={node.y}
        width={NODE_W}
        height={node.h}
        rx={8}
        style={{ fill: NODE_BG, stroke: hl ? ACCENT : LINE }}
        strokeWidth={hl ? 2 : 1}
      />
      <rect x={node.x} y={node.y + 10} width={4} height={node.h - 20} rx={2} style={{ fill: color }} />
      <g clipPath={`url(#${clip})`}>
        <text x={tx} y={node.y + rows.kindY} fontSize={10.5} style={{ fill: color }} fontFamily={MONO}>
          {node.kind}
        </text>
        <text x={tx} y={node.y + rows.nameY} fontSize={14} fontWeight={600} style={{ fill: FG }}>
          {node.label}
          <title>{node.name}</title>
        </text>
        {node.permLines.length ? (
          <text fontSize={11.5} style={{ fill: MUTED }} fontFamily={MONO}>
            {node.permLines.map((line, i) => ({ line, y: node.y + rows.permY[i] })).map(({ line, y }) => (
              <tspan key={y} x={tx} y={y}>{line}</tspan>
            ))}
          </text>
        ) : null}
        {node.noteLines.length ? (
          <text fontSize={10.5} style={{ fill: MUTED }} fontStyle="italic">
            {node.noteLines.map((line, i) => ({ line, y: node.y + rows.noteY[i] })).map(({ line, y }) => (
              <tspan key={y} x={tx} y={y}>{line}</tspan>
            ))}
          </text>
        ) : null}
      </g>
      {rows.pillY != null ? (
        <g
          data-toggle
          role="button"
          tabIndex={0}
          aria-label={node.hiddenCount ? `Show ${pill}` : `Collapse ${node.name}`}
          onKeyDown={(e) => {
            if (e.key === 'Enter' || e.key === ' ') {
              e.preventDefault();
              onToggle();
            }
          }}
          style={{ cursor: 'pointer' }}
        >
          <rect
            x={tx}
            y={node.y + rows.pillY}
            width={pill.length * 6.4 + 16}
            height={18}
            rx={9}
            style={{ fill: ACCENT_BG, stroke: ACCENT }}
          />
          <text x={tx + 8} y={node.y + rows.pillY + 12.5} fontSize={10.5} style={{ fill: ACCENT }} fontFamily={MONO}>
            {pill}
          </text>
        </g>
      ) : null}
    </g>
  );
}

export function AccessModal({ api, user, onClose }: Props) {
  const [graph, setGraph] = useState<AccessGraph | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [expanded, setExpanded] = useState<Set<string>>(new Set());
  const [active, setActive] = useState<Reason | null>(null);
  // Where nodes were dragged to, by node id; kept across expand/collapse.
  const [positions, setPositions] = useState<Positions>({});
  // Null until the graph is first fitted into the viewport.
  const [view, setView] = useState<View | null>(null);
  const [svg, setSvg] = useState<SVGSVGElement | null>(null);
  const [drag, setDrag] = useState<Drag | null>(null);
  const viewport = useRef<HTMLDivElement>(null);
  const narrow = useNarrow();
  const viewHeight = narrow ? 420 : 600;
  const userId = user?.id;

  useEffect(() => {
    setGraph(null);
    setError(null);
    setExpanded(new Set());
    setActive(null);
    setPositions({});
    setView(null);
    if (!userId) {
      return undefined;
    }
    let cancelled = false;
    api.access(userId).then(
      (g) => !cancelled && setGraph(g),
      (e) => !cancelled && setError(e.message)
    );
    return () => {
      cancelled = true;
    };
  }, [api, userId]);

  const drawn = useMemo(() => (graph ? layout(graph, expanded) : null), [graph, expanded]);
  const marked = useMemo(() => (active ? highlight(active) : null), [active]);
  const placed = useMemo(() => {
    if (!drawn) {
      return null;
    }
    const nodes = applyPositions(drawn.nodes, positions);
    const geometry = edgeGeometry(nodes, drawn.edges);
    return { nodes, geometry, box: bounds(nodes, geometry) };
  }, [drawn, positions]);

  function fit() {
    const width = viewport.current?.clientWidth || 0;
    if (placed && width > 0) {
      setView(fitView(placed.box, width, viewHeight));
    }
  }

  // The graph opens fitted (once the viewport has a width).
  useLayoutEffect(() => {
    if (view || !placed) {
      return undefined;
    }
    if (viewport.current?.clientWidth) {
      fit();
      return undefined;
    }
    const frame = requestAnimationFrame(fit);
    return () => cancelAnimationFrame(frame);
  });

  // Pointer position in the SVG's own coordinates, whatever CSS does to it.
  function svgPoint(e: { clientX: number; clientY: number }): Point {
    const m = svg?.getScreenCTM();
    if (!m) {
      return { x: e.clientX, y: e.clientY };
    }
    const p = new DOMPoint(e.clientX, e.clientY).matrixTransform(m.inverse());
    return { x: p.x, y: p.y };
  }
  const svgPointRef = useRef(svgPoint);
  svgPointRef.current = svgPoint;

  // Wheel (and pinch, a wheel with ctrlKey) zooms around the pointer. Not
  // passive, so the page does not scroll meanwhile.
  useEffect(() => {
    if (!svg) {
      return undefined;
    }
    const onWheel = (e: WheelEvent) => {
      e.preventDefault();
      const p = svgPointRef.current(e);
      const delta = e.deltaY * (e.deltaMode === 1 ? 16 : 1);
      const factor = Math.exp(-delta * (e.ctrlKey ? 0.01 : 0.002));
      setView((v) => zoomAt(v || IDENTITY, factor, p.x, p.y));
    };
    svg.addEventListener('wheel', onWheel, { passive: false });
    return () => svg.removeEventListener('wheel', onWheel);
  }, [svg]);

  function zoomBy(factor: number) {
    const width = viewport.current?.clientWidth || 0;
    setView((v) => zoomAt(v || IDENTITY, factor, width / 2, viewHeight / 2));
  }

  function startDrag(e: ReactPointerEvent<Element>, node?: LaidOutNode) {
    if (e.button !== 0 || !svg) {
      return;
    }
    e.stopPropagation();
    svg.setPointerCapture(e.pointerId);
    const v = view || IDENTITY;
    setDrag({
      pointerId: e.pointerId,
      node: node?.id,
      toggle: !!(e.target as Element).closest?.('[data-toggle]'),
      start: svgPoint(e),
      origin: node ? { x: node.x, y: node.y } : { x: v.x, y: v.y },
      moved: false,
    });
  }

  function moveDrag(e: ReactPointerEvent<SVGSVGElement>) {
    if (!drag || e.pointerId !== drag.pointerId) {
      return;
    }
    const p = svgPoint(e);
    const dx = p.x - drag.start.x;
    const dy = p.y - drag.start.y;
    if (!drag.moved && !movedBeyond(dx, dy)) {
      return;
    }
    if (!drag.moved) {
      setDrag({ ...drag, moved: true });
    }
    const { node, origin } = drag;
    if (node) {
      // Screen distance over the scale: the node stays under the pointer.
      const { k } = view || IDENTITY;
      setPositions((prev) => ({ ...prev, [node]: { x: origin.x + dx / k, y: origin.y + dy / k } }));
    } else {
      setView((v) => ({ ...(v || IDENTITY), x: origin.x + dx, y: origin.y + dy }));
    }
  }

  function endDrag(e: ReactPointerEvent<SVGSVGElement>) {
    if (!drag || e.pointerId !== drag.pointerId) {
      return;
    }
    if (svg?.hasPointerCapture(e.pointerId)) {
      svg.releasePointerCapture(e.pointerId);
    }
    // A press that did not move is a click: on the toggle, it flips it.
    if (!drag.moved && drag.toggle && drag.node && e.type === 'pointerup') {
      flip(drag.node);
    }
    setDrag(null);
  }

  function flip(orgId: string) {
    setExpanded((prev) => {
      const next = new Set(prev);
      if (!next.delete(orgId)) {
        next.add(orgId);
      }
      return next;
    });
  }

  const grouped = useMemo(() => {
    if (!graph) {
      return [];
    }
    return graph.nodes
      .map((node) => ({ node, grants: graph.grants.filter((g) => g.resource === node.id) }))
      .filter((r) => r.grants.length > 0);
  }, [graph]);

  const summary = graph ? `${new Set(graph.grants.map((g) => g.resource)).size} resources · ${graph.grants.length} permissions` : null;

  function choose(r: Reason) {
    if (active === r) {
      setActive(null);
      return;
    }
    // A reason through a collapsed organization opens it, so its path shows.
    if (graph && drawn) {
      const { nodes } = drawn;
      setExpanded((prev) => expandFor(graph, nodes, r.path, prev));
    }
    setActive(r);
  }

  function renderGraph() {
    if (!placed) {
      return null;
    }
    const { nodes, geometry } = placed;
    const v = view || IDENTITY;
    const isHl = (e: AccessEdge) => marked?.edges.has(`${e.from}>${e.to}`) ?? false;
    // Highlighted edges last, so they sit above the others.
    const edges = [...geometry].sort((a, b) => Number(isHl(a.edge)) - Number(isHl(b.edge)));
    const panning = drag != null && drag.node == null;
    return (
      <div ref={viewport} style={{ overflow: 'hidden' }}>
        <svg
          ref={setSvg}
          width="100%"
          height={viewHeight}
          role="img"
          aria-label="Access graph"
          style={{
            display: 'block', touchAction: 'none', userSelect: 'none', cursor: panning ? 'grabbing' : 'grab',
          }}
          onPointerDown={(e) => startDrag(e)}
          onPointerMove={moveDrag}
          onPointerUp={endDrag}
          onPointerCancel={endDrag}
        >
          <defs>
            {[['', MUTED], ['-hl', ACCENT]].map(([suffix, fill]) => (
              <marker
                key={suffix}
                id={`access-arrow${suffix}`}
                viewBox="0 0 10 10"
                refX={9}
                refY={5}
                markerWidth={7}
                markerHeight={7}
                orient="auto-start-reverse"
              >
                <path d="M0,0 L10,5 L0,10 z" style={{ fill }} />
              </marker>
            ))}
          </defs>
          <g transform={`translate(${v.x},${v.y}) scale(${v.k})`}>
            <g>
              {edges.map((g) => (
                <Edge key={`${g.edge.from}>${g.edge.to}>${g.edge.label}`} geometry={g} hl={isHl(g.edge)} />
              ))}
            </g>
            {nodes.map((n, i) => (
              <Node
                key={n.id}
                node={n}
                index={i}
                hl={marked?.nodes.has(n.id) ?? false}
                grabbing={drag?.node === n.id && drag.moved}
                onToggle={() => flip(n.id)}
                onPointerDown={(e) => startDrag(e, n)}
              />
            ))}
          </g>
        </svg>
      </div>
    );
  }

  function renderPanel() {
    if (!graph) {
      return null;
    }
    return (
      <div style={{
        padding: 16, minWidth: 0, maxHeight: narrow ? undefined : viewHeight, overflowY: 'auto',
      }}
      >
        <Typography.Text type="secondary" style={{ textTransform: 'uppercase', fontSize: 12, letterSpacing: '.06em' }}>
          What {graph.user.id} can do
        </Typography.Text>
        {graph.platformAdmin ? (
          <Typography.Paragraph style={{ marginTop: 8 }}>
            Platform admin: full access to every organization, workspace and connection.
          </Typography.Paragraph>
        ) : null}
        {grouped.map(({ node, grants }) => (
          <div key={node.id} style={{ marginTop: 14 }}>
            <div style={{ fontWeight: 600 }}>
              {node.name} <Typography.Text type="secondary" style={{ fontSize: 12, fontWeight: 400 }}>{node.kind}</Typography.Text>
            </div>
            {node.notes.map((note) => (
              <div key={note}><Typography.Text type="secondary" style={{ fontSize: 12 }}>{note}</Typography.Text></div>
            ))}
            {grants.map((g) => (
              <div key={g.permission} style={{ marginTop: 6 }}>
                <code style={{ fontFamily: MONO, fontSize: 12 }}>{g.permission}</code>
                {g.reasons.map((r) => {
                  const on = active === r;
                  return (
                    <div
                      key={r.path.join('>')}
                      role="button"
                      tabIndex={0}
                      onClick={() => choose(r)}
                      onKeyDown={(e) => {
                        if (e.key === 'Enter' || e.key === ' ') {
                          e.preventDefault();
                          choose(r);
                        }
                      }}
                      style={{
                        cursor: 'pointer',
                        padding: '2px 8px',
                        marginTop: 2,
                        borderRadius: 4,
                        fontSize: 12,
                        color: on ? ACCENT : MUTED,
                        background: on ? ACCENT_BG : undefined,
                      }}
                    >
                      {r.text}
                    </div>
                  );
                })}
              </div>
            ))}
          </div>
        ))}
        {graph.nodes.filter((n) => n.notes.length > 0 && !grouped.some((r) => r.node.id === n.id)).map((n) => (
          <div key={n.id} style={{ marginTop: 14 }}>
            <div style={{ fontWeight: 600 }}>{n.name}</div>
            {n.notes.map((note) => (
              <div key={note}><Typography.Text type="secondary" style={{ fontSize: 12 }}>{note}</Typography.Text></div>
            ))}
          </div>
        ))}
      </div>
    );
  }

  let body;
  if (error) {
    body = <Alert type="error" message={error} showIcon />;
  } else if (!graph) {
    body = <div style={{ textAlign: 'center', padding: 48 }}><Spin /></div>;
  } else {
    body = (
      <>
        {graph.user.disabled ? (
          <Alert
            type="warning"
            showIcon
            message="Disabled: these grants do not apply while the user is disabled."
            style={{ marginBottom: 12 }}
          />
        ) : null}
        <div
          style={{
            display: 'grid',
            gridTemplateColumns: narrow ? 'minmax(0, 1fr)' : 'minmax(0, 1fr) 320px',
            border: `1px solid ${LINE}`,
            borderRadius: 8,
          }}
        >
          <div
            style={{
              minWidth: 0,
              background: PANEL,
              borderRight: narrow ? 0 : `1px solid ${LINE}`,
              borderBottom: narrow ? `1px solid ${LINE}` : 0,
            }}
          >
            {renderGraph()}
          </div>
          {renderPanel()}
        </div>
      </>
    );
  }

  return (
    <Modal
      title={(
        <div style={{ display: 'flex', justifyContent: 'space-between', gap: 12, flexWrap: 'wrap', paddingRight: 24 }}>
          <span>Access of {userId}</span>
          <Space size={8} wrap>
            {summary ? <Typography.Text type="secondary" style={{ fontFamily: MONO, fontSize: 12 }}>{summary}</Typography.Text> : null}
            {graph ? (
              <Space size={4}>
                <Button size="small" aria-label="Zoom out" onClick={() => zoomBy(1 / ZOOM_STEP)}>−</Button>
                <Button size="small" aria-label="Zoom in" onClick={() => zoomBy(ZOOM_STEP)}>+</Button>
                <Button size="small" onClick={fit}>Fit</Button>
                <Button size="small" disabled={!Object.keys(positions).length} onClick={() => setPositions({})}>
                  Reset layout
                </Button>
              </Space>
            ) : null}
          </Space>
        </div>
      )}
      visible={user != null}
      onCancel={onClose}
      footer={null}
      width="90vw"
      destroyOnClose
    >
      {body}
    </Modal>
  );
}
