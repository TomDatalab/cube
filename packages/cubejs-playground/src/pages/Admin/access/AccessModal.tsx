import { Alert, Modal, Spin, Typography } from 'antd';
import { useEffect, useMemo, useState } from 'react';

import { AdminApi, User } from '../api';
import { COLLAPSE_AT, highlight, layout, LaidOutNode, NODE_H, NODE_W } from './layout';
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

function Edge({ edge, from, to, hl }: { edge: AccessEdge; from: LaidOutNode; to: LaidOutNode; hl: boolean }) {
  const x1 = from.x + NODE_W;
  const y1 = from.y + NODE_H / 2;
  const x2 = to.x;
  const y2 = to.y + NODE_H / 2;
  const dx = (x2 - x1) * 0.5;
  const derived = edge.kind === 'derived';
  let stroke = derived ? DERIVED : FG;
  if (hl) {
    stroke = ACCENT;
  }
  const mx = (x1 + x2) / 2;
  const my = (y1 + y2) / 2;
  const w = edge.label.length * 6.4 + 12;
  return (
    <g>
      <path
        d={`M${x1},${y1} C${x1 + dx},${y1} ${x2 - dx},${y2} ${x2},${y2}`}
        fill="none"
        stroke={stroke}
        strokeWidth={hl ? 2.5 : 1.5}
        strokeDasharray={derived ? '5 4' : undefined}
        markerEnd={`url(#access-arrow${hl ? '-hl' : ''})`}
      />
      <rect
        x={mx - w / 2}
        y={my - 10}
        width={w}
        height={18}
        rx={9}
        fill={hl ? ACCENT_BG : '#f6f6fb'}
        stroke={hl ? ACCENT : LINE}
      />
      <text x={mx} y={my + 3.5} fontSize={10.5} textAnchor="middle" fill={hl ? ACCENT : MUTED} fontFamily={MONO}>
        {edge.label}
      </text>
    </g>
  );
}

type Toggle = { label: string; onClick: () => void };

function Node({ node, hl, toggle }: { node: LaidOutNode; hl: boolean; toggle?: Toggle }) {
  const color = KIND_COLOR[node.kind];
  return (
    <g>
      <rect
        x={node.x}
        y={node.y}
        width={NODE_W}
        height={NODE_H}
        rx={8}
        fill="#fff"
        stroke={hl ? ACCENT : LINE}
        strokeWidth={hl ? 2 : 1}
      />
      <rect x={node.x} y={node.y + 10} width={4} height={NODE_H - 20} rx={2} fill={color} />
      <text x={node.x + 16} y={node.y + 24} fontSize={14} fontWeight={600} fill={FG}>
        {node.name.length > 14 ? `${node.name.slice(0, 13)}…` : node.name}
        <title>{node.name}</title>
      </text>
      <text x={node.x + NODE_W - 12} y={node.y + 24} fontSize={10.5} textAnchor="end" fill={color} fontFamily={MONO}>
        {node.kind}
      </text>
      <text x={node.x + 16} y={node.y + 44} fontSize={11.5} fill={MUTED} fontFamily={MONO}>
        {node.perms}
      </text>
      {toggle ? (
        <g onClick={toggle.onClick} style={{ cursor: 'pointer' }}>
          <rect
            x={node.x + NODE_W - 96}
            y={node.y + NODE_H - 8}
            width={84}
            height={16}
            rx={8}
            fill={ACCENT_BG}
            stroke={ACCENT}
          />
          <text
            x={node.x + NODE_W - 54}
            y={node.y + NODE_H + 3.5}
            fontSize={10.5}
            textAnchor="middle"
            fill={ACCENT}
            fontFamily={MONO}
          >
            {toggle.label}
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

  // Organizations that can fold: those with more workspaces than COLLAPSE_AT.
  const foldable = useMemo(() => {
    const count = new Map<string, number>();
    graph?.nodes.forEach((n) => {
      if (n.kind === 'workspace' && n.organizationId) {
        const key = `organization:${n.organizationId}`;
        count.set(key, (count.get(key) || 0) + 1);
      }
    });
    return new Set([...count].filter(([, c]) => c > COLLAPSE_AT).map(([k]) => k));
  }, [graph]);

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

  function renderGraph() {
    if (!drawn) {
      return null;
    }
    const byId = new Map(drawn.nodes.map((n) => [n.id, n]));
    const isHl = (e: AccessEdge) => marked?.edges.has(`${e.from}>${e.to}`) ?? false;
    // Highlighted edges last, so they sit above the others.
    const edges = [...drawn.edges].sort((a, b) => Number(isHl(a)) - Number(isHl(b)));
    return (
      <div style={{ overflow: 'auto' }}>
        <svg width={drawn.width} height={drawn.height} role="img" aria-label="Access graph">
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
            {edges.map((e) => {
              const from = byId.get(e.from);
              const to = byId.get(e.to);
              return from && to ? <Edge key={`${e.from}>${e.to}`} edge={e} from={from} to={to} hl={isHl(e)} /> : null;
            })}
          </g>
          {drawn.nodes.map((n) => {
            let toggle;
            if (n.hiddenCount) {
              toggle = { label: `+${n.hiddenCount} workspaces`, onClick: () => flip(n.id) };
            } else if (foldable.has(n.id)) {
              toggle = { label: 'collapse', onClick: () => flip(n.id) };
            }
            return <Node key={n.id} node={n} hl={marked?.nodes.has(n.id) ?? false} toggle={toggle} />;
          })}
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
                      key={r.text}
                      role="button"
                      tabIndex={0}
                      onClick={() => setActive(on ? null : r)}
                      onKeyDown={(e) => {
                        if (e.key === 'Enter' || e.key === ' ') {
                          e.preventDefault();
                          setActive(on ? null : r);
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
