import { FormEvent, ReactElement, ReactNode, useState } from 'react';
import {
  Check, CircleAlert, Copy, Eye, EyeOff, GripVertical, Lock, Pencil, ShieldCheck, Tags,
} from 'lucide-react';

import { Alert, AlertDescription } from '@/components/ui/alert';
import { Button } from '@/components/ui/button';
import { Input } from '@/components/ui/input';
import { Switch } from '@/components/ui/switch';
import {
  Table, TableBody, TableCell, TableHead, TableHeader, TableRow,
} from '@/components/ui/table';
import {
  Tabs, TabsContent, TabsList, TabsTrigger,
} from '@/components/ui/tabs';
import {
  Tooltip, TooltipContent, TooltipProvider, TooltipTrigger,
} from '@/components/ui/tooltip';
import {
  EmptyState, Field, FormDialog, notify, Pager, SearchInput, Spinner, Tag,
} from '@/components/kit';

import { IdText } from './IdText';
import { AccessModal } from './access/AccessModal';
import { AdminApi, PAGE_SIZE, User } from './api';
import { AttributesModal } from './AttributesModal';
import { MembershipTree, USER_DRAG_NAME_TYPE, USER_DRAG_TYPE } from './MembershipTree';
import { RuleTree } from './RuleTree';
import { useSearch } from './useSearch';

type Props = {
  api: AdminApi;
  // The session's user: `id` and `username`.
  currentUser: { id: string; username: string } | null;
};

// `is_uuid` in `cubeserver/src/rbac/store.rs`.
const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/;

/**
 * Whether `user` is the session's. A session stored before users had UUIDs
 * carries the username as its id (until the next login): match that one by
 * username. A UUID session is matched by id only, so a rename cannot point
 * it at whoever takes the old name.
 */
export function isCurrentUser(user: User, current: Props['currentUser']): boolean {
  if (!current) {
    return false;
  }
  return user.id === current.id || (!UUID.test(current.id) && user.username === current.username);
}

// `MIN_PASSWORD_LENGTH` in `cubeserver/src/rbac/identity.rs`.
const MIN_PASSWORD = 10;

function passwordError(password: string): string | undefined {
  if (!password) {
    return 'Enter a password';
  }
  if (password.length < MIN_PASSWORD) {
    return `At least ${MIN_PASSWORD} characters`;
  }
  return undefined;
}

type Values = { username: string; name: string; email: string; password: string };
type Missing = { username?: string; password?: string };

const EMPTY: Values = {
  username: '', name: '', email: '', password: '',
};

// A text with a button that copies it.
function Copyable({ text }: { text: string }) {
  const [copied, setCopied] = useState(false);
  return (
    <span className="tw:inline-flex tw:items-center tw:gap-1">
      <span>{text}</span>
      <button
        type="button"
        data-slot="copy-text"
        aria-label="Copy"
        className="tw:cursor-pointer tw:text-muted-foreground tw:hover:text-foreground"
        onClick={(e) => {
          e.stopPropagation();
          navigator.clipboard?.writeText(text).then(() => {
            setCopied(true);
            setTimeout(() => setCopied(false), 1500);
          }, () => {});
        }}
      >
        {copied ? <Check className="tw:size-3.5" /> : <Copy className="tw:size-3.5" />}
      </button>
    </span>
  );
}

function Tip({ title, children }: { title: ReactNode; children: ReactElement }) {
  return (
    <Tooltip>
      <TooltipTrigger asChild>{children}</TooltipTrigger>
      <TooltipContent>{title}</TooltipContent>
    </Tooltip>
  );
}

function PasswordInput({
  id, value, invalid, onChange,
}: { id: string; value: string; invalid: boolean; onChange: (value: string) => void }) {
  const [shown, setShown] = useState(false);
  return (
    <div className="tw:relative">
      <Input
        id={id}
        type={shown ? 'text' : 'password'}
        autoComplete="new-password"
        className="tw:pr-9"
        value={value}
        aria-invalid={invalid}
        onChange={(e) => onChange(e.target.value)}
      />
      <button
        type="button"
        data-slot="password-toggle"
        className="tw:absolute tw:inset-y-0 tw:right-0 tw:flex tw:w-9 tw:cursor-pointer tw:items-center tw:justify-center tw:text-muted-foreground tw:hover:text-foreground"
        aria-label={shown ? 'Hide password' : 'Show password'}
        onClick={() => setShown((s) => !s)}
      >
        {shown ? <EyeOff className="tw:size-4" /> : <Eye className="tw:size-4" />}
      </button>
    </div>
  );
}

export function UsersTab({ api, currentUser }: Props) {
  const {
    q, setQ, page, setPage, result, loading, error: searchError, reload,
  } = useSearch((term, p) => api.searchUsers(term, p), [api]);
  const [creating, setCreating] = useState(false);
  const [editing, setEditing] = useState<User | null>(null);
  const [resetting, setResetting] = useState<User | null>(null);
  const [accessing, setAccessing] = useState<User | null>(null);
  const [attributing, setAttributing] = useState<User | null>(null);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [values, setValues] = useState<Values>(EMPTY);
  const [missing, setMissing] = useState<Missing>({});
  // The side panel's tabs: a tab's content mounts when first shown and then
  // stays, keeping what was opened in it.
  const [tab, setTab] = useState('members');
  const [visited, setVisited] = useState<Set<string>>(() => new Set(['members']));

  function open(set: (u: any) => void, value: any, initial: Partial<Values> = {}) {
    setValues({ ...EMPTY, ...initial });
    setMissing({});
    setError(null);
    set(value);
  }

  function close() {
    setCreating(false);
    setEditing(null);
    setResetting(null);
  }

  function edit(field: keyof Values, value: string) {
    setValues((v) => ({ ...v, [field]: value }));
    setMissing((m) => ({ ...m, [field]: undefined }));
  }

  async function run(action: () => Promise<unknown>, done: string) {
    setSaving(true);
    setError(null);
    try {
      await action();
      close();
      notify.success(done);
      reload();
    } catch (e: any) {
      setError(e.message);
    } finally {
      setSaving(false);
    }
  }

  async function toggle(user: User, enabled: boolean) {
    try {
      await api.updateUser(user.id, { disabled: !enabled });
      notify.success(`${user.username} ${enabled ? 'enabled' : 'disabled'}`);
      reload();
    } catch (e: any) {
      notify.error(e.message);
    }
  }

  // Empty fields are left out, so they keep their value.
  function optional(fields: Record<string, string | undefined>) {
    return Object.fromEntries(Object.entries(fields).filter(([, v]) => v));
  }

  const hasUsername = creating || editing != null;
  const hasPassword = creating || resetting != null;

  async function submit(event?: FormEvent) {
    event?.preventDefault();
    const lacking: Missing = {
      username: hasUsername && !values.username ? 'Enter a username' : undefined,
      password: hasPassword ? passwordError(values.password) : undefined,
    };
    setMissing(lacking);
    if (lacking.username || lacking.password) {
      return;
    }
    if (creating) {
      const user = {
        username: values.username,
        password: values.password,
        ...optional({ name: values.name, email: values.email }),
      };
      await run(() => api.createUser(user), `User ${values.username} created`);
      return;
    }
    if (editing) {
      // The username goes only when it changed: a rename is the one
      // edit the server can refuse (taken, or a pinned account).
      const renamed = values.username && values.username !== editing.username
        ? { username: values.username }
        : {};
      await run(
        () => api.updateUser(editing.id, {
          ...renamed,
          ...optional({ name: values.name, email: values.email }),
        }),
        `${renamed.username ?? editing.username} saved`
      );
      return;
    }
    await run(
      () => api.setPassword(resetting!.id, values.password),
      `New password set for ${resetting!.username}; their sessions ended`
    );
  }

  const modalOpen = creating || editing != null || resetting != null;
  let title = 'New user';
  if (editing) {
    title = `Edit ${editing.username}`;
  } else if (resetting) {
    title = `Reset the password of ${resetting.username}`;
  }

  const rows = result?.items || [];
  const columnCount = 7;

  return (
    <div className="tw:grid tw:grid-cols-1 tw:gap-6 tw:xl:grid-cols-[minmax(0,1fr)_320px]">
      <div className="tw:min-w-0">
        <div className="tw:mb-4 tw:flex tw:items-center tw:gap-2">
          <Button onClick={() => open(setCreating, true)}>
            New user
          </Button>
        </div>

        <SearchInput
          className="tw:mb-4"
          placeholder="Search users by username, name or email"
          value={q}
          onChange={setQ}
        />
        {searchError ? (
          <Alert variant="destructive" className="tw:mb-4">
            <CircleAlert />
            <AlertDescription>{searchError}</AlertDescription>
          </Alert>
        ) : null}

        <TooltipProvider delayDuration={300}>
          <div className="tw:relative">
            {/* the tag and action buttons can outgrow the column; scroll inside it */}
            <div className="tw:rounded-md tw:border">
              <Table>
                <TableHeader>
                  <TableRow>
                    <TableHead style={{ width: 32 }} />
                    <TableHead>Username</TableHead>
                    <TableHead>Id</TableHead>
                    <TableHead>Name</TableHead>
                    <TableHead>Email</TableHead>
                    <TableHead>Enabled</TableHead>
                    <TableHead className="tw:sticky tw:right-0 tw:bg-background" />
                  </TableRow>
                </TableHeader>
                <TableBody>
                  {rows.length === 0 ? (
                    <TableRow>
                      <TableCell colSpan={columnCount}>
                        <EmptyState className="tw:py-6">{loading ? <Spinner className="tw:mx-auto" /> : 'No data'}</EmptyState>
                      </TableCell>
                    </TableRow>
                  ) : rows.map((user) => (
                    <TableRow
                      key={user.id}
                      // Enabled users drag onto a workspace, or a rule, of the tree to
                      // join it.
                      draggable={user.disabled ? undefined : true}
                      onDragStart={user.disabled ? undefined : (e) => {
                        e.dataTransfer.setData(USER_DRAG_TYPE, user.id);
                        e.dataTransfer.setData(USER_DRAG_NAME_TYPE, user.username);
                        e.dataTransfer.effectAllowed = 'copy';
                      }}
                      className={user.disabled ? undefined : 'tw:cursor-grab'}
                    >
                      <TableCell>
                        {user.disabled ? null : (
                          <span title="Drag onto a workspace" className="tw:inline-flex tw:text-muted-foreground">
                            <GripVertical className="tw:size-4" />
                          </span>
                        )}
                      </TableCell>
                      <TableCell>
                        <span className="tw:inline-flex tw:items-center tw:gap-1.5">
                          <Copyable text={user.username} />
                          {isCurrentUser(user, currentUser) ? <Tag>you</Tag> : null}
                        </span>
                      </TableCell>
                      <TableCell><IdText id={user.id} /></TableCell>
                      <TableCell>{user.name || '—'}</TableCell>
                      <TableCell>
                        {/* long addresses end in an ellipsis; the whole one is the title */}
                        {user.email ? (
                          <span title={user.email} className="tw:block tw:max-w-[110px] tw:truncate">
                            {user.email}
                          </span>
                        ) : '—'}
                      </TableCell>
                      <TableCell>
                        <Switch
                          size="sm"
                          checked={!user.disabled}
                          // A platform admin cannot lock themselves out.
                          disabled={isCurrentUser(user, currentUser)}
                          onCheckedChange={(enabled) => toggle(user, enabled)}
                        />
                      </TableCell>
                      {/* Icon buttons, so the actions fit beside the workspaces
                          panel; the name is in the tooltip and the aria-label. */}
                      <TableCell className="tw:sticky tw:right-0 tw:bg-background tw:text-right">
                        <span className="tw:inline-flex tw:items-center tw:gap-1">
                          <Tip title="Edit">
                            <Button
                              size="icon-sm"
                              variant="outline"
                              aria-label={`Edit ${user.username}`}
                              onClick={() => open(setEditing, user, {
                                username: user.username,
                                name: user.name || '',
                                email: user.email || '',
                              })}
                            >
                              <Pencil />
                            </Button>
                          </Tip>
                          <Tip title="Access">
                            <Button
                              size="icon-sm"
                              variant="outline"
                              aria-label={`Access of ${user.username}`}
                              onClick={() => setAccessing(user)}
                            >
                              <ShieldCheck />
                            </Button>
                          </Tip>
                          <Tip title="Attributes">
                            <Button
                              size="icon-sm"
                              variant="outline"
                              aria-label={`Attributes of ${user.username}`}
                              onClick={() => setAttributing(user)}
                            >
                              <Tags />
                            </Button>
                          </Tip>
                          <Tip title="Reset password">
                            <Button
                              size="icon-sm"
                              variant="outline"
                              aria-label={`Reset the password of ${user.username}`}
                              onClick={() => open(setResetting, user)}
                            >
                              <Lock />
                            </Button>
                          </Tip>
                        </span>
                      </TableCell>
                    </TableRow>
                  ))}
                </TableBody>
              </Table>
            </div>
            {loading && rows.length > 0 ? (
              <div className="tw:absolute tw:inset-0 tw:flex tw:items-center tw:justify-center tw:bg-background/50">
                <Spinner className="tw:size-5" />
              </div>
            ) : null}
          </div>
        </TooltipProvider>
        <Pager current={page} pageSize={PAGE_SIZE} total={result?.total ?? 0} onChange={setPage} />

        <FormDialog
          title={title}
          open={modalOpen}
          onClose={close}
          onOk={() => submit()}
          okText="Save"
          saving={saving}
        >
          <form className="tw:flex tw:flex-col tw:gap-4" onSubmit={submit} noValidate>
            {hasUsername ? (
              <Field label="Username" htmlFor="username" error={missing.username}>
                <Input
                  id="username"
                  autoComplete="off"
                  value={values.username}
                  aria-invalid={Boolean(missing.username)}
                  onChange={(e) => edit('username', e.target.value)}
                />
              </Field>
            ) : null}

            {resetting == null ? (
              <>
                <Field label="Name" htmlFor="name">
                  <Input
                    id="name"
                    autoComplete="off"
                    value={values.name}
                    onChange={(e) => edit('name', e.target.value)}
                  />
                </Field>
                <Field label="Email" htmlFor="email">
                  <Input
                    id="email"
                    type="email"
                    autoComplete="off"
                    value={values.email}
                    onChange={(e) => edit('email', e.target.value)}
                  />
                </Field>
              </>
            ) : null}

            {hasPassword ? (
              <Field label="Password" htmlFor="password" error={missing.password}>
                <PasswordInput
                  id="password"
                  value={values.password}
                  invalid={Boolean(missing.password)}
                  onChange={(v) => edit('password', v)}
                />
              </Field>
            ) : null}

            {error ? (
              <Alert variant="destructive">
                <CircleAlert />
                <AlertDescription>{error}</AlertDescription>
              </Alert>
            ) : null}
            {/* Enter in a field saves. */}
            <button type="submit" hidden aria-hidden tabIndex={-1} />
          </form>
        </FormDialog>
        <AccessModal api={api} user={accessing} onClose={() => setAccessing(null)} />
        <AttributesModal api={api} user={attributing} onClose={() => setAttributing(null)} />
      </div>
      <div className="tw:min-w-0">
        <Tabs
          value={tab}
          onValueChange={(next) => {
            setTab(next);
            setVisited((v) => new Set(v).add(next));
          }}
        >
          <TabsList>
            <TabsTrigger value="members">Workspaces</TabsTrigger>
            <TabsTrigger value="rules">Rules</TabsTrigger>
          </TabsList>
          <TabsContent value="members" forceMount hidden={tab !== 'members'}>
            <p className="tw:mb-3 tw:text-sm tw:text-muted-foreground">
              Drag a user onto a workspace to add them as a viewer.
            </p>
            <MembershipTree api={api} />
          </TabsContent>
          <TabsContent value="rules" forceMount hidden={tab !== 'rules'}>
            {visited.has('rules') ? (
              <>
                <p className="tw:mb-3 tw:text-sm tw:text-muted-foreground">
                  Drag a user onto a rule to give them its role in the rule&apos;s workspace.
                </p>
                <RuleTree api={api} />
              </>
            ) : null}
          </TabsContent>
        </Tabs>
      </div>
    </div>
  );
}
