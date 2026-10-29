import {
  Alert, Form, Input, Modal, Select,
} from 'antd';
import { useEffect, useState } from 'react';

type Choice = {
  field: 'owner' | 'organization';
  label: string;
  message: string;
  options: { value: string; label: string }[];
  initial?: string;
};

export type NameValues = { name: string; owner?: string; organization?: string };

type Props = {
  title: string | null;
  initialName?: string;
  // Asks for one more value from a list as well: the first owner of a new
  // organization, or the organization of a new workspace.
  choice?: Choice;
  onSubmit: (values: NameValues) => Promise<unknown>;
  onClose: () => void;
};

// A modal asking for a name; open while `title` is set. A failure keeps it
// open with the server's message.
export function NameDialog({
  title, initialName = '', choice, onSubmit, onClose,
}: Props) {
  const [form] = Form.useForm();
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (title != null) {
      form.resetFields();
      form.setFieldsValue({ name: initialName });
      if (choice) {
        form.setFieldsValue({ [choice.field]: choice.initial });
      }
      setError(null);
    }
  }, [title, initialName, choice?.initial]);

  async function submit(values: NameValues) {
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
        {choice ? (
          <Form.Item
            label={choice.label}
            name={choice.field}
            rules={[{ required: true, message: choice.message }]}
          >
            <Select showSearch optionFilterProp="label" options={choice.options} />
          </Form.Item>
        ) : null}
        {error ? <Alert type="error" message={error} showIcon /> : null}
      </Form>
    </Modal>
  );
}
