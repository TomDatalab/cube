import { MinusCircleOutlined, PlusOutlined } from '@ant-design/icons';
import {
  Alert, Button, Form, Input, message, Modal, Select, Space, Spin, Typography,
} from 'antd';
import { useEffect, useState } from 'react';

import { useIsMounted } from '../../hooks';
import { AdminApi, Attributes, User } from './api';

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
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!user) {
      return;
    }
    setError(null);
    setLoading(true);
    form.setFieldsValue({ pairs: [] });
    api.userAttributes(user.id).then((attributes) => {
      if (isMounted()) {
        form.setFieldsValue({
          pairs: Object.entries(attributes).map(([name, values]) => ({ name, values })),
        });
      }
    }).catch((e: any) => {
      if (isMounted()) {
        setError(e.message);
      }
    }).finally(() => {
      if (isMounted()) {
        setLoading(false);
      }
    });
  }, [api, user]);

  async function save({ pairs }: { pairs?: Pair[] }) {
    const attributes: Attributes = {};
    for (const { name, values } of pairs || []) {
      const key = (name || '').trim();
      if (key in attributes) {
        setError(`The attribute ${key} is listed twice`);
        return;
      }
      attributes[key] = values || [];
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
      okButtonProps={{ disabled: loading }}
      destroyOnClose
    >
      <Typography.Paragraph type="secondary">
        Role row filters can match a member against one of these by name.
      </Typography.Paragraph>
      {loading ? <Spin style={{ display: 'block', margin: '8px auto' }} /> : null}
      <div style={{ display: loading ? 'none' : undefined }}>
        <Form form={form} layout="vertical" requiredMark={false} onFinish={save}>
          <Form.List name="pairs">
            {(fields, { add, remove }) => (
              <>
                {fields.map((field) => (
                  <Space key={field.key} align="start" style={{ display: 'flex' }}>
                    <Form.Item
                      name={[field.name, 'name']}
                      rules={[{ required: true, whitespace: true, message: 'Name the attribute' }]}
                    >
                      <Input placeholder="Name, e.g. region" autoComplete="off" style={{ width: 160 }} />
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
          {error ? <Alert type="error" message={error} showIcon style={{ marginTop: 16 }} /> : null}
        </Form>
      </div>
    </Modal>
  );
}
