/* eslint-disable no-undef,react/jsx-no-target-blank */
import '@ant-design/compatible/assets/index.css';
import './shadcn.css';
import { Alert, Layout } from 'antd';
import { Component, PropsWithChildren, useEffect } from 'react';
import { RouteComponentProps, withRouter } from 'react-router-dom';
import styled from 'styled-components';
import { Root } from '@cube-dev/ui-kit';

import { CubeLoader } from './atoms';
import { AppContextConsumer, PlaygroundContext } from './components/AppContext';
import GlobalStyles from './components/GlobalStyles';
import { Toaster } from './components/ui/sonner';
import { installColumnResize } from './shared/column-resize';
import Header from './components/Header/Header';
import { Sidebar } from './components/Sidebar/Sidebar';
import { LivePreviewContextProvider } from './components/LivePreviewContext/LivePreviewContextProvider';
import {
  event,
  setAnonymousId,
  setTelemetry,
  setTracker,
  trackImpl,
} from './events';
import { useAppContext } from './hooks';
import { LoginPage } from './pages/Login/LoginPage';
import { QUERY_BUILDER_COLOR_TOKENS } from './QueryBuilderV2';
import { getSession, Session } from './shared/auth';
import { setPlaygroundKey } from './shared/helpers';
import ThemeStyles from './theme/ThemeStyles';

const StyledLayoutContent = styled(Layout.Content)`
  flex: 1;
`;

// Right of the sidebar: the top bar over the page.
const Main = styled.div`
  display: flex;
  flex-direction: column;
  min-width: 0;
`;

// A token saved from the Security Context dialog would win over the login's;
// it was signed for no user.
function forgetSecurityContextToken() {
  try {
    window.localStorage.removeItem('cubejsToken');
  } catch {
    // Storage unavailable: nothing was saved there either.
  }
}

type AppState = {
  fatalError: Error | null;
  context: PlaygroundContext | null;
  // With RBAC on: the context, until the user logs in.
  loginContext: PlaygroundContext | null;
  showLoader: boolean;
  isAppContextSet: boolean;
};

const ROOT_STYLES = {
  height: 'min 100vh',
  display: 'grid',
  gridTemplateColumns: 'max-content minmax(0, 1fr)',
  ...QUERY_BUILDER_COLOR_TOKENS,
};

class App extends Component<PropsWithChildren<RouteComponentProps>, AppState> {
  static getDerivedStateFromError(error) {
    return { fatalError: error };
  }

  constructor(props: PropsWithChildren<RouteComponentProps>) {
    super(props);

    this.state = {
      fatalError: null,
      context: null,
      loginContext: null,
      showLoader: false,
      isAppContextSet: false,
    };
  }

  async componentDidMount() {
    setTimeout(() => this.setState({ showLoader: true }), 700);

    window.addEventListener('unhandledrejection', (promiseRejectionEvent) => {
      const error = promiseRejectionEvent.reason;
      console.log(error);
      const e = (error.stack || error).toString();
      event('Playground Error', {
        error: e,
      });
    });

    const res = await fetch('playground/context');
    const context = await res.json();

    setPlaygroundKey(context.playgroundKey);
    setTelemetry(context.telemetry);
    setTracker(trackImpl);
    setAnonymousId(context.anonymousId, {
      coreServerVersion: context.coreServerVersion,
      projectFingerprint: context.projectFingerprint,
      isDocker: Boolean(context.isDocker),
      dockerVersion: context.dockerVersion,
    });

    if (context.authRequired) {
      forgetSecurityContextToken();
      const session = getSession();
      if (session == null) {
        this.setState({ loginContext: context });
      } else {
        this.setState({ context: { ...context, cubejsToken: session.token } });
      }
    } else {
      this.setState({ context });
    }
  }

  onLogin(session: Session) {
    const { loginContext } = this.state;
    forgetSecurityContextToken();
    this.setState({
      context: { ...loginContext!, cubejsToken: session.token },
      loginContext: null,
    });
  }

  componentDidCatch(error, info) {
    event('Playground Error', {
      error: (error.stack || error).toString(),
      info: info.toString(),
    });
  }

  render() {
    const { location, children } = this.props;
    const {
      context, loginContext, fatalError, isAppContextSet, showLoader,
    } = this.state;

    if (loginContext != null) {
      return (
        <>
          <GlobalStyles />
          <ThemeStyles />
          <LoginPage
            basePath={loginContext.basePath}
            onLogin={(session) => this.onLogin(session)}
          />
        </>
      );
    }

    if (context != null && !isAppContextSet) {
      return (
        <>
          <ContextSetter context={context} />
          <AppContextConsumer
            onReady={() => this.setState({ isAppContextSet: true })}
          />
        </>
      );
    }

    if (context == null && !isAppContextSet) {
      return showLoader ? <CubeLoader /> : null;
    }

    if (fatalError) {
      console.log(fatalError.stack);
    }

    return (
      <LivePreviewContextProvider
        disabled={!context?.livePreview}
      >
        <Root publicUrl="." styles={ROOT_STYLES}>
          <GlobalStyles />
          <ThemeStyles />

          <Sidebar pathname={location.pathname} />

          <Main>
            <Header pathname={location.pathname} />

            <StyledLayoutContent>
              {fatalError ? (
                <Alert
                  message="Error occured while rendering"
                  description={fatalError.stack || ''}
                  type="error"
                />
              ) : (
                children
              )}
            </StyledLayoutContent>
          </Main>
        </Root>
        {/*
          shadcn/ui notifications (`toast`), in place of antd's `message`.
          Outside Root: Root is a two-column grid (sidebar, main), and the
          Toaster's <section> would take a column.
        */}
        <Toaster richColors closeButton position="top-center" />
        <ColumnResize />
      </LivePreviewContextProvider>
    );
  }
}

// Every table's columns resize by hand (src/shared/column-resize.ts).
function ColumnResize() {
  useEffect(() => installColumnResize(document.body), []);
  return null;
}

type ContextSetterProps = {
  context: PlaygroundContext;
};

function ContextSetter({ context }: ContextSetterProps) {
  const { setContext } = useAppContext();

  useEffect(() => {
    if (context !== null) {
      setContext({
        ready: true,
        playgroundContext: {
          ...context,
          isCloud: false,
        },
        identifier: context.identifier,
      });
    }
  }, [context]);

  return null;
}

export default withRouter(App);
