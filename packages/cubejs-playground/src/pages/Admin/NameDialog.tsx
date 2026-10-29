import {
  Alert, Form, Input, Modal, Select,
} from 'antd';
import { useEffect, useState } from 'react';

type Props = {
  title: string | null;
  initialName?: string;
  // Asks for a first owner as well (a new organization).
  ownerOptions?: { value: string; label: string }[];
  onSubmit: (values: { name: string; owner?: string }) => Promise<unknown>;
  onClose: () => void;
};

// A modal asking for a name; open while `title` is set. A failure keeps it
// open with the server's message.
export function NameDialog({
  title, initialName = '', ownerOptions, onSubmit, onClose,
}: Props) {
  const [form] = Form.useForm();
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (title != null) {
      form.resetFields();
      form.setFieldsValue({ name: initialName });
      setError(null);
    }
  }, [title, initialName]);

  async function submit(values: { name: string; owner?: string }) {
    setSaving(true);
    setError(null);
    try {
      await onSubmit(values);
      onClose();
    } catch (e: any) {
      setError(e.message);
    } finally {
      setSaving(false);
    }
  }

  return (
    <Modal
      title={title}
      visible={title != null}
      onCancel={onClose}
      onOk={() => form.submit()}
      okText="Save"
      confirmLoading={saving}
      forceRender
    >
      <Form form={form} layout="vertical" requiredMark={false} onFinish={submit}>
        <Form.Item label="Name" name="name" rules={[{ required: true, message: 'Enter a name' }]}>
          <Input autoComplete="off" />
        </Form.Item>
        {ownerOptions ? (
          <Form.Item
            label="Owner"
            name="owner"
            rules={[{ required: true, message: 'Pick the first owner' }]}
          >
            <Select showSearch options={ownerOptions} />
          </Form.Item>
        ) : null}
        {error ? <Alert type="error" message={error} showIcon /> : null}
      </Form>
    </Modal>
  );
}
