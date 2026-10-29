import styled from 'styled-components';
import { Alert as AntdAlert } from 'antd';

export const TYPES: any = {
  error: {
    border: 'var(--pg-alert-error-border, #FFCCC7)',
    background: 'var(--pg-alert-error-bg, rgb(255, 100, 109, 0.05))',
    color: 'var(--pg-alert-error-text, #EF404A)',
  },
  warning: {
    border: 'var(--pg-alert-warning-border, #FFE58F)',
    background: 'var(--pg-alert-warning-bg, #FFFBE6)',
  },
  info: {
    border: 'var(--pg-alert-info-border, #91D5FF)',
    background: 'var(--pg-alert-info-bg, #E6F7FF)',
  },
  success: {
    border: 'var(--pg-alert-success-border, #B7EB8F)',
    background: 'var(--pg-alert-success-bg, #F6FFED)',
  },
};

export const Alert: typeof AntdAlert = styled(AntdAlert)`
  && {
    background: ${(props) => TYPES[props.type || 'info'].background};
    border: 1px solid ${(props) => TYPES[props.type || 'info'].border};
    color: ${(props) => (props.type && TYPES[props.type]?.color) || 'inherit'};
    box-sizing: border-box;
    border-radius: 2px;
  }
`;
