import { MinusCircleOutlined, PlusOutlined } from '@ant-design/icons';
import {
  Button, Form, Input, Radio, Select, Space, Typography,
} from 'antd';

import {
  ATTRIBUTE_NAME, ModelCube, Restrictions, RowFilter,
} from './api';

// A row filter as the form edits it: `source` picks which of `attribute` and
// `values` is kept on save.
export type FilterField = {
  member?: string;
  operator?: RowFilter['operator'];
  source?: 'attribute' | 'values';
  attribute?: string;
  values?: string[];
};

export type RestrictionsFormValues = { rowFilters?: FilterField[]; blockedMembers?: string[] };

export function toFormRestrictions(r: Restrictions): RestrictionsFormValues {
  return {
    rowFilters: r.rowFilters.map((f) => ({
      member: f.member,
      operator: f.operator,
      source: f.attribute != null ? 'attribute' : 'values',
      attribute: f.attribute,
      values: f.values ?? [],
    })),
    blockedMembers: r.blockedMembers,
  };
}

// Sends exactly `attribute` OR `values` per filter, as its source says.
export function toRestrictions(v: RestrictionsFormValues | undefined): Restrictions {
  return {
    rowFilters: (v?.rowFilters || []).map((f): RowFilter => (f.source === 'attribute'
      ? { member: f.member!, operator: f.operator!, attribute: (f.attribute || '').trim() }
      : { member: f.member!, operator: f.operator!, values: f.values || [] })),
    blockedMembers: v?.blockedMembers || [],
  };
}

type Props = { cubes: ModelCube[] };

const OPERATORS = [
  { value: 'equals', label: 'equals' },
  { value: 'notEquals', label: 'not equals' },
];

// The "Row filters" and "Blocked columns" sections of the role form, in the
// fields `restrictions.rowFilters` and `restrictions.blockedMembers`.
export function RestrictionsFields({ cubes }: Props) {
  return (
    <>
      <Typography.Title level={5}>Row filters</Typography.Title>
      <Form.List name={['restrictions', 'rowFilters']}>
        {(fields, { add, remove }) => (
          <>
            {fields.map((field) => (
              <div
                key={field.key}
                style={{
                  border: '1px solid var(--pg-line)',
                  borderRadius: 4,
                  padding: 8,
                  marginBottom: 8,
                }}
              >
                <Space align="start" wrap>
                  <Form.Item
                    name={[field.name, 'member']}
                    style={{ marginBottom: 0 }}
                    rules={[{ required: true, message: 'Pick a dimension' }]}
                  >
                    <Select
                      showSearch
                      placeholder="Member"
                      style={{ width: 200 }}
                      aria-label="Row filter member"
                    >
                      {cubes.map((cube) => (
                        <Select.OptGroup key={cube.name} label={cube.title || cube.name}>
                          {cube.dimensions.map((d) => (
                            <Select.Option key={d.name} value={d.name}>{d.name}</Select.Option>
                          ))}
                        </Select.OptGroup>
                      ))}
                    </Select>
                  </Form.Item>
                  <Form.Item name={[field.name, 'operator']} style={{ marginBottom: 0 }}>
                    <Select style={{ width: 120 }} options={OPERATORS} aria-label="Row filter operator" />
                  </Form.Item>
                  <Button
                    type="text"
                    aria-label="Remove filter"
                    icon={<MinusCircleOutlined />}
                    onClick={() => remove(field.name)}
                  />
                </Space>
                <Form.Item name={[field.name, 'source']} style={{ margin: '8px 0 4px' }}>
                  <Radio.Group>
                    <Radio value="attribute">User attribute</Radio>
                    <Radio value="values">Fixed values</Radio>
                  </Radio.Group>
                </Form.Item>
                <Form.Item noStyle shouldUpdate>
                  {({ getFieldValue }) => (getFieldValue(['restrictions', 'rowFilters', field.name, 'source']) === 'attribute' ? (
                    <Form.Item
                      name={[field.name, 'attribute']}
                      style={{ marginBottom: 0 }}
                      rules={[
                        { required: true, message: 'Name the attribute' },
                        { pattern: ATTRIBUTE_NAME, message: 'a-z, 0-9 and _, up to 64' },
                      ]}
                    >
                      <Input placeholder="Attribute name, e.g. region" maxLength={64} autoComplete="off" />
                    </Form.Item>
                  ) : (
                    <Form.Item
                      name={[field.name, 'values']}
                      style={{ marginBottom: 0 }}
                      rules={[{ required: true, type: 'array', min: 1, message: 'Add at least one value' }]}
                    >
                      <Select mode="tags" placeholder="Values" tokenSeparators={[',']} open={false} />
                    </Form.Item>
                  ))}
                </Form.Item>
              </div>
            ))}
            <Button
              type="dashed"
              icon={<PlusOutlined />}
              onClick={() => add({ operator: 'equals', source: 'attribute' })}
            >
              Add filter
            </Button>
          </>
        )}
      </Form.List>

      <Typography.Title level={5} style={{ marginTop: 16 }}>Blocked columns</Typography.Title>
      <Form.Item name={['restrictions', 'blockedMembers']}>
        <Select
          mode="multiple"
          showSearch
          placeholder="Pick members to block"
          aria-label="Blocked members"
        >
          {cubes.map((cube) => (
            <Select.OptGroup key={cube.name} label={cube.title || cube.name}>
              {[...cube.measures, ...cube.dimensions, ...cube.segments].map((m) => (
                <Select.Option key={m.name} value={m.name}>{m.name}</Select.Option>
              ))}
            </Select.OptGroup>
          ))}
        </Select>
      </Form.Item>
    </>
  );
}
