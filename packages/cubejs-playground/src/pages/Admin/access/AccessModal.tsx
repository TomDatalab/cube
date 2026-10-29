import { Alert, Modal, Spin, Typography } from 'antd';
import { useEffect, useMemo, useState } from 'react';

import { AdminApi, User } from '../api';
import {
  bounds, edgeGeometry, EdgeGeometry, expandFor, highlight, layout, LaidOutNode, NODE_W, nodeRows,
} from './layout';
import { AccessEdge, AccessGraph, AccessNodeKind, Reason } from './types';

type Props = {
  api: AdminApi;
  user: User | null;
  onClose: () => void;
};

const ACCENT = '#7a77ff';
const ACCENT_BG = '#e8e7ff';
const LINE = '#d9d8e8';
const MUTED = '#6b6a85';
const FG = '#1f1d3a';
const DERIVED = '#9c9ab8';

const KIND_COLOR: Record<AccessNodeKind, string> = {
  user: '#1f1d3a',
  organization: '#2f7d6d',
  workspace: '#6a66f0',
  connection: '#b8641e',
};

const MONO = 'ui-monospace, SFMono-Regular, Menlo, monospace';
const NARROW = '(max-width: 820px)';

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
        stroke={stroke}
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
            fill={hl ? ACCENT_BG : '#f6f6fb'}
            stroke={hl ? ACCENT : LINE}
          />
          <text x={label.x} y={label.y + 3.5} fontSize={10.5} textAnchor="middle" fill={hl ? ACCENT : MUTED} fontFamily={MONO}>
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
  onToggle: () => void;
};

function Node({
  node, index, hl, onToggle,
}: NodeProps) {
  const color = KIND_COLOR[node.kind];
  const rows = nodeRows(node);
  const clip = `access-clip-${index}`;
  const tx = node.x + 16;
  const pill = node.hiddenCount ? `+${node.hiddenCount} workspaces` : 'collapse';
  return (
    <g>
      <clipPath id={clip}>
        <rect x={node.x + 8} y={node.y} width={NODE_W - 16} height={node.h} />
      </clipPath>
      <rect
        x={node.x}
        y={node.y}
        width={NODE_W}
        height={node.h}
        rx={8}
        fill="#fff"
        stroke={hl ? ACCENT : LINE}
        strokeWidth={hl ? 2 : 1}
      />
      <rect x={node.x} y={node.y + 10} width={4} height={node.h - 20} rx={2} fill={color} />
      <g clipPath={`url(#${clip})`}>
        <text x={tx} y={node.y + rows.kindY} fontSize={10.5} fill={color} fontFamily={MONO}>
          {node.kind}
        </text>
        <text x={tx} y={node.y + rows.nameY} fontSize={14} fontWeight={600} fill={FG}>
          {node.label}
          <title>{node.name}</title>
        </text>
        {node.permLines.length ? (
          <text fontSize={11.5} fill={MUTED} fontFamily={MONO}>
            {node.permLines.map((line, i) => ({ line, y: node.y + rows.permY[i] })).map(({ line, y }) => (
              <tspan key={y} x={tx} y={y}>{line}</tspan>
            ))}
          </text>
        ) : null}
        {node.noteLines.length ? (
          <text fontSize={10.5} fill={MUTED} fontStyle="italic">
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
          onClick={onToggle}
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
            fill={ACCENT_BG}
            stroke={ACCENT}
          />
          <text x={tx + 8} y={node.y + rows.pillY + 12.5} fontSize={10.5} fill={ACCENT} fontFamily={MONO}>
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
  const narrow = useNarrow();
  const userId = user?.id;

  useEffect(() => {
    setGraph(null);
    setError(null);
    setExpanded(new Set());
    setActive(null);
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
    if (!drawn) {
      return null;
    }
    const geometry = edgeGeometry(drawn.nodes, drawn.edges);
    const box = bounds(drawn.nodes, geometry);
    const isHl = (e: AccessEdge) => marked?.edges.has(`${e.from}>${e.to}`) ?? false;
    // Highlighted edges last, so they sit above the others.
    const edges = [...geometry].sort((a, b) => Number(isHl(a.edge)) - Number(isHl(b.edge)));
    return (
      <div style={{ overflow: 'auto' }}>
        <svg width={box.maxX + 20} height={box.maxY + 20} role="img" aria-label="Access graph">
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
                <path d="M0,0 L10,5 L0,10 z" fill={fill} />
              </marker>
            ))}
          </defs>
          <g>
            {edges.map((g) => (
              <Edge key={`${g.edge.from}>${g.edge.to}>${g.edge.label}`} geometry={g} hl={isHl(g.edge)} />
            ))}
          </g>
          {drawn.nodes.map((n, i) => (
            <Node key={n.id} node={n} index={i} hl={marked?.nodes.has(n.id) ?? false} onToggle={() => flip(n.id)} />
          ))}
        </svg>
      </div>
    );
  }

  function renderPanel() {
    if (!graph) {
      return null;
    }
    return (
      <div style={{ padding: 16, minWidth: 0 }}>
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
              background: '#f6f6fb',
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
          {summary ? <Typography.Text type="secondary" style={{ fontFamily: MONO, fontSize: 12 }}>{summary}</Typography.Text> : null}
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
