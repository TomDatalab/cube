import { MinusCircleOutlined, PlusOutlined } from '@ant-design/icons';
import {
  Alert, Button, Form, Input, message, Modal, Select, Space, Spin, Typography,
} from 'antd';
import { useEffect, useState } from 'react';

import { useIsMounted } from '../../hooks';
import {
  AdminApi, ATTRIBUTE_NAME, attributesFromPairs, User,
} from './api';

type Props = {
  api: AdminApi;
  user: User | null;
  onClose: () => void;
};

type Pair = { name?: string; values?: string[] };

// A user's attributes: name -> values. A role's row filter can read one by
// name, so each user sees only the rows their own values allow.
export function AttributesModal({ api, user, onClose }: Props) {
  const isMounted = useIsMounted();
  const [form] = Form.useForm<{ pairs: Pair[] }>();
  const [loading, setLoading] = useState(false);
  // Saving replaces the whole set, so it waits for a successful load.
  const [loaded, setLoaded] = useState(false);
  const [attempt, setAttempt] = useState(0);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!user) {
      return;
    }
    // The modal stays mounted: an answer for another user, or for an earlier
    // attempt, must neither fill the form nor allow saving.
    let cancelled = false;
    const current = () => !cancelled && isMounted();
    setError(null);
    setLoaded(false);
    setLoading(true);
    form.setFieldsValue({ pairs: [] });
    api.userAttributes(user.id).then((attributes) => {
      if (current()) {
        form.setFieldsValue({
          pairs: Object.entries(attributes).map(([name, values]) => ({ name, values })),
        });
        setLoaded(true);
      }
    }).catch((e: any) => {
      if (current()) {
        setError(e.message);
      }
    }).finally(() => {
      if (current()) {
        setLoading(false);
      }
    });
    return () => {
      cancelled = true;
    };
  }, [api, user, attempt]);

  async function save({ pairs }: { pairs?: Pair[] }) {
    let attributes;
    try {
      attributes = attributesFromPairs(pairs || []);
    } catch (e: any) {
      setError(e.message);
      return;
    }
    setSaving(true);
    setError(null);
    try {
      await api.setUserAttributes(user!.id, attributes);
      message.success(`Attributes of ${user!.id} saved`);
      onClose();
    } catch (e: any) {
      setError(e.message);
    } finally {
      setSaving(false);
    }
  }

  return (
    <Modal
      title={user ? `Attributes of ${user.id}` : 'Attributes'}
      visible={user != null}
      onCancel={onClose}
      onOk={() => form.submit()}
      okText="Save"
      confirmLoading={saving}
      okButtonProps={{ disabled: !loaded }}
      destroyOnClose
    >
      <Typography.Paragraph type="secondary">
        Role row filters can match a member against one of these by name.
      </Typography.Paragraph>
      {loading ? <Spin style={{ display: 'block', margin: '8px auto' }} /> : null}
      {!loaded && error ? (
        <Alert
          type="error"
          message={error}
          showIcon
          action={<Button size="small" onClick={() => setAttempt(attempt + 1)}>Retry</Button>}
        />
      ) : null}
      <div style={{ display: loaded ? undefined : 'none' }}>
        <Form form={form} layout="vertical" requiredMark={false} onFinish={save}>
          <Form.List name="pairs">
            {(fields, { add, remove }) => (
              <>
                {fields.map((field) => (
                  <Space key={field.key} align="start" style={{ display: 'flex' }}>
                    <Form.Item
                      name={[field.name, 'name']}
                      rules={[
                        { required: true, message: 'Name the attribute' },
                        { pattern: ATTRIBUTE_NAME, message: 'a-z, 0-9 and _, up to 64' },
                      ]}
                    >
                      <Input placeholder="Name, e.g. region" maxLength={64} autoComplete="off" style={{ width: 160 }} />
                    </Form.Item>
                    <Form.Item
                      name={[field.name, 'values']}
                      rules={[{ required: true, type: 'array', min: 1, message: 'Add at least one value' }]}
                    >
                      <Select
                        mode="tags"
                        placeholder="Values"
                        tokenSeparators={[',']}
                        open={false}
                        style={{ width: 240 }}
                      />
                    </Form.Item>
                    <Button
                      type="text"
                      aria-label="Remove attribute"
                      icon={<MinusCircleOutlined />}
                      onClick={() => remove(field.name)}
                    />
                  </Space>
                ))}
                <Button type="dashed" icon={<PlusOutlined />} onClick={() => add({ values: [] })}>
                  Add attribute
                </Button>
              </>
            )}
          </Form.List>
          {loaded && error ? <Alert type="error" message={error} showIcon style={{ marginTop: 16 }} /> : null}
        </Form>
      </div>
    </Modal>
  );
}
